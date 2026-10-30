//! Interactivity: Block Kit validation, ephemeral messages, block_actions and
//! response URLs, modals, slash commands and App Home.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use common::Harness;
use serde_json::{Value, json};
use slack_emu_server::delivery::sign;
use slack_emu_server::events::BusEvent;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

type Reply = Arc<dyn Fn(&Value) -> (StatusCode, String) + Send + Sync>;

/// A fake HTTP app. Records what it receives (interaction payloads, slash
/// command fields as an object, and Events API callbacks), checks signatures,
/// and answers with `reply`.
struct FakeApp {
    base: String,
    rx: mpsc::UnboundedReceiver<Value>,
    secret: Arc<Mutex<String>>,
}

async fn fake_app(reply: Reply) -> FakeApp {
    let (tx, rx) = mpsc::unbounded_channel();
    let secret = Arc::new(Mutex::new(String::new()));
    let handler = {
        let secret = secret.clone();
        move |headers: HeaderMap, body: Bytes| {
            let tx = tx.clone();
            let reply = reply.clone();
            let secret = secret.lock().unwrap().clone();
            async move {
                let raw = String::from_utf8(body.to_vec()).unwrap();
                let v: Value = if headers["content-type"] == "application/json" {
                    serde_json::from_str(&raw).unwrap()
                } else {
                    let pairs: Vec<(String, String)> = serde_urlencoded::from_str(&raw).unwrap();
                    match pairs.iter().find(|(k, _)| k == "payload") {
                        Some((_, p)) => serde_json::from_str(p).unwrap(),
                        None => pairs.into_iter().map(|(k, v)| (k, Value::String(v))).collect(),
                    }
                };
                // Verification happens during apps.manifest.create, before we know the secret.
                if v["type"] == "url_verification" {
                    return (StatusCode::OK, v["challenge"].as_str().unwrap().to_owned());
                }
                let ts = headers["x-slack-request-timestamp"].to_str().unwrap();
                assert_eq!(headers["x-slack-signature"].to_str().unwrap(), sign(&secret, ts, &raw));
                let _ = tx.send(v.clone());
                reply(&v)
            }
        }
    };
    let app = Router::new()
        .route("/interactive", post(handler.clone()))
        .route("/commands", post(handler.clone()))
        .route("/events", post(handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    FakeApp { base, rx, secret }
}

impl FakeApp {
    async fn next(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(5), self.rx.recv())
            .await
            .expect("app received nothing")
            .unwrap()
    }
}

fn ok_empty() -> (StatusCode, String) {
    (StatusCode::OK, String::new())
}

struct Installed {
    app_id: String,
    token: String,
    bot_user: String,
}

/// Creates and installs an HTTP app pointed at `fake`.
async fn install(h: &Harness, who: &str, fake: &FakeApp, extra: impl FnOnce(&mut Value)) -> Installed {
    let mut m = json!({
        "display_information": {"name": "Clicky"},
        "features": {
            "bot_user": {"display_name": "clicky"},
            "slash_commands": [
                {"command": "/deploy", "url": format!("{}/commands", fake.base), "description": "Ship it",
                 "should_escape": true}
            ],
            "app_home": {"home_tab_enabled": true, "messages_tab_enabled": true}
        },
        "oauth_config": {"scopes": {"bot": ["chat:write", "chat:write.public", "commands", "users:read"]}},
        "settings": {
            "interactivity": {"is_enabled": true, "request_url": format!("{}/interactive", fake.base)},
        },
    });
    extra(&mut m);
    let created = h
        .ok(who, "apps.manifest.create", json!({"manifest": m.to_string()}))
        .await;
    *fake.secret.lock().unwrap() = created["credentials"]["signing_secret"].as_str().unwrap().to_owned();
    let app_id = created["app_id"].as_str().unwrap().to_owned();
    let inst = h.ok(who, "emu.apps.install", json!({"app_id": app_id})).await;
    Installed {
        app_id,
        token: inst["access_token"].as_str().unwrap().to_owned(),
        bot_user: inst["bot_user_id"].as_str().unwrap().to_owned(),
    }
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

/// The next UI event of this type on the bus.
async fn bus_next(rx: &mut broadcast::Receiver<Arc<BusEvent>>, ty: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ev = rx.recv().await.unwrap();
            if ev.event["type"] == ty {
                return ev.event.clone();
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {ty} event"))
}

/// The path part of a response URL, for calling it in-process.
fn hook_path(url: &str) -> String {
    url.split_once("/hooks/")
        .map(|(_, rest)| format!("/hooks/{rest}"))
        .unwrap()
}

fn button_blocks() -> Value {
    json!([
        {"type": "section", "text": {"type": "mrkdwn", "text": "Deploy *v2*?"}},
        {"type": "actions", "block_id": "decide", "elements": [
            {"type": "button", "action_id": "approve", "text": {"type": "plain_text", "text": "Approve"},
             "style": "primary", "value": "v2"},
            {"type": "static_select", "action_id": "env", "options": [
                {"text": {"type": "plain_text", "text": "Production"}, "value": "prod"}
            ]}
        ]}
    ])
}

#[tokio::test]
async fn blocks_are_validated_and_ephemerals_reach_one_person() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let bob = h.signup("bob@example.com").await;
    let general = general(&h, &alice).await;
    let fake = fake_app(Arc::new(|_| ok_empty())).await;
    let app = install(&h, &alice, &fake, |_| {}).await;

    let bad = h
        .call(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "blocks": [{"type": "section"}]}),
        )
        .await;
    assert_eq!(bad["error"], "invalid_blocks");
    assert_eq!(
        bad["response_metadata"]["messages"][0],
        "[ERROR] must provide at least one of: text, fields [json-pointer:/blocks/0]"
    );

    // IDs are filled in, as Slack does.
    let posted = h
        .ok(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "text": "Deploy?", "blocks": [
                {"type": "actions", "elements": [{"type": "button", "text": {"type": "plain_text", "text": "Go"}}]}
            ]}),
        )
        .await;
    assert!(posted["message"]["blocks"][0]["block_id"].is_string());
    assert!(posted["message"]["blocks"][0]["elements"][0]["action_id"].is_string());

    // Ephemeral: only to a member, never stored.
    let bob_id = h.user_id(&bob).await;
    let private = h
        .ok(
            &alice,
            "conversations.create",
            json!({"name": "secret", "is_private": true}),
        )
        .await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    h.ok(
        &alice,
        "conversations.invite",
        json!({"channel": private, "users": app.bot_user}),
    )
    .await;
    h.err(
        &app.token,
        "chat.postEphemeral",
        json!({"channel": private, "user": bob_id, "text": "psst"}),
        "user_not_in_channel",
    )
    .await;
    let mut bus = h.state.bus.subscribe();
    let r = h
        .ok(
            &app.token,
            "chat.postEphemeral",
            json!({"channel": general, "user": alice_id, "text": "only you", "blocks": button_blocks()}),
        )
        .await;
    let ev = bus_next(&mut bus, "emu_ephemeral").await;
    assert_eq!(ev["message"]["ts"], r["message_ts"]);
    assert_eq!(ev["message"]["is_ephemeral"], true);
    assert_eq!(ev["message"]["bot_profile"]["app_id"], app.app_id.as_str());
    let history = h.ok(&alice, "conversations.history", json!({"channel": general})).await;
    assert!(
        !history["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["text"] == "only you")
    );
}

