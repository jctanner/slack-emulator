//! Shared test harness: drives the router in-process against an in-memory database.

#![allow(dead_code)]

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use slack_emu_core::ids::{IdKind, TokenKind, new_api_token, new_id};
use slack_emu_core::manifest::BOT_SCOPES;
use slack_emu_core::models::{ApiToken, NewUser};
use slack_emu_core::now;
use slack_emu_server::{AppState, Config, router};
use tower::ServiceExt;

pub struct Harness {
    pub app: Router,
    pub state: AppState,
}

pub struct Resp {
    pub status: StatusCode,
    pub headers: axum::http::HeaderMap,
    pub cookie: Option<String>,
    pub location: Option<String>,
    pub body: Value,
}

impl Harness {
    pub async fn new() -> Self {
        Self::with_config(|_| {}).await
    }

    pub async fn with_config(tweak: impl FnOnce(&mut Config)) -> Self {
        // Set EMU_TEST_POSTGRES (a server URL) to run against Postgres; each harness gets its own database.
        let database_url = match std::env::var("EMU_TEST_POSTGRES").ok().filter(|s| !s.is_empty()) {
            Some(server) => slack_emu_store::fresh_test_database(&server).await.unwrap(),
            None => "sqlite::memory:".into(),
        };
        let mut config = Config {
            database_url,
            ..Config::default()
        };
        tweak(&mut config);
        let state = AppState::new(config).await.unwrap();
        Self {
            app: router(state.clone()),
            state,
        }
    }

    pub async fn send(&self, req: Request<Body>) -> Resp {
        let res = self.app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let header_str = |name| {
            res.headers()
                .get(name)
                .map(|v: &axum::http::HeaderValue| v.to_str().unwrap().to_owned())
        };
        let cookie = header_str(header::SET_COOKIE).map(|c| c.split(';').next().unwrap().to_owned());
        let location = header_str(header::LOCATION);
        let headers = res.headers().clone();
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Resp {
            status,
            headers,
            cookie,
            location,
            body,
        }
    }

    pub async fn post_json(&self, path: &str, cookie: Option<&str>, body: Value) -> Resp {
        let mut req = Request::post(path).header(header::CONTENT_TYPE, "application/json");
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        self.send(req.body(Body::from(body.to_string())).unwrap()).await
    }

    pub async fn get(&self, path: &str, cookie: Option<&str>) -> Resp {
        let mut req = Request::get(path);
        if let Some(c) = cookie {
            req = req.header(header::COOKIE, c);
        }
        self.send(req.body(Body::empty()).unwrap()).await
    }

    /// Signs up and returns the session cookie.
    pub async fn signup(&self, email: &str) -> String {
        let r = self
            .post_json(
                "/auth/signup",
                None,
                json!({"email": email, "password": "hunter2hunter2"}),
            )
            .await;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        r.cookie.expect("session cookie")
    }

    pub async fn api_bearer(&self, method: &str, token: &str) -> Resp {
        let req = Request::post(format!("/api/{method}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap();
        self.send(req).await
    }

    /// Calls a Web API method as a signed-in user (cookie) or with a token (`xox…`).
    pub async fn call(&self, who: &str, method: &str, args: Value) -> Value {
        let mut req = Request::post(format!("/api/{method}")).header(header::CONTENT_TYPE, "application/json");
        req = if who.starts_with("xox") || who.starts_with("xapp") {
            req.header(header::AUTHORIZATION, format!("Bearer {who}"))
        } else {
            req.header(header::COOKIE, who)
        };
        self.send(req.body(Body::from(args.to_string())).unwrap()).await.body
    }

    /// Like [`call`], asserting `ok: true`.
    pub async fn ok(&self, who: &str, method: &str, args: Value) -> Value {
        let r = self.call(who, method, args.clone()).await;
        assert_eq!(r["ok"], true, "{method}({args}) failed: {r}");
        r
    }

    /// Like [`call`], asserting the given error code.
    pub async fn err(&self, who: &str, method: &str, args: Value, code: &str) {
        let r = self.call(who, method, args.clone()).await;
        assert_eq!(r["error"], code, "{method}({args}) expected {code}: {r}");
    }

    pub async fn user_id(&self, who: &str) -> String {
        self.ok(who, "auth.test", json!({})).await["user_id"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Creates a bot user and a broadly-scoped `xoxb-` token in the default workspace.
    pub async fn bot(&self, name: &str) -> (String, String) {
        let store = &self.state.store;
        let ws = store.get_workspace_by_domain("emulator").await.unwrap().unwrap();
        let user = store
            .create_user(NewUser {
                team_id: ws.id.clone(),
                account_id: None,
                name: name.into(),
                real_name: name.into(),
                display_name: name.into(),
                email: None,
                is_bot: true,
                is_admin: false,
                is_owner: false,
            })
            .await
            .unwrap();
        let token = ApiToken {
            token: new_api_token(TokenKind::Bot),
            kind: TokenKind::Bot,
            team_id: ws.id,
            user_id: Some(user.id.clone()),
            app_id: Some(new_id(IdKind::App)),
            bot_id: Some(new_id(IdKind::Bot)),
            // Everything except chat:write.public, so membership rules still apply.
            scopes: BOT_SCOPES
                .iter()
                .filter(|s| **s != "chat:write.public")
                .map(|s| s.to_string())
                .collect(),
            created: now(),
            revoked: false,
            name: None,
        };
        store.create_token(&token).await.unwrap();
        (token.token, user.id)
    }
}
