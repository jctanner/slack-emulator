//! Apps: manifests, installation, scopes, and event delivery over HTTP and Socket Mode.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use common::Harness;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use slack_emu_server::delivery::sign;
use slack_emu_server::router;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

fn manifest(name: &str, settings: Value) -> Value {
    json!({
        "display_information": {"name": name},
        "features": {"bot_user": {"display_name": name}},
        "oauth_config": {"scopes": {"bot": [
            "chat:write", "channels:history", "channels:read", "app_mentions:read", "reactions:read", "users:read"
        ]}},
        "settings": settings,
    })
}

/// Creates and installs an app as `who`; returns (app_id, bot token, bot user, signing secret).
async fn install(h: &Harness, who: &str, m: Value) -> (String, String, String, String) {
    let created = h
        .ok(who, "apps.manifest.create", json!({"manifest": m.to_string()}))
        .await;
    let app_id = created["app_id"].as_str().unwrap().to_owned();
    let secret = created["credentials"]["signing_secret"].as_str().unwrap().to_owned();
    let inst = h.ok(who, "emu.apps.install", json!({"app_id": app_id})).await;
    (
        app_id,
        inst["access_token"].as_str().unwrap().to_owned(),
        inst["bot_user_id"].as_str().unwrap().to_owned(),
        secret,
    )
}