#[tokio::test]
async fn buttons_send_block_actions_and_response_urls_change_messages() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let general = general(&h, &alice).await;
    let mut fake = fake_app(Arc::new(|_| ok_empty())).await;
    let app = install(&h, &alice, &fake, |_| {}).await;
    let posted = h
        .ok(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "text": "Deploy v2?", "blocks": button_blocks()}),
        )
        .await;
    let ts = posted["ts"].as_str().unwrap().to_owned();
    let container = json!({"type": "message", "channel_id": general, "message_ts": ts});

    // A select changes state; then the button is clicked.
    let prod = json!({"type": "static_select", "selected_option":
        {"text": {"type": "plain_text", "text": "Production"}, "value": "prod"}});
    h.ok(
        &alice,
        "emu.blocks.action",
        json!({"container": container, "block_id": "decide", "action_id": "env", "value": prod,
               "state": {"values": {"decide": {"env": prod}}}}),
    )
    .await;
    let p = fake.next().await;
    assert_eq!(p["actions"][0]["selected_option"]["value"], "prod");

    h.ok(
        &alice,
        "emu.blocks.action",
        json!({"container": container, "block_id": "decide", "action_id": "approve",
               "state": {"values": {"decide": {"env": prod}}}}),
    )
    .await;
    let p = fake.next().await;
    assert_eq!(p["type"], "block_actions");
    assert_eq!(p["user"]["id"], alice_id.as_str());
    assert_eq!(p["api_app_id"], app.app_id.as_str());
    assert_eq!(p["container"]["message_ts"], ts.as_str());
    assert_eq!(p["channel"]["name"], "general");
    assert_eq!(p["message"]["text"], "Deploy v2?");
    let action = &p["actions"][0];
    assert_eq!(action["action_id"], "approve");
    assert_eq!(action["value"], "v2");
    assert_eq!(action["style"], "primary");
    assert_eq!(
        p["state"]["values"]["decide"]["env"]["selected_option"]["value"],
        "prod"
    );
    assert_eq!(p["trigger_id"].as_str().unwrap().split('.').count(), 3);
    let hook = hook_path(p["response_url"].as_str().unwrap());

    // replace_original updates the message without an "edited" mark.
    let r = h
        .post_json(
            &hook,
            None,
            json!({"replace_original": true, "text": "Approved by <@U1>"}),
        )
        .await;
    assert_eq!(r.body["ok"], true, "{}", r.body);
    let replies = h
        .ok(&alice, "conversations.history", json!({"channel": general, "limit": 1}))
        .await;
    let msg = &replies["messages"][0];
    assert_eq!(msg["ts"], ts.as_str());
    assert_eq!(msg["text"], "Approved by <@U1>");
    assert!(msg.get("edited").is_none());

    // A new in-channel message, then delete the original.
    h.post_json(&hook, None, json!({"response_type": "in_channel", "text": "Shipping!"}))
        .await;
    h.post_json(&hook, None, json!({"delete_original": true})).await;
    let history = h.ok(&alice, "conversations.history", json!({"channel": general})).await;
    let texts: Vec<&str> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["text"].as_str().unwrap())
        .collect();
    assert!(texts.contains(&"Shipping!"));
    assert!(!texts.contains(&"Approved by <@U1>"));

    // Five uses per URL.
    h.post_json(&hook, None, json!({"text": "4"})).await;
    h.post_json(&hook, None, json!({"text": "5"})).await;
    let r = h.post_json(&hook, None, json!({"text": "6"})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body["error"], "expired_url");

    // Responses show up in the app's delivery log.
    let log = h.ok(&alice, "emu.apps.deliveries", json!({"app_id": app.app_id})).await;
    assert!(
        log["deliveries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["label"] == "response_url")
    );
}

