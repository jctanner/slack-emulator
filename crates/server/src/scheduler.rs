//! Posts scheduled messages when they're due, checking once a second. Each is
//! posted with `chat.postMessage` as whoever scheduled it, so a token revoked
//! or a channel left since then makes it fail, as on Slack.

use std::time::Duration;

use serde_json::{Value, json};
use slack_emu_core::models::ScheduledMessage;
use slack_emu_core::now;

use crate::AppState;
use crate::api::{self, ApiResult, Args, Caller};

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        loop {
            tick.tick().await;
            if let Err(e) = post_due(&state).await {
                tracing::warn!("posting scheduled messages failed: {}", e.code);
            }
        }
    });
}

/// Who a scheduled message posts as, if they still can.
async fn caller_for(state: &AppState, m: &ScheduledMessage) -> ApiResult<Option<Caller>> {
    let Some(team) = state.store.get_workspace(&m.team_id).await? else {
        return Ok(None);
    };
    let token = match &m.token {
        Some(t) => match state.store.get_token(t).await? {
            Some(t) if !t.revoked => Some(t),
            _ => return Ok(None),
        },
        None => None,
    };
    let user = state.store.get_user(&m.user_id).await?.filter(|u| !u.deleted);
    Ok(user.map(|user| Caller {
        team,
        user: Some(user),
        token,
    }))
}

pub async fn post_due(state: &AppState) -> ApiResult<()> {
    for m in state.store.due_scheduled(now()).await? {
        // Deleting first claims it, so it's posted at most once.
        if !state.store.delete_scheduled(&m.id).await? {
            continue;
        }
        let args: Args = match &m.args {
            Value::Object(a) => a.clone(),
            _ => Args::new(),
        };
        let result = match caller_for(state, &m).await? {
            Some(caller) => api::invoke(state, "chat.postMessage", caller, args.clone())
                .await
                .map(|_| ())
                .map_err(|e| e.code),
            None => Err("token_revoked".into()),
        };
        if let Err(code) = &result {
            tracing::info!(id = %m.id, "scheduled message not posted: {code}");
        }
        // Apps can see what happened in their delivery log.
        if let Some(app) = &m.app_id {
            let entry = json!({"scheduled_message_id": m.id, "post_at": m.post_at, "args": args});
            state.delivery.log_inbound(
                app,
                "scheduled_message",
                &entry,
                result.as_ref().err().map(String::as_str),
            );
        }
    }
    Ok(())
}