async fn general(h: &Harness, who: &str) -> String {
    let r = h.ok(who, "users.conversations", json!({})).await;
    r["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "general")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test]
async fn manifests_install_and_scopes() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;

    let r = h
        .call(
            &alice,
            "apps.manifest.validate",
            json!({"manifest": {"oauth_config": {"scopes": {"bot": ["nope"]}}}}),
        )
        .await;
    assert_eq!(r["error"], "invalid_manifest");
    assert_eq!(r["errors"][0]["pointer"], "/display_information/name");

    let m = manifest("Echo Bot", json!({"socket_mode_enabled": true}));
    let (app_id, token, bot_user, _) = install(&h, &alice, m.clone()).await;
    assert!(token.starts_with("xoxb-"));

    let who = h.ok(&token, "auth.test", json!({})).await;
    assert_eq!(who["user_id"], bot_user.as_str());
    assert_eq!(who["user"], "echo-bot");
    assert_eq!(who["app_id"], app_id.as_str());
    let bot_id = who["bot_id"].as_str().unwrap().to_owned();

    // Bot user and bot profile look like Slack's.
    let u = h.ok(&token, "users.info", json!({"user": bot_user})).await;
    assert_eq!(u["user"]["is_bot"], true);
    assert_eq!(u["user"]["profile"]["bot_id"], bot_id.as_str());
    let b = h.ok(&token, "bots.info", json!({"bot": bot_id})).await;
    assert_eq!(b["bot"]["app_id"], app_id.as_str());

    // Scopes are enforced, with Slack's needed/provided detail.
    let r = h
        .call(
            &token,
            "reactions.add",
            json!({"channel": "x", "timestamp": "1", "name": "x"}),
        )
        .await;
    assert_eq!(r["error"], "missing_scope");
    assert_eq!(r["needed"], "reactions:write");
    assert!(r["provided"].as_str().unwrap().contains("chat:write"));
    let r = h
        .call(&token, "conversations.list", json!({"types": "private_channel"}))
        .await;
    assert_eq!(r["error"], "missing_scope");
    assert_eq!(r["needed"], "groups:read");
    h.ok(&token, "conversations.list", json!({})).await;

    // Without chat:write.public the bot has to be invited first.
    let general = general(&h, &alice).await;
    h.err(
        &token,
        "chat.postMessage",
        json!({"channel": general, "text": "hi"}),
        "not_in_channel",
    )
    .await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;
    let r = h
        .ok(
            &token,
            "chat.postMessage",
            json!({"channel": general, "text": "hi", "username": "ignored"}),
        )
        .await;
    assert_eq!(r["message"]["bot_profile"]["name"], "Echo Bot");
    assert!(r["message"].get("username").is_none(), "needs chat:write.customize");

    // Adding chat:write.public needs a reinstall to take effect.
    let mut m2 = m.clone();
    m2["oauth_config"]["scopes"]["bot"]
        .as_array_mut()
        .unwrap()
        .push(json!("chat:write.public"));
    let r = h
        .ok(
            &alice,
            "apps.manifest.update",
            json!({"app_id": app_id, "manifest": m2.to_string()}),
        )
        .await;
    assert_eq!(r["permissions_updated"], true);
    let info = h.ok(&alice, "emu.apps.info", json!({"app_id": app_id})).await;
    assert_eq!(info["app"]["needs_reinstall"], true);
    let random = h.ok(&alice, "users.conversations", json!({})).await["channels"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "random")
        .unwrap()["id"]
        .clone();
    h.err(
        &token,
        "chat.postMessage",
        json!({"channel": random, "text": "x"}),
        "not_in_channel",
    )
    .await;
    let reinstalled = h.ok(&alice, "emu.apps.install", json!({"app_id": app_id})).await;
    assert_eq!(reinstalled["access_token"], token.as_str(), "reinstall keeps the token");
    h.ok(&token, "chat.postMessage", json!({"channel": random, "text": "x"}))
        .await;

    // Only the creator (or an admin) manages an app; bots can't call emu.* at all.
    h.err(&bob, "emu.apps.info", json!({"app_id": app_id}), "not_allowed")
        .await;
    h.err(&token, "emu.apps.list", json!({}), "not_allowed_token_type")
        .await;

    // Uninstalling revokes the bot token and deactivates the bot user.
    h.ok(&alice, "emu.apps.uninstall", json!({"app_id": app_id})).await;
    h.err(&token, "auth.test", json!({}), "invalid_auth").await;
    let u = h.ok(&alice, "users.info", json!({"user": bot_user})).await;
    assert_eq!(u["user"]["deleted"], true);
    h.ok(&alice, "apps.manifest.delete", json!({"app_id": app_id})).await;
    h.err(&alice, "emu.apps.info", json!({"app_id": app_id}), "invalid_app_id")
        .await;
}

/// A fake app endpoint that records requests and fails the first `fail_first` event deliveries.
struct Receiver {
    url: String,
    rx: mpsc::UnboundedReceiver<(HeaderMap, Value, String)>,
    /// Deliveries skipped by `next` while looking for another event type.
    skipped: Vec<(HeaderMap, Value, String)>,
}

async fn receiver(fail_first: usize) -> Receiver {
    let (tx, rx) = mpsc::unbounded_channel();
    let failures = Arc::new(Mutex::new(fail_first));
    let app = Router::new().route(
        "/events",
        post(move |headers: HeaderMap, body: Bytes| {
            let tx = tx.clone();
            let failures = failures.clone();
            async move {
                let raw = String::from_utf8(body.to_vec()).unwrap();
                let v: Value = serde_json::from_str(&raw).unwrap();
                if v["type"] == "url_verification" {
                    return (StatusCode::OK, v["challenge"].as_str().unwrap().to_owned());
                }
                let _ = tx.send((headers, v, raw));
                let mut f = failures.lock().unwrap();
                if *f > 0 {
                    *f -= 1;
                    return (StatusCode::INTERNAL_SERVER_ERROR, String::new());
                }
                (StatusCode::OK, String::new())
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/events", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Receiver {
        url,
        rx,
        skipped: Vec::new(),
    }
}

impl Receiver {
    /// Next delivered event_callback, skipping events of other types.
    async fn next(&mut self, event_type: &str) -> (HeaderMap, Value, String) {
        if let Some(i) = self.skipped.iter().position(|d| d.1["event"]["type"] == event_type) {
            return self.skipped.remove(i);
        }
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let got = self.rx.recv().await.unwrap();
                if got.1["event"]["type"] == event_type {
                    return got;
                }
                self.skipped.push(got);
            }
        })
        .await
        .unwrap_or_else(|_| panic!("no {event_type} event delivered"))
    }
}

#[tokio::test]
async fn http_events_are_signed_filtered_and_retried() {
    let h = Harness::with_config(|c| c.event_retry_delays = vec![Duration::from_millis(50), Duration::from_millis(50)])
        .await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let general = general(&h, &alice).await;
    let mut recv = receiver(1).await;

    let m = manifest(
        "Hooky",
        json!({"event_subscriptions": {"request_url": recv.url, "bot_events": ["message.channels", "app_mention"]}}),
    );
    let (app_id, _token, bot_user, secret) = install(&h, &alice, m).await;
    let info = h.ok(&alice, "emu.apps.info", json!({"app_id": app_id})).await;
    assert_eq!(info["app"]["request_url_verified"], true);

    // Not in the channel yet: no message events.
    h.ok(
        &alice,
        "chat.postMessage",
        json!({"channel": general, "text": "before"}),
    )
    .await;
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;
    h.ok(
        &alice,
        "chat.postMessage",
        json!({"channel": general, "text": format!("hey <@{bot_user}>")}),
    )
    .await;

    // First come the bot's channel_join and the mention, in order. The
    // channel_join's first delivery fails (500) and is retried in the background,
    // so the retry may arrive after the mention.
    let (h1, first, _) = recv.next("message").await;
    assert!(h1.get("x-slack-retry-num").is_none());
    assert_eq!(first["event"]["subtype"], "channel_join");
    let (mut msg, mut retry) = (None, None);
    while msg.is_none() || retry.is_none() {
        let got = recv.next("message").await;
        if got.0.contains_key("x-slack-retry-num") {
            retry = Some(got);
        } else {
            msg = Some(got.1);
        }
    }
    let (h2, retry, raw) = retry.unwrap();
    let msg = msg.unwrap();
    assert_eq!(h2["x-slack-retry-num"], "1");
    assert_eq!(h2["x-slack-retry-reason"], "http_error");
    assert_eq!(retry["event"]["subtype"], "channel_join");
    assert_eq!(retry["event_id"], first["event_id"]);

    // Signature checks out against the signing secret.
    let ts = h2["x-slack-request-timestamp"].to_str().unwrap();
    assert_eq!(h2["x-slack-signature"].to_str().unwrap(), sign(&secret, ts, &raw));
    assert_eq!(retry["type"], "event_callback");
    assert_eq!(retry["api_app_id"], app_id.as_str());
    assert_eq!(retry["authorizations"][0]["user_id"], bot_user.as_str());

    assert_eq!(msg["event"]["text"], format!("hey <@{bot_user}>"));
    assert_eq!(msg["event"]["user"], alice_id.as_str());
    assert_eq!(msg["event"]["channel_type"], "channel");
    let (_, mention, _) = recv.next("app_mention").await;
    assert_eq!(mention["event"]["channel"], general.as_str());
    assert_eq!(mention["event"]["ts"], msg["event"]["ts"]);

    let log = h.ok(&alice, "emu.apps.deliveries", json!({"app_id": app_id})).await;
    let log = log["deliveries"].as_array().unwrap();
    assert!(log.iter().any(|d| d["ok"] == false && d["error"] == "http_error (500)"));
    assert!(log.iter().all(|d| d["transport"] == "http"));
}

#[tokio::test]
async fn unverified_request_urls_get_no_events() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let m = manifest(
        "Nowhere",
        json!({"event_subscriptions": {"request_url": "http://127.0.0.1:9/events", "bot_events": ["message.channels"]}}),
    );
    let created = h.ok(&alice, "apps.manifest.create", json!({"manifest": m})).await;
    assert_eq!(created["warning"], "request_url_not_verified");
    let app_id = created["app_id"].as_str().unwrap();
    let r = h.ok(&alice, "emu.apps.verifyUrl", json!({"app_id": app_id})).await;
    assert_eq!(r["verified"], false);
}