#[tokio::test]
async fn unanswered_and_disabled_interactions_report_errors() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let fake = fake_app(Arc::new(|_| (StatusCode::INTERNAL_SERVER_ERROR, String::new()))).await;
    let app = install(&h, &alice, &fake, |_| {}).await;
    let posted = h
        .ok(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "text": "x", "blocks": button_blocks()}),
        )
        .await;
    let args = json!({"container": {"type": "message", "channel_id": general, "message_ts": posted["ts"]},
                      "block_id": "decide", "action_id": "approve"});
    let r = h.call(&alice, "emu.blocks.action", args.clone()).await;
    assert_eq!(r["error"], "dispatch_failed");
    assert_eq!(r["detail"], "http_error (500)");

    let quiet = install(&h, &alice, &fake, |m| {
        m["display_information"]["name"] = json!("Quiet");
        m["settings"]["interactivity"] = json!({"is_enabled": false});
    })
    .await;
    let posted = h
        .ok(
            &quiet.token,
            "chat.postMessage",
            json!({"channel": general, "text": "x", "blocks": button_blocks()}),
        )
        .await;
    let mut args = args;
    args["container"]["message_ts"] = posted["ts"].clone();
    h.err(&alice, "emu.blocks.action", args, "interactivity_disabled").await;
}

fn modal(title: &str) -> Value {
    json!({
        "type": "modal",
        "callback_id": "ticket",
        "notify_on_close": true,
        "private_metadata": "C123",
        "title": {"type": "plain_text", "text": title},
        "submit": {"type": "plain_text", "text": "Create"},
        "blocks": [
            {"type": "input", "block_id": "title", "label": {"type": "plain_text", "text": "Title"},
             "element": {"type": "plain_text_input", "action_id": "value"}}
        ]
    })
}

