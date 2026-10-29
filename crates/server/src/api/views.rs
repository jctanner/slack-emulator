//! `views.*`: modals (`open`, `push`, `update`) and App Home tabs (`publish`).

use serde_json::{Value, json};
use slack_emu_core::blocks::validate_view;
use slack_emu_core::models::App;

use super::args::{self, opt_str, req_str};
use super::conversations::{fail, team_user};
use super::{ApiError, ApiResult, Args, Caller};
use crate::AppState;
use crate::interactions::{self, MAX_MODAL_STACK, View};

/// The installed app behind the caller's token.
pub(super) async fn caller_app(state: &AppState, caller: &Caller) -> ApiResult<App> {
    let Some(app_id) = caller.token.as_ref().and_then(|t| t.app_id.as_deref()) else {
        return fail("not_allowed_token_type");
    };
    match state.store.get_app(app_id).await? {
        Some(a) if a.is_installed() => Ok(a),
        _ => fail("invalid_auth"),
    }
}

pub(crate) fn invalid_view(errors: Vec<String>) -> ApiError {
    ApiError::invalid_arguments("").with("response_metadata", json!({ "messages": errors }))
}

/// Validates a view definition (from an argument or an app's response).
pub(crate) fn check_view(mut view: Value, expected: Option<&str>) -> ApiResult<Value> {
    let errors = validate_view(&mut view, expected);
    if errors.is_empty() {
        Ok(view)
    } else {
        Err(invalid_view(errors))
    }
}

fn view_arg(args: &Args, expected: Option<&str>) -> ApiResult<Value> {
    let view = args::json(args, "view")?.ok_or_else(|| ApiError::invalid_arguments("missing required field: view"))?;
    check_view(view, expected)
}

fn trigger_arg(args: &Args) -> ApiResult<String> {
    opt_str(args, "trigger_id")
        .or_else(|| opt_str(args, "interactivity_pointer"))
        .ok_or_else(|| ApiError::invalid_arguments("missing required field: trigger_id"))
}

fn parse_view(data: Option<Value>) -> Option<View> {
    data.and_then(|d| serde_json::from_value(d).ok())
}

/// `external_id`s are unique per app.
async fn check_external_id(state: &AppState, app: &App, view: &Value, except: Option<&str>) -> ApiResult<()> {
    let Some(ext) = view["external_id"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(());
    };
    let other = |v: &View| v.app_id == app.id && v.external_id() == Some(ext) && Some(v.id.as_str()) != except;
    let in_modal = state.interactions.find_modal(other).is_some();
    let in_home = parse_view(state.store.home_view_by_external_id(&app.id, ext).await?).is_some_and(|v| other(&v));
    if in_modal || in_home {
        return fail("duplicate_external_id");
    }
    Ok(())
}

pub async fn open(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = caller_app(state, caller).await?;
    let trigger_id = trigger_arg(args)?;
    let definition = view_arg(args, Some("modal"))?;
    check_external_id(state, &app, &definition, None).await?;
    let t = interactions::redeem_trigger(state, &trigger_id, &app.id)?;
    let view = View::new(&app, &t.user_id, definition, None);
    // A new modal replaces whatever the user had open.
    state
        .interactions
        .with_stack(&t.user_id, |stack| *stack = vec![view.clone()]);
    interactions::publish_modals(state, &t.team_id, &t.user_id);
    Ok(json!({"ok": true, "view": view.to_json()}))
}

pub async fn push(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = caller_app(state, caller).await?;
    let trigger_id = trigger_arg(args)?;
    let definition = view_arg(args, Some("modal"))?;
    check_external_id(state, &app, &definition, None).await?;
    let t = interactions::redeem_trigger(state, &trigger_id, &app.id)?;
    let view = state.interactions.with_stack(&t.user_id, |stack| {
        let Some(top) = stack.last().filter(|v| v.app_id == app.id) else {
            return fail("not_found");
        };
        if stack.len() >= MAX_MODAL_STACK {
            return fail("push_limit_reached");
        }
        let view = View::new(&app, &t.user_id, definition, Some(top));
        stack.push(view.clone());
        Ok(view)
    })?;
    interactions::publish_modals(state, &t.team_id, &t.user_id);
    Ok(json!({"ok": true, "view": view.to_json()}))
}

pub async fn update(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = caller_app(state, caller).await?;
    let found = match (opt_str(args, "view_id"), opt_str(args, "external_id")) {
        (Some(id), _) => interactions::find_view(state, &id).await?,
        (None, Some(ext)) => match state
            .interactions
            .find_modal(|v| v.app_id == app.id && v.external_id() == Some(&ext))
        {
            Some(v) => Some(v),
            None => parse_view(state.store.home_view_by_external_id(&app.id, &ext).await?),
        },
        (None, None) => return Err(ApiError::invalid_arguments("missing required field: view_id")),
    };
    let Some(mut view) = found.filter(|v| v.app_id == app.id) else {
        return fail("not_found");
    };
    if let Some(hash) = opt_str(args, "hash")
        && hash != view.hash
    {
        return fail("hash_conflict");
    }
    let kind = view.definition["type"].as_str().unwrap_or("modal").to_owned();
    let definition = view_arg(args, Some(&kind))?;
    check_external_id(state, &app, &definition, Some(&view.id)).await?;
    view.replace(definition);
    interactions::save_view(state, &view).await?;
    Ok(json!({"ok": true, "view": view.to_json()}))
}

pub async fn publish(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let app = caller_app(state, caller).await?;
    let user = team_user(state, caller, &req_str(args, "user_id")?).await?;
    let definition = view_arg(args, Some("home"))?;
    let existing = parse_view(state.store.home_view(&app.id, &user.id).await?);
    if let (Some(hash), Some(v)) = (opt_str(args, "hash"), &existing)
        && hash != v.hash
    {
        return fail("hash_conflict");
    }
    check_external_id(state, &app, &definition, existing.as_ref().map(|v| v.id.as_str())).await?;
    let view = match existing {
        Some(mut v) => {
            v.replace(definition);
            v
        }
        None => View::new(&app, &user.id, definition, None),
    };
    interactions::save_view(state, &view).await?;
    Ok(json!({"ok": true, "view": view.to_json()}))
}
