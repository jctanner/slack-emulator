//! The `/ws/client` event stream, over a real socket.

mod common;

use std::time::Duration;

use common::Harness;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use slack_emu_server::router;
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(h: &Harness) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(h.state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr.to_string()
}

async fn connect(addr: &str, cookie: &str, team: &str, origin: Option<&str>) -> Result<Ws, String> {
    let mut req = format!("ws://{addr}/ws/client?team={team}")
        .into_client_request()
        .unwrap();
    req.headers_mut().insert("cookie", cookie.parse().unwrap());
    if let Some(o) = origin {
        req.headers_mut().insert("origin", o.parse().unwrap());
    }
    tokio_tungstenite::connect_async(req)
        .await
        .map(|(ws, _)| ws)
        .map_err(|e| e.to_string())
}

/// Next event whose `type` (and `subtype`, if given) matches, skipping others.
async fn next_event(ws: &mut Ws, ty: &str, subtype: Option<&str>) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let Some(Ok(Message::Text(t))) = ws.next().await else {
                panic!("socket closed");
            };
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == ty && subtype.is_none_or(|s| v["subtype"] == s) {
                return v;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {ty}"))
}

/// Asserts no event of this type arrives within a short window.
async fn assert_quiet(ws: &mut Ws, ty: &str) {
    let r = tokio::time::timeout(Duration::from_millis(300), next_event(ws, ty, None)).await;
    assert!(r.is_err(), "unexpected {ty}: {r:?}");
}

#[tokio::test]
async fn ui_stream_delivers_member_events() {
    let h = Harness::new().await;
    let addr = serve(&h).await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;
    let team = h.ok(&alice, "auth.test", json!({})).await["team_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice_id = h.user_id(&alice).await;

    // Unauthenticated and cross-origin connections are refused.
    assert!(connect(&addr, "slack_emu_session=nope", &team, None).await.is_err());
    assert!(
        connect(&addr, &alice, &team, Some("https://evil.example"))
            .await
            .is_err()
    );

    let mut a = connect(&addr, &alice, &team, Some(&format!("http://{addr}")))
        .await
        .unwrap();
    let mut b = connect(&addr, &bob, &team, None).await.unwrap();
    next_event(&mut a, "hello", None).await;
    next_event(&mut b, "hello", None).await;

    // A private channel's messages reach members only.
    let secret = h
        .ok(
            &alice,
            "conversations.create",
            json!({"name": "secret", "is_private": true}),
        )
        .await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.ok(&alice, "chat.postMessage", json!({"channel": secret, "text": "shh"}))
        .await;
    let ev = next_event(&mut a, "message", None).await;
    assert_eq!(ev["text"], "shh");
    assert_eq!(ev["channel"], secret.as_str());
    assert_eq!(ev["channel_type"], "group");
    assert_quiet(&mut b, "message").await;

    // Public channel creation is announced to everyone.
    h.ok(&alice, "conversations.create", json!({"name": "dev"})).await;
    let ev = next_event(&mut b, "channel_created", None).await;
    assert_eq!(ev["channel"]["name"], "dev");

    // Invites, edits, reactions and typing.
    let bob_id = h.user_id(&bob).await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": secret, "users": bob_id}),
    )
    .await;
    let ev = next_event(&mut b, "group_joined", None).await;
    assert_eq!(ev["channel"]["id"], secret.as_str());
    let ts = h
        .ok(&bob, "chat.postMessage", json!({"channel": secret, "text": "hi"}))
        .await["ts"]
        .as_str()
        .unwrap()
        .to_owned();
    h.ok(&bob, "chat.update", json!({"channel": secret, "ts": ts, "text": "hi!"}))
        .await;
    let ev = next_event(&mut a, "message", Some("message_changed")).await;
    assert_eq!(ev["message"]["text"], "hi!");
    assert_eq!(ev["previous_message"]["text"], "hi");

    h.ok(
        &alice,
        "reactions.add",
        json!({"channel": secret, "timestamp": ts, "name": "eyes"}),
    )
    .await;
    let ev = next_event(&mut b, "reaction_added", None).await;
    assert_eq!(ev["item_user"], bob_id.as_str());

    a.send(Message::text(json!({"type": "typing", "channel": secret}).to_string()))
        .await
        .unwrap();
    let ev = next_event(&mut b, "user_typing", None).await;
    assert_eq!(ev["user"], alice_id.as_str());

    // Replies refresh the parent's thread summary.
    h.ok(
        &alice,
        "chat.postMessage",
        json!({"channel": secret, "text": "r", "thread_ts": ts}),
    )
    .await;
    let ev = next_event(&mut b, "emu_thread_updated", None).await;
    assert_eq!(ev["message"]["reply_count"], 1);

    h.ok(&bob, "chat.delete", json!({"channel": secret, "ts": ts})).await;
    let ev = next_event(&mut a, "message", Some("message_changed")).await;
    assert_eq!(ev["message"]["subtype"], "tombstone");
}