#[tokio::test]
async fn modals_open_validate_submit_and_close() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let general = general(&h, &alice).await;
    // The app rejects the first submission and accepts the second.
    let submissions = Arc::new(Mutex::new(0));
    let reply: Reply = {
        let submissions = submissions.clone();
        Arc::new(move |p| {
            if p["type"] != "view_submission" {
                return ok_empty();
            }
            let mut n = submissions.lock().unwrap();
            *n += 1;
            if *n == 1 {
                let body = json!({"response_action": "errors", "errors": {"title": "Too short"}});
                return (StatusCode::OK, body.to_string());
            }
            ok_empty()
        })
    };
    let mut fake = fake_app(reply).await;
    let app = install(&h, &alice, &fake, |_| {}).await;
    let posted = h
        .ok(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "text": "x", "blocks": button_blocks()}),
        )
        .await;
    h.ok(
        &alice,
        "emu.blocks.action",
        json!({"container": {"type": "message", "channel_id": general, "message_ts": posted["ts"]},
               "block_id": "decide", "action_id": "approve"}),
    )
    .await;
    let trigger = fake.next().await["trigger_id"].as_str().unwrap().to_owned();

    // Missing submit with input blocks is rejected like Slack.
    let mut no_submit = modal("New ticket");
    no_submit.as_object_mut().unwrap().remove("submit");
    let r = h
        .call(
            &app.token,
            "views.open",
            json!({"trigger_id": trigger, "view": no_submit}),
        )
        .await;
    assert_eq!(r["error"], "invalid_arguments");
    assert_eq!(
        r["response_metadata"]["messages"][0],
        "[ERROR] must provide a submit property when using input blocks [json-pointer:/view]"
    );

    let mut bus = h.state.bus.subscribe();
    let opened = h
        .ok(
            &app.token,
            "views.open",
            json!({"trigger_id": trigger, "view": modal("New ticket")}),
        )
        .await;
    let view_id = opened["view"]["id"].as_str().unwrap().to_owned();
    assert!(view_id.starts_with('V'));
    assert_eq!(opened["view"]["root_view_id"], view_id.as_str());
    assert_eq!(
        bus_next(&mut bus, "emu_modals").await["views"][0]["id"],
        view_id.as_str()
    );
    h.err(
        &app.token,
        "views.open",
        json!({"trigger_id": trigger, "view": modal("Again")}),
        "exchanged_trigger_id",
    )
    .await;

    // Updates need the current hash, if one is given.
    h.err(
        &app.token,
        "views.update",
        json!({"view_id": view_id, "hash": "stale", "view": modal("Edited")}),
        "hash_conflict",
    )
    .await;
    let updated = h
        .ok(
            &app.token,
            "views.update",
            json!({"view_id": view_id, "hash": opened["view"]["hash"], "view": modal("Edited")}),
        )
        .await;
    assert_eq!(updated["view"]["title"]["text"], "Edited");
    assert_ne!(updated["view"]["hash"], opened["view"]["hash"]);

    // Required fields are checked before the app sees anything.
    let r = h.ok(&alice, "emu.views.submit", json!({"view_id": view_id})).await;
    assert_eq!(r["errors"]["title"], "Please complete this required field.");

    let values = json!({"values": {"title": {"value": {"type": "plain_text_input", "value": "Hi"}}}});
    let r = h
        .ok(&alice, "emu.views.submit", json!({"view_id": view_id, "state": values}))
        .await;
    assert_eq!(r["errors"]["title"], "Too short");
    let p = fake.next().await;
    assert_eq!(p["type"], "view_submission");
    assert_eq!(p["user"]["id"], alice_id.as_str());
    assert_eq!(p["view"]["callback_id"], "ticket");
    assert_eq!(p["view"]["private_metadata"], "C123");
    assert_eq!(p["view"]["state"]["values"]["title"]["value"]["value"], "Hi");

    // Accepted: the modal closes.
    h.ok(&alice, "emu.views.submit", json!({"view_id": view_id, "state": values}))
        .await;
    fake.next().await;
    let current = h.ok(&alice, "emu.views.current", json!({})).await;
    assert_eq!(current["views"], json!([]));
    h.err(
        &app.token,
        "views.update",
        json!({"view_id": view_id, "view": modal("Gone")}),
        "not_found",
    )
    .await;
}

