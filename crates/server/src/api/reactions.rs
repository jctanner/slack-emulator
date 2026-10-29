//! `reactions.*` methods.

use serde_json::json;

use super::args::{self, opt_str};
use super::conversations::{fail, visible};
use super::{ApiResult, Args, Caller, chat, shapes};
use crate::AppState;
use crate::events::publish_to_members;

/// Emoji name without surrounding colons; allows skin-tone suffixes like `+1::skin-tone-2`.
fn emoji_name(args: &Args) -> ApiResult<String> {
    let raw = opt_str(args, "name").unwrap_or_default();
    let name = raw.trim().trim_matches(':').to_owned();
    let valid = !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '+' | '-' | '\'' | ':'));
    if valid { Ok(name) } else { fail("invalid_name") }
}

async fn change(state: &AppState, caller: &Caller, args: &Args, add: bool) -> ApiResult {
    let a = visible(state, caller, args, "channel", None).await?;
    a.require_read(caller)?;
    let ts = args::req_ts(args, "timestamp")?;
    let name = emoji_name(args)?;
    let Some(msg) = state.store.get_message(&a.conv.id, &ts).await? else {
        return fail("message_not_found");
    };
    let me = caller.user()?;
    if add {
        if !state.store.add_reaction(&a.conv.id, &ts, &name, &me.id).await? {
            return fail("already_reacted");
        }
    } else if !state.store.remove_reaction(&a.conv.id, &ts, &name, &me.id).await? {
        return fail("no_reaction");
    }
    let mut ev = json!({
        "type": if add { "reaction_added" } else { "reaction_removed" },
        "user": me.id,
        "reaction": name,
        "item": {"type": "message", "channel": a.conv.id, "ts": ts},
        "event_ts": state.clock.next(),
    });
    if let Some(u) = &msg.user_id {
        ev["item_user"] = json!(u);
    }
    publish_to_members(state, &a.conv, &[], false, ev).await?;
    Ok(json!({"ok": true}))
}

pub async fn add(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    change(state, caller, args, true).await
}

pub async fn remove(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    change(state, caller, args, false).await
}

pub async fn get(state: &AppState, caller: &Caller, args: &Args) -> ApiResult {
    let a = visible(state, caller, args, "channel", None).await?;
    a.require_read(caller)?;
    let ts = args::req_ts(args, "timestamp")?;
    let Some(msg) = state.store.get_message(&a.conv.id, &ts).await? else {
        return fail("message_not_found");
    };
    let mut rendered = shapes::one_message(state, &msg).await?;
    rendered["permalink"] = json!(chat::permalink(state, &a.conv, &msg));
    Ok(json!({"ok": true, "type": "message", "channel": a.conv.id, "message": rendered}))
}
