//! End-to-end tests of the HTTP surface against an in-memory database.

mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use common::Harness;
use serde_json::json;
use slack_emu_core::ids::{IdKind, TokenKind, new_api_token, new_id};
use slack_emu_core::models::{ApiToken, NewUser};
use slack_emu_core::now;

#[tokio::test]
async fn api_test_echoes_args_from_query_form_and_json() {
    let h = Harness::new().await;

    let r = h.get("/api/api.test?foo=bar", None).await;
    assert_eq!(r.body, json!({"ok": true, "args": {"foo": "bar"}}));

    let form = Request::post("/api/api.test?a=1")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from("b=2&token=xoxb-ignored"))
        .unwrap();
    let r = h.send(form).await;
    assert_eq!(r.body, json!({"ok": true, "args": {"a": "1", "b": "2"}}));

    let r = h.post_json("/api/api.test", None, json!({"n": 3})).await;
    assert_eq!(r.body["args"], json!({"n": 3}));

    let r = h.get("/api/api.test?error=my_error", None).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body, json!({"ok": false, "error": "my_error"}));
}

#[tokio::test]
async fn auth_errors() {
    let h = Harness::new().await;
    let r = h.get("/api/auth.test", None).await;
    assert_eq!(r.body, json!({"ok": false, "error": "not_authed"}));

    let r = h.api_bearer("auth.test", "xoxb-nope").await;
    assert_eq!(r.body, json!({"ok": false, "error": "invalid_auth"}));

    let r = h.get("/api/no.such.method", None).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body["error"], "unknown_method");
}