#[tokio::test]
async fn modal_stacks_push_and_close() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let mut fake = fake_app(Arc::new(|_| ok_empty())).await;
    let app = install(&h, &alice, &fake, |_| {}).await;
    let posted = h
        .ok(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "text": "x", "blocks": button_blocks()}),
        )
        .await;
    let click = json!({"container": {"type": "message", "channel_id": general, "message_ts": posted["ts"]},
                       "block_id": "decide", "action_id": "approve"});
    let mut trigger = async || {
        h.ok(&alice, "emu.blocks.action", click.clone()).await;
        fake.next().await["trigger_id"].as_str().unwrap().to_owned()
    };

    let t = trigger().await;
    h.err(
        &app.token,
        "views.push",
        json!({"trigger_id": t, "view": modal("One")}),
        "not_found",
    )
    .await;
    let t = trigger().await;
    let root = h
        .ok(&app.token, "views.open", json!({"trigger_id": t, "view": modal("One")}))
        .await["view"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for title in ["Two", "Three"] {
        let t = trigger().await;
        let v = h
            .ok(&app.token, "views.push", json!({"trigger_id": t, "view": modal(title)}))
            .await;
        assert_eq!(v["view"]["root_view_id"], root.as_str());
    }
    let t = trigger().await;
    h.err(
        &app.token,
        "views.push",
        json!({"trigger_id": t, "view": modal("Four")}),
        "push_limit_reached",
    )
    .await;
    let stack = h.ok(&alice, "emu.views.current", json!({})).await["views"].clone();
    assert_eq!(stack.as_array().unwrap().len(), 3);

    // Back from the top view, then close everything.
    h.ok(&alice, "emu.views.close", json!({"view_id": stack[2]["id"]}))
        .await;
    let closed = fake.next().await;
    assert_eq!(closed["type"], "view_closed");
    assert_eq!(closed["view"]["title"]["text"], "Three");
    assert_eq!(closed["is_cleared"], false);
    h.ok(
        &alice,
        "emu.views.close",
        json!({"view_id": stack[1]["id"], "all": true}),
    )
    .await;
    let closed = fake.next().await;
    assert_eq!(closed["is_cleared"], true);
    assert_eq!(h.ok(&alice, "emu.views.current", json!({})).await["views"], json!([]));
}

#[tokio::test]
async fn slash_commands_reach_apps_and_responses_come_back() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let general = general(&h, &alice).await;
    let reply: Reply = Arc::new(|p| match p["text"].as_str() {
        Some(t) if t.starts_with("loud") => (
            StatusCode::OK,
            json!({"response_type": "in_channel", "text": "Deploying to everyone"}).to_string(),
        ),
        Some("fail") => (StatusCode::INTERNAL_SERVER_ERROR, String::new()),
        _ => (StatusCode::OK, "Only you can see this".into()),
    });
    let mut fake = fake_app(reply).await;
    let app = install(&h, &alice, &fake, |_| {}).await;

    let listed = h.ok(&alice, "emu.commands.list", json!({})).await;
    assert_eq!(listed["commands"][0]["command"], "/deploy");

    let mut bus = h.state.bus.subscribe();
    let r = h
        .ok(
            &alice,
            "chat.command",
            json!({"channel": general, "command": "/nope", "text": "x"}),
        )
        .await;
    assert_eq!(r["valid"], false);
    let ev = bus_next(&mut bus, "emu_ephemeral").await;
    assert_eq!(ev["message"]["user"], "USLACKBOT");
    assert!(
        ev["message"]["text"]
            .as_str()
            .unwrap()
            .contains("is not a valid command")
    );

    // Plain-text reply: an ephemeral message from the app.
    h.ok(
        &alice,
        "chat.command",
        json!({"channel": general, "command": "/deploy", "text": "quiet @alice"}),
    )
    .await;
    let p = fake.next().await;
    assert_eq!(p["command"], "/deploy");
    assert_eq!(p["user_id"], alice_id.as_str());
    assert_eq!(p["channel_name"], "general");
    assert_eq!(p["text"], format!("quiet <@{alice_id}|alice>"));
    assert_eq!(p["is_enterprise_install"], "false");
    assert!(p["response_url"].as_str().unwrap().contains("/hooks/commands/"));
    let ev = bus_next(&mut bus, "emu_ephemeral").await;
    assert_eq!(ev["message"]["text"], "Only you can see this");
    assert_eq!(ev["message"]["user"], app.bot_user.as_str());

    // in_channel: the command and the reply are both posted.
    h.ok(
        &alice,
        "chat.command",
        json!({"channel": general, "command": "/deploy", "text": "loud"}),
    )
    .await;
    let response_url = fake.next().await["response_url"].as_str().unwrap().to_owned();
    let history = h
        .ok(&alice, "conversations.history", json!({"channel": general, "limit": 2}))
        .await;
    assert_eq!(history["messages"][0]["text"], "Deploying to everyone");
    assert!(history["messages"][0]["bot_id"].is_string());
    assert_eq!(history["messages"][1]["text"], "/deploy loud");
    assert_eq!(history["messages"][1]["user"], alice_id.as_str());

    // Later responses through the response URL.
    h.post_json(
        &hook_path(&response_url),
        None,
        json!({"response_type": "in_channel", "text": "Done!"}),
    )
    .await;
    let history = h
        .ok(&alice, "conversations.history", json!({"channel": general, "limit": 1}))
        .await;
    assert_eq!(history["messages"][0]["text"], "Done!");

    // Failures are reported by Slackbot.
    h.ok(
        &alice,
        "chat.command",
        json!({"channel": general, "command": "/deploy", "text": "fail"}),
    )
    .await;
    let ev = loop {
        let ev = bus_next(&mut bus, "emu_ephemeral").await;
        if ev["message"]["user"] == "USLACKBOT" {
            break ev;
        }
    };
    assert_eq!(
        ev["message"]["text"],
        "`/deploy` failed with the error \"dispatch_failed\""
    );
}