#[tokio::test]
async fn socket_mode_delivers_and_retries_unacked_envelopes() {
    let h = Harness::with_config(|c| c.event_retry_delays = vec![Duration::from_millis(50)]).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let app = router(h.state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let m = manifest(
        "Sockety",
        json!({"socket_mode_enabled": true, "event_subscriptions": {"bot_events": ["message.channels"]}}),
    );
    let (app_id, _token, bot_user, _) = install(&h, &alice, m).await;

    // Only app-level tokens may open connections.
    h.err(&_token, "apps.connections.open", json!({}), "not_allowed_token_type")
        .await;
    let xapp = h.ok(&alice, "emu.apps.tokens.create", json!({"app_id": app_id})).await["token"]
        .as_str()
        .unwrap()
        .to_owned();
    h.err(&xapp, "auth.test", json!({}), "not_allowed_token_type").await;
    let url = h.ok(&xapp, "apps.connections.open", json!({})).await["url"]
        .as_str()
        .unwrap()
        .replace("localhost:8080", &addr);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Tickets are single-use.
    assert!(tokio_tungstenite::connect_async(&url).await.is_err());

    let mut next = async || -> Value {
        let msg = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        serde_json::from_str(msg.to_text().unwrap()).unwrap()
    };
    let hello = next().await;
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["connection_info"]["app_id"], app_id.as_str());
    let info = h.ok(&alice, "emu.apps.info", json!({"app_id": app_id})).await;
    assert_eq!(info["app"]["socket_connections"], 1);

    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": general, "users": bot_user}),
    )
    .await;
    // channel_join message: don't ack, so it's retried.
    let env = next().await;
    assert_eq!(env["type"], "events_api");
    assert_eq!(env["payload"]["event"]["subtype"], "channel_join");
    assert_eq!(env["retry_attempt"], 0);
    let again = next().await;
    assert_eq!(again["retry_attempt"], 1);
    assert_eq!(again["retry_reason"], "timeout");
    assert_eq!(again["payload"]["event_id"], env["payload"]["event_id"]);
    ws.send(WsMessage::text(
        json!({"envelope_id": again["envelope_id"]}).to_string(),
    ))
    .await
    .unwrap();

    h.ok(
        &alice,
        "chat.postMessage",
        json!({"channel": general, "text": "over the socket"}),
    )
    .await;
    let msg = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let v: Value = serde_json::from_str(m.to_text().unwrap()).unwrap();
        if v["payload"]["event"]["text"] == "over the socket" {
            break v;
        }
    };
    ws.send(WsMessage::text(json!({"envelope_id": msg["envelope_id"]}).to_string()))
        .await
        .unwrap();

    // Revoking the app token disconnects.
    h.ok(
        &alice,
        "emu.apps.tokens.revoke",
        json!({"app_id": app_id, "target": xapp}),
    )
    .await;
    let bye = loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let Ok(v) = serde_json::from_str::<Value>(m.to_text().unwrap_or(""))
            && v["type"] == "disconnect"
        {
            break v;
        }
    };
    assert_eq!(bye["reason"], "link_disabled");
    h.err(&xapp, "apps.connections.open", json!({}), "invalid_auth").await;
}
