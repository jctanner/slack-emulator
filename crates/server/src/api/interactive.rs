//! What people do in the web UI that apps hear about: clicking buttons and
//! menus (`block_actions`), submitting and closing modals, running slash
//! commands, and opening an app's Home tab. These methods are for the web UI
//! only (`chat.command` is what Slack's own client calls).

use serde_json::{Map, Value, json};
use slack_emu_core::blocks::{check_view_values, find_element};
use slack_emu_core::manifest::SlashCommand;
use slack_emu_core::models::{App, User};

use super::args::{self, bool_arg, opt_str, req_str};
use super::chat::link_names;
use super::conversations::{fail, find_conversation, visible};
use super::views::check_view;
use super::{ApiError, ApiResult, Args, Caller, shapes};
use crate::AppState;
use crate::delivery::event_callback;
use crate::interactions::{
    self, MAX_MODAL_STACK, ResponseTarget, Source, View, channel_name, response_args, team_obj, user_obj,
};

/// An installed app that can receive interactions.
async fn interactive_app(state: &AppState, app_id: Option<&str>) -> ApiResult<App> {
    let Some(app) = (match app_id {
        Some(id) => state.store.get_app(id).await?,
        None => None,
    }) else {
        return fail("not_an_app");
    };
    if !app.is_installed() {
        return fail("app_not_installed");
    }
    if !app.manifest().interactivity_enabled() {
        return fail("interactivity_disabled");
    }
    Ok(app)
}

/// The app didn't take the interaction: `dispatch_failed`, with why.
fn dispatch_failed(reason: String) -> ApiError {
    ApiError::from(slack_emu_core::SlackError::new("dispatch_failed")).with("detail", json!(reason))
}

/// `state.values` from the UI.
fn values_arg(args: &Args) -> ApiResult<Map<String, Value>> {
    Ok(match args::json(args, "state")? {
        Some(Value::Object(mut s)) => match s.remove("values") {
            Some(Value::Object(v)) => v,
            _ => s,
        },
        _ => Map::new(),
    })
}

/// The `actions[0]` object: the element's own fields plus what the user picked.
fn action(state: &AppState, el: &Value, block_id: &str, picked: &Value) -> Value {
    let mut a = json!({
        "type": el["type"],
        "block_id": block_id,
        "action_id": el["action_id"],
        "action_ts": state.clock.next(),
    });
    if el["type"] == "button" {
        for key in ["text", "value", "style"] {
            if let Some(v) = el.get(key) {
                a[key] = v.clone();
            }
        }
    } else {
        for (k, v) in picked.as_object().into_iter().flatten().filter(|(k, _)| *k != "type") {
            a[k] = v.clone();
        }
        if let Some(p) = el.get("placeholder") {
            a["placeholder"] = p.clone();
        }
    }
    a
}

fn find_in_message(message: &Value, block_id: &str, action_id: &str) -> Option<Value> {
    find_element(&message["blocks"], block_id, action_id)
        .or_else(|| {
            message["attachments"]
                .as_array()?
                .iter()
                .find_map(|att| find_element(&att["blocks"], block_id, action_id))
        })
        .cloned()
}