#[tokio::test]
async fn app_home_publish_and_open() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let alice_id = h.user_id(&alice).await;
    let mut fake = fake_app(Arc::new(|_| ok_empty())).await;
    let events_url = format!("{}/events", fake.base);
    let app = install(&h, &alice, &fake, |m| {
        m["settings"]["event_subscriptions"] = json!({"request_url": events_url, "bot_events": ["app_home_opened"]});
    })
    .await;
    let dm = h.ok(&alice, "conversations.open", json!({"users": app.bot_user})).await["channel"]["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let opened = h
        .ok(&alice, "emu.apps.home", json!({"channel": dm, "tab": "home"}))
        .await;
    assert_eq!(opened["app"]["home_tab_enabled"], true);
    assert_eq!(opened["view"], Value::Null);
    let ev = fake.next().await;
    assert_eq!(ev["event"]["type"], "app_home_opened");
    assert_eq!(ev["event"]["tab"], "home");
    assert_eq!(ev["event"]["user"], alice_id.as_str());
    assert_eq!(ev["event"]["channel"], dm.as_str());

    let home = json!({"type": "home", "blocks": [{"type": "section", "text": {"type": "mrkdwn", "text": "Welcome"}}]});
    h.err(
        &app.token,
        "views.publish",
        json!({"user_id": alice_id, "view": modal("Nope")}),
        "invalid_arguments",
    )
    .await;
    let mut bus = h.state.bus.subscribe();
    let published = h
        .ok(&app.token, "views.publish", json!({"user_id": alice_id, "view": home}))
        .await;
    assert_eq!(
        bus_next(&mut bus, "emu_home_view").await["view"]["id"],
        published["view"]["id"]
    );
    // Republishing keeps the view ID.
    let again = h
        .ok(&app.token, "views.publish", json!({"user_id": alice_id, "view": home}))
        .await;
    assert_eq!(again["view"]["id"], published["view"]["id"]);

    let opened = h
        .ok(&alice, "emu.apps.home", json!({"channel": dm, "tab": "home"}))
        .await;
    assert_eq!(opened["view"]["blocks"][0]["text"]["text"], "Welcome");
    assert_eq!(fake.next().await["event"]["view"]["id"], published["view"]["id"]);
}

