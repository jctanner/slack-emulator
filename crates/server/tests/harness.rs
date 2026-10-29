//! The test harness: `/_emu/*` (reset, seed, logs, act-as, wait, rate limits)
//! and the inspector methods.

mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use common::{Harness, Resp};
use serde_json::{Value, json};

const ADMIN: &str = "admin-secret";

async fn harness() -> Harness {
    Harness::with_config(|c| c.admin_token = Some(ADMIN.into())).await
}

/// Calls `/_emu{path}` with the admin token.
async fn emu(h: &Harness, method: Method, path: &str, body: Option<(&str, String)>) -> Resp {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/_emu{path}"))
        .header(header::AUTHORIZATION, format!("Bearer {ADMIN}"));
    let body = match body {
        Some((ct, b)) => {
            req = req.header(header::CONTENT_TYPE, ct);
            Body::from(b)
        }
        None => Body::empty(),
    };
    h.send(req.body(body).unwrap()).await
}

async fn post(h: &Harness, path: &str, body: Value) -> Resp {
    emu(h, Method::POST, path, Some(("application/json", body.to_string()))).await
}

async fn get(h: &Harness, path: &str) -> Resp {
    emu(h, Method::GET, path, None).await
}

const FIXTURE: &str = r#"
workspaces:
  - domain: acme
    name: Acme Inc
    users:
      - name: alice
        real_name: Alice Anders
        email: alice@acme.test
        password: password123
        owner: true
      - name: bob
    apps:
      - manifest:
          display_information: {name: Deploy Bot}
          features: {bot_user: {display_name: Deploy Bot}}
          oauth_config: {scopes: {bot: [chat:write, channels:history, channels:read, reactions:write]}}
          settings: {socket_mode_enabled: true, event_subscriptions: {bot_events: [message.channels]}}
        bot_token: xoxb-test-deploy
        app_token: xapp-test-deploy
        signing_secret: fixed-signing-secret
    channels:
      - name: deploys
        members: [alice, bob, Deploy Bot]
        topic: Ship it
        messages:
          - user: alice
            text: First deploy today
            reactions: {rocket: [bob]}
            replies:
              - {user: Deploy Bot, text: On it}
      - name: secret
        private: true
        members: [alice]
"#;

#[tokio::test]
async fn harness_needs_its_token() {
    let off = Harness::new().await;
    let r = off.post_json("/_emu/reset", None, json!({})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body["error"], "harness_disabled");

    let h = harness().await;
    let r = h.post_json("/_emu/reset", None, json!({})).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(get(&h, "/seq").await.body["ok"], true);
}

#[tokio::test]
async fn seeding_creates_people_apps_and_history() {
    let h = harness().await;
    let r = emu(&h, Method::POST, "/seed", Some(("application/yaml", FIXTURE.into()))).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let acme = &r.body["workspaces"]["acme"];
    let deploys = acme["channels"]["deploys"].as_str().unwrap();
    let app = &acme["apps"]["Deploy Bot"];
    assert_eq!(app["bot_token"], "xoxb-test-deploy");
    assert_eq!(app["app_token"], "xapp-test-deploy");
    assert_eq!(app["credentials"]["signing_secret"], "fixed-signing-secret");

    // The fixed token works, and the bot is in the channel.
    let me = h.ok("xoxb-test-deploy", "auth.test", json!({})).await;
    assert_eq!(me["team_id"], acme["id"]);
    assert_eq!(me["user_id"], app["bot_user_id"]);
    let history = h
        .ok("xoxb-test-deploy", "conversations.history", json!({"channel": deploys}))
        .await;
    let first = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["text"] == "First deploy today")
        .expect("seeded message");
    assert_eq!(first["user"], acme["users"]["alice"]);
    assert_eq!(first["reply_count"], 1);
    assert_eq!(first["reactions"][0]["name"], "rocket");
    let info = h
        .ok("xoxb-test-deploy", "conversations.info", json!({"channel": deploys}))
        .await;
    assert_eq!(info["channel"]["topic"]["value"], "Ship it");

    // Seeded people with passwords can sign in.
    let login = h
        .post_json(
            "/auth/login",
            None,
            json!({"email": "alice@acme.test", "password": "password123"}),
        )
        .await;
    assert_eq!(login.status, StatusCode::OK, "{}", login.body);

    // Seeding again reuses what exists; the fixed tokens are already taken.
    let again = emu(&h, Method::POST, "/seed", Some(("application/yaml", FIXTURE.into()))).await;
    assert_eq!(again.body["error"], "seed_failed");
    assert!(again.body["message"].as_str().unwrap().contains("already exists"));

    // Mistakes are explained.
    let bad = post(&h, "/seed", json!({"workspaces": [{"domain": "x", "userz": []}]})).await;
    assert_eq!(bad.body["error"], "invalid_fixture");
    assert!(bad.body["message"].as_str().unwrap().contains("userz"));
    let bad = post(
        &h,
        "/seed",
        json!({"workspaces": [{"domain": "acme", "channels": [{"name": "x", "members": ["nobody"]}]}]}),
    )
    .await;
    assert_eq!(bad.body["message"], "channel #x: no user or app named \"nobody\"");
}