/// `emu.blocks.action`: someone used an interactive element in a message or view.
pub async fn block_action(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?;
    let container = args::json(args, "container")?.unwrap_or_default();
    let block_id = req_str(args, "block_id")?;
    let action_id = req_str(args, "action_id")?;
    let picked = args::json(args, "value")?.unwrap_or_else(|| json!({}));
    let values = values_arg(args)?;

    let mut payload = json!({
        "type": "block_actions",
        "user": user_obj(me),
        "team": team_obj(&caller.team),
        "enterprise": null,
        "is_enterprise_install": false,
    });
    let (app, el) = match container["type"].as_str() {
        Some("message") => {
            let channel = container["channel_id"].as_str().unwrap_or_default();
            let ts = container["message_ts"].as_str().unwrap_or_default();
            let conv = find_conversation(state, caller, channel).await?;
            let ephemeral = container["is_ephemeral"].as_bool().unwrap_or(false);
            let (message, app_id) = if ephemeral {
                match state.interactions.ephemeral(&conv.id, ts) {
                    Some(e) if e.user_id == me.id => (e.message, e.app_id),
                    _ => return fail("message_not_found"),
                }
            } else {
                if state.store.member_last_read(&conv.id, &me.id).await?.is_none()
                    && (conv.is_private || !conv.is_channel())
                {
                    return fail("channel_not_found");
                }
                let Some(msg) = state.store.get_message(&conv.id, ts).await? else {
                    return fail("message_not_found");
                };
                (shapes::one_message(state, &msg).await?, msg.app_id)
            };
            let el = find_in_message(&message, &block_id, &action_id)
                .ok_or_else(|| ApiError::from(slack_emu_core::SlackError::new("action_not_found")))?;
            let app = interactive_app(state, app_id.as_deref()).await?;
            let target = ResponseTarget {
                team_id: caller.team.id.clone(),
                app_id: app.id.clone(),
                user_id: me.id.clone(),
                channel_id: Some(conv.id.clone()),
                source: Some(Source {
                    ts: ts.to_owned(),
                    ephemeral,
                }),
            };
            payload["container"] = json!({
                "type": "message",
                "message_ts": ts,
                "channel_id": conv.id,
                "is_ephemeral": ephemeral,
            });
            payload["channel"] = json!({"id": conv.id, "name": channel_name(&conv)});
            if !ephemeral {
                payload["message"] = message;
            }
            payload["state"] = json!({ "values": values });
            payload["response_url"] = json!(interactions::response_url(state, "actions", target));
            (app, el)
        }
        Some("view") => {
            let view_id = container["view_id"].as_str().unwrap_or_default();
            let Some(mut view) = interactions::find_view(state, view_id)
                .await?
                .filter(|v| v.user_id == me.id)
            else {
                return fail("not_found");
            };
            let el = find_element(&view.definition["blocks"], &block_id, &action_id)
                .cloned()
                .ok_or_else(|| ApiError::from(slack_emu_core::SlackError::new("action_not_found")))?;
            let app = interactive_app(state, Some(&view.app_id)).await?;
            view.values = values;
            interactions::save_view(state, &view).await?;
            payload["container"] = json!({"type": "view", "view_id": view.id});
            payload["view"] = view.to_json();
            (app, el)
        }
        _ => return Err(ApiError::invalid_arguments("container must be a message or a view")),
    };
    payload["api_app_id"] = json!(app.id);
    payload["token"] = json!(app.verification_token);
    payload["trigger_id"] = json!(interactions::trigger(state, &caller.team.id, &me.id, &app.id));
    payload["actions"] = json!([action(state, &el, &block_id, &picked)]);

    let url = app.manifest().interactivity_url().map(str::to_owned);
    state
        .delivery
        .interact(&app, "interactive", "block_actions".into(), payload, url)
        .await
        .map_err(dispatch_failed)?;
    Ok(json!({"ok": true}))
}

fn interaction_base(caller: &Caller, me: &User, app: &App, kind: &str) -> Value {
    json!({
        "type": kind,
        "team": team_obj(&caller.team),
        "user": user_obj(me),
        "api_app_id": app.id,
        "token": app.verification_token,
        "enterprise": null,
        "is_enterprise_install": false,
    })
}

/// `emu.views.submit`: the user submitted the top modal. Returns `errors` if
/// the app rejected the input; otherwise the app's `response_action` is applied.
pub async fn view_submit(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?;
    let view_id = req_str(args, "view_id")?;
    let values = values_arg(args)?;
    let Some(mut view) = state.interactions.modal_stack(&me.id).pop().filter(|v| v.id == view_id) else {
        return fail("not_found");
    };
    let app = interactive_app(state, Some(&view.app_id)).await?;
    view.values = values;
    if let Some(problems) = check_view_values(&view.definition["blocks"], &view.values) {
        return Ok(json!({"ok": true, "errors": problems}));
    }
    state.interactions.with_stack(&me.id, |stack| {
        if let Some(top) = stack.last_mut().filter(|v| v.id == view.id) {
            top.values = view.values.clone();
        }
    });

    let mut payload = interaction_base(caller, me, &app, "view_submission");
    payload["trigger_id"] = json!(interactions::trigger(state, &caller.team.id, &me.id, &app.id));
    payload["view"] = view.to_json();
    payload["response_urls"] = json!([]);
    let url = app.manifest().interactivity_url().map(str::to_owned);
    let resp = state
        .delivery
        .interact(&app, "interactive", "view_submission".into(), payload, url)
        .await
        .map_err(dispatch_failed)?
        .unwrap_or(Value::Null);

    let action = resp["response_action"].as_str().unwrap_or_default();
    let next = match action {
        "errors" => return Ok(json!({"ok": true, "errors": resp["errors"]})),
        "update" | "push" => Some(check_view(resp["view"].clone(), Some("modal"))?),
        _ => None,
    };
    state.interactions.with_stack(&me.id, |stack| {
        match (action, next) {
            ("update", Some(def)) => {
                if let Some(top) = stack.last_mut().filter(|v| v.id == view.id) {
                    top.replace(def);
                }
            }
            ("push", Some(def)) => {
                if stack.len() >= MAX_MODAL_STACK {
                    return fail("push_limit_reached");
                }
                let below = stack.last().cloned();
                stack.push(View::new(&app, &me.id, def, below.as_ref()));
            }
            ("clear", _) => stack.clear(),
            // No response action: this view closes.
            _ => {
                if stack.last().is_some_and(|v| v.id == view.id) {
                    stack.pop();
                }
            }
        }
        Ok(())
    })?;
    interactions::publish_modals(state, &caller.team.id, &me.id);
    Ok(json!({"ok": true}))
}