#[tokio::test]
async fn external_selects_load_options_from_the_app() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let reply: Reply = Arc::new(|p| {
        if p["type"] != "block_suggestion" {
            return ok_empty();
        }
        let q = p["value"].as_str().unwrap_or_default().to_owned();
        let body = json!({"options": [
            {"text": {"type": "plain_text", "text": format!("{q}-one")}, "value": "1"},
            {"text": {"type": "plain_text", "text": format!("{q}-two")}, "value": "2"}
        ]});
        (StatusCode::OK, body.to_string())
    });
    let mut fake = fake_app(reply).await;
    let options_url = format!("{}/interactive", fake.base);
    let app = install(&h, &alice, &fake, |m| {
        m["settings"]["interactivity"]["message_menu_options_url"] = json!(options_url);
    })
    .await;
    let blocks = json!([{"type": "actions", "block_id": "pick", "elements": [
        {"type": "external_select", "action_id": "ticket", "min_query_length": 2,
         "placeholder": {"type": "plain_text", "text": "Ticket"}},
        {"type": "button", "action_id": "go", "text": {"type": "plain_text", "text": "Go"}}
    ]}]);
    let posted = h
        .ok(
            &app.token,
            "chat.postMessage",
            json!({"channel": general, "text": "pick", "blocks": blocks}),
        )
        .await;
    let container = json!({"type": "message", "channel_id": general, "message_ts": posted["ts"]});

    let r = h
        .ok(
            &alice,
            "emu.blocks.suggest",
            json!({"container": container, "block_id": "pick", "action_id": "ticket", "value": "bug"}),
        )
        .await;
    assert_eq!(r["options"][0]["text"]["text"], "bug-one");
    let p = fake.next().await;
    assert_eq!(p["type"], "block_suggestion");
    assert_eq!(p["action_id"], "ticket");
    assert_eq!(p["value"], "bug");
    assert_eq!(p["container"]["message_ts"], posted["ts"]);

    // Only external selects load options.
    h.err(
        &alice,
        "emu.blocks.suggest",
        json!({"container": container, "block_id": "pick", "action_id": "go", "value": "x"}),
        "invalid_arguments",
    )
    .await;
}

#[tokio::test]
async fn shortcuts_send_triggers_and_message_context() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let general = general(&h, &alice).await;
    let mut fake = fake_app(Arc::new(|_| ok_empty())).await;
    let app = install(&h, &alice, &fake, |m| {
        m["features"]["shortcuts"] = json!([
            {"name": "New ticket", "type": "global", "callback_id": "new_ticket", "description": "File a ticket"},
            {"name": "Summarize", "type": "message", "callback_id": "summarize", "description": "Summarize this"}
        ]);
    })
    .await;

    let listed = h.ok(&alice, "emu.shortcuts.list", json!({})).await;
    let names: Vec<&str> = listed["shortcuts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["callback_id"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["new_ticket", "summarize"]);

    h.ok(
        &alice,
        "emu.shortcuts.run",
        json!({"app_id": app.app_id, "callback_id": "new_ticket"}),
    )
    .await;
    let p = fake.next().await;
    assert_eq!(p["type"], "shortcut");
    assert_eq!(p["callback_id"], "new_ticket");
    // The trigger opens a modal, as apps do from shortcuts.
    let modal = json!({"type": "modal", "title": {"type": "plain_text", "text": "Ticket"}, "blocks": []});
    h.ok(
        &app.token,
        "views.open",
        json!({"trigger_id": p["trigger_id"], "view": modal}),
    )
    .await;

    let posted = h
        .ok(
            &alice,
            "chat.postMessage",
            json!({"channel": general, "text": "long discussion"}),
        )
        .await;
    h.ok(
        &alice,
        "emu.shortcuts.run",
        json!({"app_id": app.app_id, "callback_id": "summarize", "channel": general, "message_ts": posted["ts"]}),
    )
    .await;
    let p = fake.next().await;
    assert_eq!(p["type"], "message_action");
    assert_eq!(p["message"]["text"], "long discussion");
    assert_eq!(p["channel"]["id"], general.as_str());
    // Its response URL answers in the conversation.
    let hook = hook_path(p["response_url"].as_str().unwrap());
    h.post_json(
        &hook,
        None,
        json!({"response_type": "in_channel", "text": "Summary: it was long"}),
    )
    .await;
    let history = h
        .ok(&alice, "conversations.history", json!({"channel": general, "limit": 1}))
        .await;
    assert_eq!(history["messages"][0]["text"], "Summary: it was long");

    // Manifests are checked: shortcuts need interactivity.
    let bad = h
        .call(
            &alice,
            "apps.manifest.validate",
            json!({"manifest": {
                "display_information": {"name": "x"},
                "features": {"bot_user": {"display_name": "x"},
                             "shortcuts": [{"name": "s", "type": "global", "callback_id": "s", "description": "s"}]},
                "oauth_config": {"scopes": {"bot": ["commands"]}}
            }}),
        )
        .await;
    assert_eq!(bad["errors"][0]["pointer"], "/settings/interactivity/is_enabled");
}
