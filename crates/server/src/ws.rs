//! `/ws/client`: the web UI's real-time stream. Authenticated by session
//! cookie; delivers every bus event visible to the user in the chosen
//! workspace, and accepts typing notifications.

use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum_extra::extract::CookieJar;
use serde::Deserialize;
use serde_json::{Value, json};
use slack_emu_core::models::User;
use tokio::sync::broadcast::error::RecvError;

use crate::AppState;
use crate::auth;
use crate::events::publish_to_members;

pub fn routes() -> Router<AppState> {
    Router::new().route("/client", get(client_ws))
}

#[derive(Deserialize)]
struct ClientQuery {
    team: String,
}

/// Rejects cross-site WebSocket connections: browsers send cookies on them
/// regardless of origin.
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // Non-browser client.
    };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or("");
    origin.split_once("://").map(|(_, h)| h) == Some(host)
}

async fn client_ws(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
    Query(q): Query<ClientQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    if !same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let user = match auth::session_account(&state, &jar).await {
        Ok(Some(account)) => match state.store.user_for_account(&q.team, &account.id).await {
            Ok(Some(u)) if !u.deleted => u,
            Ok(_) => return StatusCode::FORBIDDEN.into_response(),
            Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    ws.on_upgrade(move |socket| run(state, user, socket))
}

async fn send(socket: &mut WebSocket, v: &Value) -> bool {
    socket.send(WsMessage::Text(v.to_string().into())).await.is_ok()
}

async fn run(state: AppState, user: User, mut socket: WebSocket) {
    // Subscribe before saying hello so nothing published after hello is missed.
    let mut rx = state.bus.subscribe();
    if !send(&mut socket, &json!({"type": "hello"})).await {
        return;
    }
    loop {
        tokio::select! {
            ev = rx.recv() => match ev {
                Ok(ev) => {
                    if ev.visible_to(&user.team_id, &user.id) && !send(&mut socket, &ev.event).await {
                        break;
                    }
                }
                // We fell behind and dropped events; the client must refetch.
                Err(RecvError::Lagged(_)) => {
                    if !send(&mut socket, &json!({"type": "emu_resync"})).await {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            msg = socket.recv() => match msg {
                Some(Ok(WsMessage::Text(text))) => handle_client_message(&state, &user, &mut socket, &text).await,
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

async fn handle_client_message(state: &AppState, user: &User, socket: &mut WebSocket, text: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(text) else {
        return;
    };
    match msg["type"].as_str() {
        Some("ping") => {
            let mut pong = json!({"type": "pong"});
            if let Some(id) = msg.get("id") {
                pong["reply_to"] = id.clone();
            }
            send(socket, &pong).await;
        }
        Some("typing") => {
            let Some(channel) = msg["channel"].as_str() else { return };
            let Ok(Some(conv)) = state.store.get_conversation(channel).await else {
                return;
            };
            if conv.team_id != user.team_id
                || !matches!(state.store.member_last_read(&conv.id, &user.id).await, Ok(Some(_)))
            {
                return;
            }
            let mut ev = json!({"type": "user_typing", "channel": conv.id, "user": user.id});
            if let Some(t) = msg["thread_ts"].as_str() {
                ev["thread_ts"] = json!(t);
            }
            let _ = publish_to_members(state, &conv, &[], true, ev).await;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "emu.local:8080".parse().unwrap());
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, "http://emu.local:8080".parse().unwrap());
        assert!(same_origin(&h));
        h.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
        assert!(!same_origin(&h));
    }
}