/// `emu.views.close`: the user closed a view (`all`: the whole stack).
/// Apps that set `notify_on_close` get `view_closed`.
pub async fn view_close(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?;
    let view_id = req_str(args, "view_id")?;
    let all = bool_arg(args, "all", false);
    let (closed, cleared) = state.interactions.with_stack(&me.id, |stack| {
        let Some(pos) = stack.iter().position(|v| v.id == view_id) else {
            return (Vec::new(), false);
        };
        if all || stack[pos].definition["clear_on_close"] == true {
            let many = stack.len() > 1;
            (std::mem::take(stack), many)
        } else {
            (stack.split_off(pos), false)
        }
    });
    let Some(top) = closed.last() else {
        return fail("not_found");
    };
    interactions::publish_modals(state, &caller.team.id, &me.id);
    if top.definition["notify_on_close"] == true
        && let Ok(app) = interactive_app(state, Some(&top.app_id)).await
    {
        let mut payload = interaction_base(caller, me, &app, "view_closed");
        payload["view"] = top.to_json();
        payload["is_cleared"] = json!(cleared);
        let url = app.manifest().interactivity_url().map(str::to_owned);
        let state = state.clone();
        tokio::spawn(async move {
            let _ = state
                .delivery
                .interact(&app, "interactive", "view_closed".into(), payload, url)
                .await;
        });
    }
    Ok(json!({"ok": true}))
}

/// `emu.views.current`: the user's open modal stack, bottom first.
pub async fn views_current(state: &AppState, caller: &Caller, _args: &Args) -> ApiResult {
    let me = caller.user()?;
    let views: Vec<Value> = state
        .interactions
        .modal_stack(&me.id)
        .iter()
        .map(View::to_json)
        .collect();
    Ok(json!({"ok": true, "views": views}))
}

/// Installed apps whose bot was granted `commands`, with their slash commands.
async fn command_apps(state: &AppState, team_id: &str) -> ApiResult<Vec<App>> {
    let mut out = Vec::new();
    for app in state.store.list_apps(team_id).await? {
        if !app.is_installed() || app.manifest().features.slash_commands.is_empty() {
            continue;
        }
        let granted = state
            .store
            .app_tokens(&app.id)
            .await?
            .iter()
            .any(|t| !t.revoked && t.bot_id.is_some() && t.has_scope("commands"));
        if granted {
            out.push(app);
        }
    }
    Ok(out)
}

fn command_json(app: &App, c: &SlashCommand) -> Value {
    json!({
        "command": c.command,
        "description": c.description,
        "usage_hint": c.usage_hint,
        "app_id": app.id,
        "app_name": app.name,
    })
}

/// `emu.commands.list`: slash commands available in the workspace.
pub async fn commands_list(state: &AppState, caller: &Caller, _args: &Args) -> ApiResult {
    let mut commands = Vec::new();
    for app in command_apps(state, &caller.team.id).await? {
        for c in &app.manifest().features.slash_commands {
            commands.push(command_json(&app, c));
        }
    }
    Ok(json!({"ok": true, "commands": commands}))
}

