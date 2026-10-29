//! In-process event bus. Services publish Slack-shaped events here; the UI
//! WebSocket (and, later, Socket Mode and the Events API dispatcher)
//! subscribe and filter by audience.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use slack_emu_core::models::Conversation;
use tokio::sync::broadcast;

use crate::AppState;
use crate::api::ApiResult;

#[derive(Debug, Clone)]
pub enum Audience {
    /// Everyone in the workspace.
    Team,
    /// Only these users (a snapshot of channel members at publish time).
    Users(Vec<String>),
}

#[derive(Debug, Clone)]
pub struct BusEvent {
    /// Position in the event log, increasing from 1.
    pub seq: u64,
    pub team_id: String,
    pub audience: Audience,
    /// For the web UI only; never delivered to apps (e.g. typing indicators).
    pub ui_only: bool,
    /// The event in Slack's format (the `event` object of an Events API callback).
    pub event: Value,
}

impl BusEvent {
    pub fn visible_to(&self, team_id: &str, user_id: &str) -> bool {
        self.team_id == team_id
            && match &self.audience {
                Audience::Team => true,
                Audience::Users(ids) => ids.iter().any(|id| id == user_id),
            }
    }
}

/// Recent events kept for the test harness (`/_emu/events`, `/_emu/wait`).
const LOG_SIZE: usize = 5000;

struct Log {
    next_seq: u64,
    recent: VecDeque<Arc<BusEvent>>,
}

#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<BusEvent>>,
    log: Arc<Mutex<Log>>,
}

impl Default for EventBus {
    fn default() -> Self {
        Self {
            tx: broadcast::channel(4096).0,
            log: Arc::new(Mutex::new(Log {
                next_seq: 1,
                recent: VecDeque::new(),
            })),
        }
    }
}

impl EventBus {
    pub fn publish(&self, team_id: String, audience: Audience, ui_only: bool, event: Value) {
        let mut log = self.log.lock().unwrap();
        let ev = Arc::new(BusEvent {
            seq: log.next_seq,
            team_id,
            audience,
            ui_only,
            event,
        });
        log.next_seq += 1;
        log.recent.push_back(ev.clone());
        if log.recent.len() > LOG_SIZE {
            log.recent.pop_front();
        }
        // Sent under the lock so subscribers see events in sequence order.
        // No subscribers is fine.
        let _ = self.tx.send(ev);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<BusEvent>> {
        self.tx.subscribe()
    }

    /// The seq of the latest event (0 if none yet).
    pub fn last_seq(&self) -> u64 {
        self.log.lock().unwrap().next_seq - 1
    }

    /// Logged events after `after`, oldest first.
    pub fn since(&self, after: u64) -> Vec<Arc<BusEvent>> {
        self.log
            .lock()
            .unwrap()
            .recent
            .iter()
            .filter(|e| e.seq > after)
            .cloned()
            .collect()
    }

    /// Forgets logged events (`/_emu/reset`); sequence numbers keep increasing.
    pub fn clear(&self) {
        self.log.lock().unwrap().recent.clear();
    }
}

/// Stamps `event_ts` (if missing) and publishes to the given audience.
pub fn publish(state: &AppState, team_id: &str, audience: Audience, ui_only: bool, mut event: Value) {
    if event.get("event_ts").is_none() {
        event["event_ts"] = json!(state.clock.next());
    }
    state.bus.publish(team_id.to_owned(), audience, ui_only, event);
}

/// Publishes to the conversation's current members, plus any `extra` users
/// (e.g. someone who just left and should still hear about it).
pub async fn publish_to_members(
    state: &AppState,
    conv: &Conversation,
    extra: &[&str],
    ui_only: bool,
    event: Value,
) -> ApiResult<()> {
    let mut users = state.store.list_members(&conv.id, None, i64::MAX).await?;
    for u in extra {
        if !users.iter().any(|x| x == u) {
            users.push((*u).to_owned());
        }
    }
    publish(state, &conv.team_id, Audience::Users(users), ui_only, event);
    Ok(())
}

/// After the peer sends a close frame, keep reading briefly so the WebSocket
/// library flushes its close reply; otherwise the client sees an abnormal
/// closure (1006) and may wait for a handshake that never comes.
pub async fn finish_close(socket: &mut axum::extract::ws::WebSocket) {
    let _ = tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while let Some(Ok(_)) = socket.recv().await {}
    })
    .await;
}
