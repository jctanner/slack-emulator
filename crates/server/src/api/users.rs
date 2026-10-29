//! `users.*`, `team.*` and the client-internal `client.counts`.

use std::collections::HashMap;

use serde_json::{Value, json};
use slack_emu_core::models::App;

use super::args::{self, opt_str};
use super::conversations::{fail, list_page, team_user};
use super::{ApiResult, Args, Caller, shapes};
use crate::AppState;

/// The workspace's apps keyed by their bot user.
async fn apps_by_bot_user(state: &AppState, team_id: &str) -> ApiResult<HashMap<String, App>> {
    Ok(state
        .store
        .list_apps(team_id)
        .await?
        .into_iter()
        .filter_map(|a| a.bot_user_id.clone().map(|u| (u, a)))
        .collect())
}

pub async fn list(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let limit = args::limit(args, 200, 1000);
    let after = args::cursor(args, "user")?;
    let mut users = state
        .store
        .list_users_page(&caller.team.id, after.as_deref(), limit + 1)
        .await?;
    let next = if users.len() as i64 > limit {
        users.truncate(limit as usize);
        users.last().map(|u| args::encode_cursor("user", &u.id))
    } else {
        None
    };
    let apps = apps_by_bot_user(state, &caller.team.id).await?;
    let members: Vec<Value> = users
        .iter()
        .map(|u| shapes::user_with_app(u, apps.get(&u.id)))
        .collect();
    Ok(json!({
        "ok": true,
        "members": members,
        "cache_ts": slack_emu_core::now(),
        "response_metadata": args::response_metadata(next),
    }))
}

pub async fn info(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let id = opt_str(args, "user").ok_or_else(|| slack_emu_core::SlackError::new("user_not_found"))?;
    match state.store.get_user(&id).await? {
        Some(u) if u.team_id == caller.team.id => {
            let apps = apps_by_bot_user(state, &caller.team.id).await?;
            Ok(json!({"ok": true, "user": shapes::user_with_app(&u, apps.get(&u.id))}))
        }
        _ => fail("user_not_found"),
    }
}

pub async fn profile_get(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let user = match opt_str(args, "user") {
        Some(id) => team_user(state, caller, &id).await?,
        None => caller.user()?.clone(),
    };
    Ok(json!({"ok": true, "profile": shapes::profile(&user)}))
}

pub async fn lookup_by_email(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let email = args::req_str(args, "email")?;
    match state.store.user_by_email(&caller.team.id, &email).await? {
        Some(u) => Ok(json!({"ok": true, "user": shapes::user(&u)})),
        None => fail("users_not_found"),
    }
}

pub async fn conversations(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let viewer = match opt_str(args, "user") {
        Some(id) => team_user(state, caller, &id).await?,
        None => caller.user()?.clone(),
    };
    list_page(state, caller, &viewer, args, true).await
}

pub async fn team_info(_state: &AppState, caller: &Caller, _args: &Args) -> ApiResult {
    let t = &caller.team;
    Ok(json!({
        "ok": true,
        "team": {
            "id": t.id,
            "name": t.name,
            "domain": t.domain,
            "email_domain": "",
            "icon": {"image_default": true},
        },
    }))
}

/// Unread state for every conversation the caller belongs to. Slack's web
/// client uses this (undocumented) method to draw the sidebar.
pub async fn client_counts(state: &AppState, caller: &Caller, _args: &Args) -> ApiResult {
    let me = caller.user()?;
    let (mut channels, mut mpims, mut ims) = (Vec::new(), Vec::new(), Vec::new());
    for c in state.store.counts_for_user(&me.id).await? {
        let v = json!({
            "id": c.conversation.id,
            "last_read": c.last_read,
            "latest": c.latest,
            "updated": c.latest,
            "has_unreads": c.unread_count > 0,
            "unread_count": c.unread_count,
            "mention_count": c.mention_count,
        });
        if c.conversation.is_im {
            ims.push(v);
        } else if c.conversation.is_mpim {
            mpims.push(v);
        } else {
            channels.push(v);
        }
    }
    Ok(json!({"ok": true, "channels": channels, "mpims": mpims, "ims": ims}))
}