/// `chat.command`: runs a slash command in a conversation.
pub async fn command(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?;
    let a = visible(state, caller, args, "channel", None).await?;
    a.require_member()?;
    let cmd = req_str(args, "command")?.to_lowercase();
    let raw_text = opt_str(args, "text").unwrap_or_default();

    let found = command_apps(state, &caller.team.id).await?.into_iter().find_map(|app| {
        let spec = app.manifest().slash_command(&cmd).cloned()?;
        Some((app, spec))
    });
    let Some((app, spec)) = found else {
        let text = format!(
            "`{cmd}` is not a valid command. In Slack, all messages that start with the \"/\" character are \
             interpreted as commands.\n\nIf you are trying to send a message and not run a command, try preceding \
             the \"/\" with an empty space."
        );
        interactions::slackbot_says(state, &a.conv, &me.id, &text).await?;
        return Ok(json!({"ok": true, "valid": false}));
    };
    let text = if spec.should_escape {
        link_names(state, &caller.team.id, &raw_text, true).await?
    } else {
        raw_text.clone()
    };
    let target = ResponseTarget {
        team_id: caller.team.id.clone(),
        app_id: app.id.clone(),
        user_id: me.id.clone(),
        channel_id: Some(a.conv.id.clone()),
        source: None,
    };
    let payload = json!({
        "token": app.verification_token,
        "team_id": caller.team.id,
        "team_domain": caller.team.domain,
        "channel_id": a.conv.id,
        "channel_name": channel_name(&a.conv),
        "user_id": me.id,
        "user_name": me.name,
        "command": cmd,
        "text": text,
        "api_app_id": app.id,
        "is_enterprise_install": "false",
        "response_url": interactions::response_url(state, "commands", target.clone()),
        "trigger_id": interactions::trigger(state, &caller.team.id, &me.id, &app.id),
    });
    let result = state
        .delivery
        .interact(&app, "slash_commands", cmd.clone(), payload, spec.url.clone())
        .await;
    match result {
        Err(e) => {
            let code = if e.contains("timeout") {
                "operation_timeout"
            } else {
                "dispatch_failed"
            };
            let text = format!("`{cmd}` failed with the error \"{code}\"");
            interactions::slackbot_says(state, &a.conv, &me.id, &text).await?;
        }
        Ok(Some(resp)) => {
            if let Some(body) = response_args(&resp) {
                // An in-channel reply shares the command itself too.
                if body.get("response_type").and_then(Value::as_str) == Some("in_channel") {
                    let echo = format!("{cmd} {raw_text}").trim_end().to_owned();
                    let msg = super::chat::build_message(state, &a.conv, Some(me.id.clone()), None, None, echo);
                    interactions::post_as(state, &a.conv, msg).await?;
                }
                if let Err(e) = interactions::respond(state, &target, &body).await {
                    let text = format!("`{cmd}` failed with the error \"{}\"", e.code);
                    interactions::slackbot_says(state, &a.conv, &me.id, &text).await?;
                }
            }
        }
        Ok(None) => {}
    }
    Ok(json!({"ok": true, "valid": true}))
}

/// `emu.apps.home`: the user opened an app's Home, Messages or About tab (in
/// their DM with its bot). Sends `app_home_opened` and returns the Home view.
pub async fn app_home(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let me = caller.user()?;
    let a = visible(state, caller, args, "channel", None).await?;
    if !a.conv.is_im {
        return fail("channel_not_found");
    }
    let peer = shapes::im_peer(&a.conv, Some(&me.id));
    let app = state
        .store
        .list_apps(&caller.team.id)
        .await?
        .into_iter()
        .find(|app| app.is_installed() && app.bot_user_id.is_some() && app.bot_user_id == peer);
    let Some(app) = app else {
        return fail("app_not_found");
    };
    let manifest = app.manifest();
    let view = state
        .store
        .home_view(&app.id, &me.id)
        .await?
        .and_then(|d| serde_json::from_value::<View>(d).ok())
        .map(|v| v.to_json());
    if let Some(tab) = opt_str(args, "tab").filter(|t| t == "home" || t == "messages")
        && manifest.bot_events().iter().any(|e| e == "app_home_opened")
    {
        let mut ev = json!({
            "type": "app_home_opened",
            "user": me.id,
            "channel": a.conv.id,
            "tab": tab,
            "event_ts": state.clock.next(),
        });
        if tab == "home"
            && let Some(v) = &view
        {
            ev["view"] = v.clone();
        }
        state.delivery.enqueue_event(state, &app.id, event_callback(&app, ev));
    }
    let commands: Vec<Value> = manifest
        .features
        .slash_commands
        .iter()
        .map(|c| command_json(&app, c))
        .collect();
    Ok(json!({
        "ok": true,
        "app": {
            "id": app.id,
            "name": app.name,
            "description": manifest.display_information.description,
            "bot_user_id": app.bot_user_id,
            "home_tab_enabled": manifest.home_tab_enabled(),
            "messages_tab_enabled": manifest.messages_tab_enabled(),
            "messages_tab_read_only": manifest.messages_tab_read_only(),
            "interactivity": manifest.interactivity_enabled(),
            "commands": commands,
        },
        "view": view,
    }))
}
