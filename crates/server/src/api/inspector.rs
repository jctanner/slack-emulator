//! `emu.inspector.*`: the web UI's Inspector, a live view of one workspace's
//! Web API calls, app deliveries and events. For workspace owners and admins.

use serde_json::{Value, json};

use super::conversations::fail;
use super::{ApiResult, Args, Caller, args};
use crate::AppState;

const LIMIT: usize = 300;

fn require_admin(caller: &Caller) -> ApiResult<()> {
    let me = caller.user()?;
    if me.is_admin || me.is_owner {
        Ok(())
    } else {
        fail("not_allowed")
    }
}

fn after(a: &Args) -> u64 {
    args::opt_str(a, "after").and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// The newest `LIMIT` entries, oldest first.
fn tail(mut items: Vec<Value>) -> Vec<Value> {
    if items.len() > LIMIT {
        items.drain(..items.len() - LIMIT);
    }
    items
}

pub async fn api_log(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let with_ui = args::bool_arg(a, "include_session", false);
    let entries = state
        .api_log
        .since(after(a))
        .into_iter()
        .filter(|e| e["team_id"] == caller.team.id.as_str() && (with_ui || e["via"] != "session"))
        .collect();
    Ok(json!({"ok": true, "entries": tail(entries)}))
}

pub async fn deliveries(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let apps: Vec<String> = state
        .store
        .list_apps(&caller.team.id)
        .await?
        .into_iter()
        .map(|app| app.id)
        .collect();
    let entries = state.delivery.since(after(a), Some(&apps));
    Ok(json!({"ok": true, "entries": tail(entries)}))
}

pub async fn events(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let entries = state
        .bus
        .since(after(a))
        .iter()
        .filter(|e| e.team_id == caller.team.id && e.event["type"] != "user_typing")
        .map(|e| json!({"seq": e.seq, "ui_only": e.ui_only, "event": e.event}))
        .collect();
    Ok(json!({"ok": true, "entries": tail(entries), "rate_limits": state.rate_limits.status()}))
}