#[tokio::test]
async fn signup_login_logout() {
    let h = Harness::new().await;
    let cookie = h.signup("Alice@Example.com").await;

    let me = h.get("/auth/me", Some(&cookie)).await.body;
    assert_eq!(me["account"]["email"], "alice@example.com");
    let ws = &me["workspaces"].as_array().unwrap()[0];
    assert_eq!(ws["team"]["domain"], "emulator");
    assert_eq!(ws["user"]["name"], "alice");
    assert_eq!(ws["user"]["is_owner"], true, "first member owns the default workspace");

    let r = h.get("/api/auth.test", Some(&cookie)).await.body;
    assert_eq!(r["ok"], true);
    assert_eq!(r["user_id"], ws["user"]["id"]);
    assert_eq!(r["team_id"], ws["team"]["id"]);

    // Second member is not an owner.
    let bob = h.signup("bob@example.com").await;
    let me = h.get("/auth/me", Some(&bob)).await.body;
    assert_eq!(me["workspaces"][0]["user"]["is_owner"], false);

    // Validation.
    let r = h
        .post_json(
            "/auth/signup",
            None,
            json!({"email": "alice@example.com", "password": "longenough"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = h
        .post_json(
            "/auth/signup",
            None,
            json!({"email": "c@example.com", "password": "short"}),
        )
        .await;
    assert_eq!(r.body["error"], "password_too_short");

    // Login.
    let r = h
        .post_json(
            "/auth/login",
            None,
            json!({"email": "alice@example.com", "password": "wrong-password"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    let r = h
        .post_json(
            "/auth/login",
            None,
            json!({"email": "ALICE@example.com", "password": "hunter2hunter2"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    let cookie2 = r.cookie.unwrap();

    // Logout kills only that session.
    h.post_json("/auth/logout", Some(&cookie2), json!({})).await;
    assert_eq!(h.get("/auth/me", Some(&cookie2)).await.status, StatusCode::UNAUTHORIZED);
    assert_eq!(h.get("/auth/me", Some(&cookie)).await.status, StatusCode::OK);
}

#[tokio::test]
async fn multiple_workspaces() {
    let h = Harness::new().await;
    let alice = h.signup("alice@example.com").await;
    let bob = h.signup("bob@example.com").await;

    let r = h
        .post_json(
            "/auth/workspaces",
            Some(&alice),
            json!({"name": "Acme", "domain": "acme"}),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let acme_id = r.body["team"]["id"].as_str().unwrap().to_owned();
    let alice_acme = r.body["user"]["id"].as_str().unwrap().to_owned();
    assert_eq!(r.body["user"]["is_owner"], true);

    let r = h
        .post_json("/auth/workspaces", Some(&bob), json!({"name": "Dup", "domain": "acme"}))
        .await;
    assert_eq!(r.body["error"], "domain_taken");

    // The UI selects a workspace with a header; user IDs differ per workspace.
    let req = Request::get("/api/auth.test")
        .header(header::COOKIE, &alice)
        .header("x-slack-emu-team", &acme_id)
        .body(Body::empty())
        .unwrap();
    let r = h.send(req).await.body;
    assert_eq!(r["team_id"], acme_id.as_str());
    assert_eq!(r["user_id"], alice_acme.as_str());

    // Bob isn't a member of Acme until he joins.
    let req = Request::get("/api/auth.test")
        .header(header::COOKIE, &bob)
        .header("x-slack-emu-team", &acme_id)
        .body(Body::empty())
        .unwrap();
    assert_eq!(h.send(req).await.body["error"], "invalid_auth");

    let listed = h.get("/auth/workspaces", Some(&bob)).await.body;
    let acme = listed["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["id"] == acme_id.as_str())
        .unwrap();
    assert_eq!(acme["is_member"], false);

    let r = h
        .post_json("/auth/workspaces/join", Some(&bob), json!({"team_id": acme_id}))
        .await;
    assert_eq!(r.body["user"]["is_owner"], false);
    let me = h.get("/auth/me", Some(&bob)).await.body;
    assert_eq!(me["workspaces"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn bot_and_app_tokens() {
    let h = Harness::new().await;
    let store = &h.state.store;
    let ws = store.get_workspace_by_domain("emulator").await.unwrap().unwrap();
    let bot_user = store
        .create_user(NewUser {
            team_id: ws.id.clone(),
            account_id: None,
            name: "testbot".into(),
            real_name: "Test Bot".into(),
            display_name: "Test Bot".into(),
            email: None,
            is_bot: true,
            is_admin: false,
            is_owner: false,
        })
        .await
        .unwrap();
    let app_id = new_id(IdKind::App);
    let bot_id = new_id(IdKind::Bot);
    let mk = |kind, user_id: Option<String>| ApiToken {
        token: new_api_token(kind),
        kind,
        team_id: ws.id.clone(),
        user_id,
        app_id: Some(app_id.clone()),
        bot_id: (kind == TokenKind::Bot).then(|| bot_id.clone()),
        scopes: vec![],
        created: now(),
        revoked: false,
        name: None,
    };
    let bot = mk(TokenKind::Bot, Some(bot_user.id.clone()));
    let app = mk(TokenKind::App, None);
    let mut revoked = mk(TokenKind::Bot, Some(bot_user.id.clone()));
    revoked.revoked = true;
    for t in [&bot, &app, &revoked] {
        store.create_token(t).await.unwrap();
    }

    let r = h.api_bearer("auth.test", &bot.token).await.body;
    assert_eq!(r["ok"], true);
    assert_eq!(r["user"], "testbot");
    assert_eq!(r["user_id"], bot_user.id.as_str());
    assert_eq!(r["bot_id"], bot_id.as_str());
    assert_eq!(r["app_id"], app_id.as_str());

    // Token as a form argument, the way older clients send it.
    let req = Request::post("/api/auth.test")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("token={}", bot.token)))
        .unwrap();
    assert_eq!(h.send(req).await.body["bot_id"], bot_id.as_str());

    let r = h.api_bearer("auth.test", &app.token).await.body;
    assert_eq!(r["error"], "not_allowed_token_type");

    let r = h.api_bearer("auth.test", &revoked.token).await.body;
    assert_eq!(r["error"], "invalid_auth");
}
