//! Retention settings: Slack's `admin.conversations.*CustomRetention` for one
//! conversation, and `emu.retention.*` for the workspace default (Slack sets
//! that in its admin UI). Only workspace admins and owners may change them.

use serde_json::{Value, json};

use super::args::{opt_str, req_str};
use super::conversations::{fail, find_conversation};
use super::{ApiError, ApiResult, Args, Caller};
use crate::AppState;

fn require_admin(caller: &Caller) -> ApiResult<()> {
    let me = caller.user()?;
    if me.is_admin || me.is_owner {
        Ok(())
    } else {
        fail("not_an_admin")
    }
}

/// `duration_days` / `days`: a whole number of days, at least 1.
fn days_arg(a: &Args, key: &str) -> ApiResult<i64> {
    req_str(a, key)?
        .parse::<i64>()
        .ok()
        .filter(|d| (1..=36_500).contains(d))
        .ok_or_else(|| ApiError::invalid_arguments(format!("{key} must be a whole number of days, at least 1")))
}

async fn policy_for(state: &AppState, team_id: &str, target: &str) -> ApiResult<Option<i64>> {
    Ok(state
        .store
        .retention_policies()
        .await?
        .into_iter()
        .find(|(team, t, _)| team == team_id && t == target)
        .map(|(_, _, days)| days))
}

pub async fn set_custom(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let conv = find_conversation(state, caller, &req_str(a, "channel_id")?).await?;
    let days = days_arg(a, "duration_days")?;
    state.store.set_retention(&caller.team.id, &conv.id, Some(days)).await?;
    Ok(json!({"ok": true}))
}

pub async fn get_custom(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let conv = find_conversation(state, caller, &req_str(a, "channel_id")?).await?;
    let days = policy_for(state, &caller.team.id, &conv.id).await?;
    Ok(json!({"ok": true, "is_policy_enabled": days.is_some(), "duration_days": days.unwrap_or(0)}))
}

pub async fn remove_custom(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let conv = find_conversation(state, caller, &req_str(a, "channel_id")?).await?;
    state.store.set_retention(&caller.team.id, &conv.id, None).await?;
    Ok(json!({"ok": true}))
}

/// `emu.retention.get`: the workspace default and every conversation override.
pub async fn get(state: &AppState, caller: &Caller, _a: &Args) -> ApiResult {
    let team = &caller.team.id;
    let mut default = Value::Null;
    let mut channels = Vec::new();
    for (t, target, days) in state.store.retention_policies().await? {
        if t != *team {
            continue;
        }
        if target == *team {
            default = json!(days);
        } else {
            channels.push(json!({"channel_id": target, "days": days}));
        }
    }
    Ok(json!({"ok": true, "workspace_days": default, "channels": channels}))
}

/// `emu.retention.set`: the workspace default; omit `days` (or send 0) to keep messages forever.
pub async fn set(state: &AppState, caller: &Caller, a: &Args) -> ApiResult {
    require_admin(caller)?;
    let days = match opt_str(a, "days").as_deref() {
        None | Some("0") => None,
        Some(_) => Some(days_arg(a, "days")?),
    };
    state
        .store
        .set_retention(&caller.team.id, &caller.team.id, days)
        .await?;
    Ok(json!({"ok": true, "workspace_days": days}))
}