#[tokio::test]
async fn acting_as_people_and_waiting_for_events() {
    let h = harness().await;
    let seeded = emu(&h, Method::POST, "/seed", Some(("application/yaml", FIXTURE.into()))).await;
    let deploys = seeded.body["workspaces"]["acme"]["channels"]["deploys"]
        .as_str()
        .unwrap()
        .to_owned();

    let r = post(
        &h,
        "/as/alice/chat.postMessage",
        json!({"channel": deploys, "text": "hello bot"}),
    )
    .await;
    assert_eq!(r.body["ok"], true, "{}", r.body);
    let seq: u64 = r.headers["x-emu-seq"].to_str().unwrap().parse().unwrap();

    // Already happened: found immediately.
    let w = get(
        &h,
        &format!("/wait?type=message&channel={deploys}&text=hello%20bot&after={seq}"),
    )
    .await;
    assert_eq!(w.status, StatusCode::OK);
    assert_eq!(
        w.body["event"]["event"]["user"],
        seeded.body["workspaces"]["acme"]["users"]["alice"]
    );
    let found = w.body["seq"].as_u64().unwrap();

    // Not yet happened: waits for it.
    let waiter = {
        let app = h.app.clone();
        let path = format!("/_emu/wait?type=message&contains=later&after={found}&timeout=5");
        tokio::spawn(async move {
            use tower::ServiceExt;
            let req = Request::get(path)
                .header(header::AUTHORIZATION, format!("Bearer {ADMIN}"))
                .body(Body::empty())
                .unwrap();
            let res = app.oneshot(req).await.unwrap();
            let bytes = http_body_util::BodyExt::collect(res.into_body())
                .await
                .unwrap()
                .to_bytes();
            serde_json::from_slice::<Value>(&bytes).unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(100)).await;
    h.ok(
        "xoxb-test-deploy",
        "chat.postMessage",
        json!({"channel": deploys, "text": "see you later"}),
    )
    .await;
    let got = waiter.await.unwrap();
    assert_eq!(got["event"]["event"]["text"], "see you later");
    assert!(got["event"]["event"]["bot_id"].is_string());

    let timeout = get(&h, &format!("/wait?type=message&text=never&after={found}&timeout=1")).await;
    assert_eq!(timeout.status, StatusCode::REQUEST_TIMEOUT);
    assert_eq!(timeout.body["error"], "timeout");

    // Session-only methods work through act-as; people are found by email too.
    let r = post(&h, "/as/alice@acme.test/client.counts", json!({})).await;
    assert_eq!(r.body["ok"], true, "{}", r.body);
    let r = post(&h, "/as/nobody/auth.test", json!({})).await;
    assert_eq!(r.body["error"], "user_not_found");
    let bot = seeded.body["workspaces"]["acme"]["apps"]["Deploy Bot"]["bot_user_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let r = post(&h, &format!("/as/{bot}/auth.test"), json!({})).await;
    assert_eq!(r.body["error"], "cannot_act_as_bot");

    // Event listing with filters.
    let events = get(&h, &format!("/events?type=message&channel={deploys}&subtype=none")).await;
    let texts: Vec<&str> = events.body["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["event"]["text"].as_str().unwrap())
        .collect();
    assert!(
        texts.contains(&"hello bot") && texts.contains(&"see you later"),
        "{texts:?}"
    );
}

#[tokio::test]
async fn api_log_and_rate_limits() {
    let h = harness().await;
    let seeded = emu(&h, Method::POST, "/seed", Some(("application/yaml", FIXTURE.into()))).await;
    let deploys = seeded.body["workspaces"]["acme"]["channels"]["deploys"]
        .as_str()
        .unwrap()
        .to_owned();
    let post_msg = json!({"channel": deploys, "text": "hi"});

    // Forced limits fail the next calls with 429 and Retry-After.
    let r = post(
        &h,
        "/rate-limits",
        json!({"force": {"chat.postMessage": {"count": 1, "retry_after": 3}}}),
    )
    .await;
    assert_eq!(r.body["force"]["chat.postMessage"]["count"], 1);
    let req = Request::post("/api/chat.postMessage")
        .header(header::AUTHORIZATION, "Bearer xoxb-test-deploy")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(post_msg.to_string()))
        .unwrap();
    let limited = h.send(req).await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers[header::RETRY_AFTER], "3");
    assert_eq!(limited.body["error"], "ratelimited");
    h.ok("xoxb-test-deploy", "chat.postMessage", post_msg.clone()).await;

    // Tier limits, counted per app; people in the web UI aren't limited.
    post(
        &h,
        "/rate-limits",
        json!({"enabled": true, "methods": {"conversations.info": 2}}),
    )
    .await;
    let info = json!({"channel": deploys});
    h.ok("xoxb-test-deploy", "conversations.info", info.clone()).await;
    h.ok("xoxb-test-deploy", "conversations.info", info.clone()).await;
    h.err("xoxb-test-deploy", "conversations.info", info.clone(), "ratelimited")
        .await;
    let alice = post(&h, "/as/alice/conversations.info", info.clone()).await;
    assert_eq!(alice.body["ok"], true);

    // The call log records it all, tokens redacted.
    let log = get(&h, "/api-log?method=conversations.info&via=token").await;
    let entries = log.body["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["token"], "xoxb-…ploy");
    assert_eq!(entries[0]["token_type"], "bot");
    assert_eq!(entries[2]["status"], 429);
    let errors = get(&h, "/api-log?errors=true").await;
    assert!(
        errors.body["entries"]
            .as_array()
            .unwrap()
            .iter()
            .all(|e| e["ok"] == false)
    );
    let after = entries[0]["seq"].as_u64().unwrap();
    let newer = get(
        &h,
        &format!("/api-log?method=conversations.info&via=token&after={after}"),
    )
    .await;
    assert_eq!(newer.body["entries"].as_array().unwrap().len(), 2);
    let harness_calls = get(&h, "/api-log?via=harness").await;
    assert_eq!(
        harness_calls.body["entries"][0]["user_id"],
        seeded.body["workspaces"]["acme"]["users"]["alice"]
    );
}

#[tokio::test]
async fn reset_wipes_everything() {
    let h = harness().await;
    emu(&h, Method::POST, "/seed", Some(("application/yaml", FIXTURE.into()))).await;
    let before = get(&h, "/seq").await.body["seq"].as_u64().unwrap();
    let r = post(&h, "/reset", json!({})).await;
    assert_eq!(r.body["ok"], true);
    assert_eq!(r.body["team"]["domain"], "emulator");
    assert_eq!(get(&h, "/api-log").await.body["entries"], json!([]));
    assert_eq!(get(&h, "/events").await.body["events"], json!([]));
    h.err("xoxb-test-deploy", "auth.test", json!({}), "invalid_auth").await;
    // Rejected tokens are still logged, so bad credentials are easy to spot.
    let log = get(&h, "/api-log?errors=true").await;
    assert_eq!(log.body["entries"][0]["token"], "xoxb-…ploy");
    assert_eq!(log.body["entries"][0]["via"], "token");
    // Sequence numbers keep increasing, so old `after` values stay meaningful.
    assert!(r.body["seq"].as_u64().unwrap() >= before);
    // The fixture loads again cleanly.
    let again = emu(&h, Method::POST, "/seed", Some(("application/yaml", FIXTURE.into()))).await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
}

#[tokio::test]
async fn inspector_is_for_admins() {
    let h = harness().await;
    let owner = h.signup("owner@example.com").await;
    let member = h.signup("member@example.com").await;
    h.err(&member, "emu.inspector.apiLog", json!({}), "not_allowed").await;
    let (bot, _) = h.bot("helper").await;
    h.ok(&bot, "auth.test", json!({})).await;
    let log = h.ok(&owner, "emu.inspector.apiLog", json!({})).await;
    let methods: Vec<&str> = log["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["method"].as_str().unwrap())
        .collect();
    assert_eq!(methods, ["auth.test"], "UI calls are hidden by default");
    let events = h.ok(&owner, "emu.inspector.events", json!({})).await;
    assert!(!events["entries"].as_array().unwrap().is_empty());
    assert_eq!(events["rate_limits"]["enabled"], false);
}

#[tokio::test]
async fn seed_file_loads_into_an_empty_database_only() {
    let dir = std::env::temp_dir().join(format!("emu-seed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("fixture.yaml");
    std::fs::write(&path, FIXTURE).unwrap();
    let h = Harness::with_config(|c| c.seed_file = Some(path.to_string_lossy().into())).await;
    h.ok("xoxb-test-deploy", "auth.test", json!({})).await;

    // Starting again on the same (now populated) database leaves it alone.
    let again = slack_emu_server::harness::seed_file_on_empty(&h.state, path.to_str().unwrap()).await;
    assert!(again.is_ok());
    let bad = dir.join("bad.yaml");
    std::fs::write(&bad, "workspaces: [{domain: x, nope: 1}]").unwrap();
    let fresh = Harness::new().await;
    let err = slack_emu_server::harness::seed_file_on_empty(&fresh.state, bad.to_str().unwrap())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown field `nope`"), "{err}");
}
