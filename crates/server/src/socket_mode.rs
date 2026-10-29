//! Socket Mode: apps open a WebSocket (URL from `apps.connections.open`) and
//! receive events, interactions and slash commands over it, acknowledging
//! each envelope by echoing its `envelope_id`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::{Value, json};
use slack_emu_core::ids::new_secret;
use tokio::sync::{mpsc, oneshot};

use crate::AppState;

/// Tickets from `apps.connections.open` must be used promptly.
const TICKET_TTL: Duration = Duration::from_secs(60);
/// Like Slack, ping clients regularly; SDKs reconnect if the server goes quiet.
const PING_INTERVAL: Duration = Duration::from_secs(10);

enum Outgoing {
    Text(String),
    Close,
}

struct Conn {
    id: u64,
    tx: mpsc::UnboundedSender<Outgoing>,
}

#[derive(Default)]
struct HubInner {
    tickets: HashMap<String, (String, Instant)>,
    conns: HashMap<String, Vec<Conn>>,
    /// Round-robin position per app.
    next: HashMap<String, usize>,
    /// envelope_id → waiter for the ack's (optional) payload.
    pending: HashMap<String, oneshot::Sender<Value>>,
}

#[derive(Default)]
pub struct SocketHub {
    inner: Mutex<HubInner>,
    ids: AtomicU64,
}

impl SocketHub {
    pub fn issue_ticket(&self, app_id: &str) -> String {
        let ticket = new_secret();
        let mut inner = self.inner.lock().unwrap();
        inner.tickets.retain(|_, (_, at)| at.elapsed() < TICKET_TTL);
        inner
            .tickets
            .insert(ticket.clone(), (app_id.to_owned(), Instant::now()));
        ticket
    }

    fn redeem(&self, ticket: &str) -> Option<String> {
        let mut inner = self.inner.lock().unwrap();
        match inner.tickets.remove(ticket) {
            Some((app, at)) if at.elapsed() < TICKET_TTL => Some(app),
            _ => None,
        }
    }

    fn register(&self, app_id: &str) -> (u64, mpsc::UnboundedReceiver<Outgoing>, usize) {
        let id = self.ids.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::unbounded_channel();
        let mut inner = self.inner.lock().unwrap();
        let list = inner.conns.entry(app_id.to_owned()).or_default();
        list.push(Conn { id, tx });
        let n = list.len();
        (id, rx, n)
    }

    fn unregister(&self, app_id: &str, id: u64) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(list) = inner.conns.get_mut(app_id) {
            list.retain(|c| c.id != id);
        }
    }

    pub fn connection_count(&self, app_id: &str) -> usize {
        self.inner.lock().unwrap().conns.get(app_id).map_or(0, Vec::len)
    }

    /// Sends an envelope over one of the app's connections (round robin).
    /// Returns a receiver for the ack, or `None` if the app isn't connected.
    pub fn send(&self, app_id: &str, envelope_id: &str, envelope: &Value) -> Option<oneshot::Receiver<Value>> {
        let mut inner = self.inner.lock().unwrap();
        let text = envelope.to_string();
        loop {
            let n = inner.conns.get(app_id).map_or(0, Vec::len);
            if n == 0 {
                return None;
            }
            let i = {
                let next = inner.next.entry(app_id.to_owned()).or_default();
                *next = (*next + 1) % n;
                *next
            };
            let list = inner.conns.get_mut(app_id).expect("checked");
            if list[i].tx.send(Outgoing::Text(text.clone())).is_ok() {
                let (tx, rx) = oneshot::channel();
                inner.pending.insert(envelope_id.to_owned(), tx);
                return Some(rx);
            }
            // That connection is gone; drop it and try another.
            list.remove(i);
        }
    }

    /// Stops waiting for an ack (after a timeout).
    pub fn forget(&self, envelope_id: &str) {
        self.inner.lock().unwrap().pending.remove(envelope_id);
    }

    fn ack(&self, envelope_id: &str, payload: Value) {
        if let Some(tx) = self.inner.lock().unwrap().pending.remove(envelope_id) {
            let _ = tx.send(payload);
        }
    }

    /// Tells every connection of the app to go away (uninstall, token revoked).
    pub fn disconnect_all(&self, app_id: &str, reason: &str) {
        let msg = json!({"type": "disconnect", "reason": reason, "debug_info": {"host": "slack-emu"}}).to_string();
        let inner = self.inner.lock().unwrap();
        for c in inner.conns.get(app_id).into_iter().flatten() {
            let _ = c.tx.send(Outgoing::Text(msg.clone()));
            let _ = c.tx.send(Outgoing::Close);
        }
    }
}

impl SocketHub {
    /// Disconnects every app and forgets pending tickets and acks.
    pub fn disconnect_everyone(&self) {
        let apps: Vec<String> = self.inner.lock().unwrap().conns.keys().cloned().collect();
        for app in apps {
            self.disconnect_all(&app, "refresh_requested");
        }
        let mut inner = self.inner.lock().unwrap();
        inner.tickets.clear();
        inner.pending.clear();
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/socket-mode", get(connect))
}

#[derive(Deserialize)]
struct ConnectQuery {
    ticket: String,
}

async fn connect(State(state): State<AppState>, Query(q): Query<ConnectQuery>, ws: WebSocketUpgrade) -> Response {
    let Some(app_id) = state.delivery.sockets.redeem(&q.ticket) else {
        return (StatusCode::UNAUTHORIZED, "invalid or expired ticket").into_response();
    };
    ws.on_upgrade(move |socket| run(state, app_id, socket))
}

async fn run(state: AppState, app_id: String, mut socket: WebSocket) {
    let hub = &state.delivery.sockets;
    let (conn_id, mut rx, n) = hub.register(&app_id);
    tracing::info!(app = %app_id, connections = n, "socket mode connected");
    let hello = json!({
        "type": "hello",
        "num_connections": n,
        "debug_info": {"host": "slack-emu", "build_number": 1, "approximate_connection_time": 18060},
        "connection_info": {"app_id": app_id},
    });
    if socket.send(WsMessage::Text(hello.to_string().into())).await.is_ok() {
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await;
        loop {
            tokio::select! {
                _ = ping.tick() => {
                    if socket.send(WsMessage::Ping(Default::default())).await.is_err() {
                        break;
                    }
                }
                out = rx.recv() => match out {
                    Some(Outgoing::Text(t)) => {
                        if socket.send(WsMessage::Text(t.into())).await.is_err() {
                            break;
                        }
                    }
                    Some(Outgoing::Close) | None => {
                        let _ = socket.send(WsMessage::Close(None)).await;
                        break;
                    }
                },
                msg = socket.recv() => match msg {
                    Some(Ok(WsMessage::Text(text))) => {
                        if let Ok(v) = serde_json::from_str::<Value>(&text)
                            && let Some(id) = v["envelope_id"].as_str()
                        {
                            hub.ack(id, v.get("payload").cloned().unwrap_or(Value::Null));
                        }
                    }
                    Some(Ok(WsMessage::Close(_))) => {
                        crate::events::finish_close(&mut socket).await;
                        break;
                    }
                    None | Some(Err(_)) => break,
                    Some(Ok(_)) => {}
                },
            }
        }
    }
    hub.unregister(&app_id, conn_id);
    tracing::info!(app = %app_id, "socket mode disconnected");
}
