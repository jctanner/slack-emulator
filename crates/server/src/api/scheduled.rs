//! Scheduled messages: `chat.scheduleMessage`, `chat.scheduledMessages.list`
//! and `chat.deleteScheduledMessage`. [`crate::scheduler`] posts them when due.

use serde_json::{Value, json};
use slack_emu_core::ids::{IdKind, new_id};
use slack_emu_core::models::ScheduledMessage;
use slack_emu_core::now;

use super::args::{self, opt_str, req_str};
use super::chat::{self, require_can_post};
use super::conversations::{fail, visible};
use super::{ApiError, ApiResult, Args, Caller};
use crate::AppState;

/// Slack accepts times up to 120 days ahead.
const MAX_AHEAD_SECS: i64 = 120 * 24 * 3600;

/// Whether the caller scheduled `m`: the same app for API tokens, the same person otherwise.
fn owns(caller: &Caller, m: &ScheduledMessage) -> bool {
    match caller.token.as_ref().and_then(|t| t.app_id.as_deref()) {
        Some(app) => m.app_id.as_deref() == Some(app),
        None => m.app_id.is_none() && caller.user.as_ref().is_some_and(|u| u.id == m.user_id),
    }
}

pub async fn schedule(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let me = caller.user()?;
    let access = visible(state, caller, a, "channel", None).await?;
    require_can_post(caller, &access)?;
    let post_at: i64 = req_str(a, "post_at")?
        .parse::<f64>()
        .map_err(|_| ApiError::invalid_arguments("post_at must be a Unix timestamp"))? as i64;
    let now = now();
    if post_at <= now {
        return fail("time_in_past");
    }
    if post_at > now + MAX_AHEAD_SECS {
        return fail("time_too_far");
    }
    // Checked now as well as when posting, so mistakes show up here.
    let content = chat::content(a)?;
    if content.is_empty() {
        return fail("no_text");
    }
    if let Some(tts) = args::opt_ts(a, "thread_ts")?
        && state.store.get_message(&access.conv.id, &tts).await?.is_none()
    {
        return fail("thread_not_found");
    }
    let mut stored = a.clone();
    stored.remove("token");
    stored.remove("post_at");
    stored.insert("channel".into(), json!(access.conv.id));
    let token = caller.token.as_ref();
    let m = ScheduledMessage {
        id: format!("Q{}", &new_id(IdKind::Channel)[1..]),
        team_id: caller.team.id.clone(),
        channel_id: access.conv.id.clone(),
        user_id: me.id.clone(),
        token: token.map(|t| t.token.clone()),
        app_id: token.and_then(|t| t.app_id.clone()),
        post_at,
        created: now,
        args: Value::Object(stored),
    };
    state.store.add_scheduled(&m).await?;
    let mut message = json!({
        "type": "delayed_message",
        "text": content.text.unwrap_or_default(),
        "user": me.id,
        "team": caller.team.id,
    });
    if let Some(b) = token.and_then(|t| t.bot_id.as_ref()) {
        message["bot_id"] = json!(b);
    }
    if let Some(b) = content.blocks {
        message["blocks"] = b;
    }
    Ok(json!({
        "ok": true,
        "channel": access.conv.id,
        "scheduled_message_id": m.id,
        "post_at": post_at,
        "message": message,
    }))
}

pub async fn list(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let channel = opt_str(a, "channel");
    let oldest = opt_str(a, "oldest").and_then(|s| s.parse::<f64>().ok());
    let latest = opt_str(a, "latest").and_then(|s| s.parse::<f64>().ok());
    let limit = args::limit(a, 100, 100) as usize;
    let items: Vec<Value> = state
        .store
        .list_scheduled(&caller.team.id)
        .await?
        .into_iter()
        .filter(|m| owns(caller, m))
        .filter(|m| channel.as_ref().is_none_or(|c| *c == m.channel_id))
        .filter(|m| oldest.is_none_or(|o| m.post_at as f64 >= o) && latest.is_none_or(|l| m.post_at as f64 <= l))
        .take(limit)
        .map(|m| {
            json!({
                "id": m.id,
                "channel_id": m.channel_id,
                "post_at": m.post_at,
                "date_created": m.created,
                "text": m.args["text"].as_str().unwrap_or_default(),
            })
        })
        .collect();
    Ok(json!({"ok": true, "scheduled_messages": items, "response_metadata": {"next_cursor": ""}}))
}

pub async fn delete(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    let access = visible(state, caller, a, "channel", None).await?;
    let id = req_str(a, "scheduled_message_id")?;
    match state.store.get_scheduled(&id).await? {
        Some(m) if m.channel_id == access.conv.id && owns(caller, &m) => {
            state.store.delete_scheduled(&id).await?;
            Ok(json!({"ok": true}))
        }
        _ => fail("invalid_scheduled_message_id"),
    }
}
