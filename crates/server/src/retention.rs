//! Message retention: deletes messages older than a workspace's default or a
//! conversation's own policy (`admin.conversations.setCustomRetention`).
//! Checked once a minute. Apps aren't told (Slack doesn't either); open web
//! clients reload the conversation.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::json;
use slack_emu_core::now;

use crate::AppState;
use crate::api::ApiResult;
use crate::events::publish_to_members;

const SWEEP_INTERVAL: Duration = Duration::from_secs(60);

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(SWEEP_INTERVAL);
        loop {
            tick.tick().await;
            if let Err(e) = sweep(&state, now()).await {
                tracing::warn!("retention sweep failed: {}", e.code);
            }
        }
    });
}

/// Applies every policy as of `now` (Unix seconds). Returns how many messages went.
pub async fn sweep(state: &AppState, now: i64) -> ApiResult<u64> {
    // team → (default days, conversation → days)
    let mut teams: HashMap<String, (Option<i64>, HashMap<String, i64>)> = HashMap::new();
    for (team, target, days) in state.store.retention_policies().await? {
        let entry = teams.entry(team.clone()).or_default();
        if target == team {
            entry.0 = Some(days);
        } else {
            entry.1.insert(target, days);
        }
    }
    let mut total = 0;
    for (team, (default, overrides)) in teams {
        let mut targets: Vec<(String, i64)> = overrides.clone().into_iter().collect();
        if let Some(days) = default {
            for id in state.store.conversation_ids(&team).await? {
                if !overrides.contains_key(&id) {
                    targets.push((id, days));
                }
            }
        }
        for (channel, days) in targets {
            let before = format!("{:010}.000000", now - days * 86_400);
            let deleted = state.store.prune_messages(&channel, &before).await?;
            if deleted == 0 {
                continue;
            }
            total += deleted;
            tracing::info!(%channel, deleted, days, "retention removed old messages");
            if let Some(conv) = state.store.get_conversation(&channel).await? {
                let ev = json!({"type": "emu_history_pruned", "channel": channel, "before": before});
                publish_to_members(state, &conv, &[], true, ev).await?;
            }
        }
    }
    Ok(total)
}
