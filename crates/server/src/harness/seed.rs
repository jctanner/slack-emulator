//! `POST /_emu/seed`: loads a YAML or JSON fixture of workspaces, people,
//! apps (with fixed tokens and secrets), channels and messages. Seeding is
//! additive: existing workspaces (by domain), users (by handle) and channels
//! (by name) are reused, so a fixture can extend the default workspace.
//!
//! Everything except account creation goes through the same Web API methods
//! the UI uses, so seeded data looks exactly like data made by hand.

use std::collections::{BTreeMap, HashMap};

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use slack_emu_core::ids::{TokenKind, new_api_token};
use slack_emu_core::models::{ApiToken, NewUser, User, Workspace};
use slack_emu_core::now;
use slack_emu_store::StoreError;

use super::error;
use crate::api::apps::{FixedCredentials, create_app, credentials};
use crate::api::{ApiError, ApiResult, Args, Caller, invoke};
use crate::{AppState, auth, services};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    #[serde(default)]
    workspaces: Vec<WorkspaceFx>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceFx {
    /// The default workspace's domain (`emulator`) extends it.
    domain: String,
    name: Option<String>,
    #[serde(default)]
    users: Vec<UserFx>,
    #[serde(default)]
    apps: Vec<AppFx>,
    #[serde(default)]
    channels: Vec<ChannelFx>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UserFx {
    /// The handle (`@alice`).
    name: String,
    real_name: Option<String>,
    display_name: Option<String>,
    email: Option<String>,
    /// With an email, lets the person sign in to the web UI.
    password: Option<String>,
    #[serde(default)]
    admin: bool,
    #[serde(default)]
    owner: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppFx {
    manifest: Value,
    id: Option<String>,
    /// Fixed `xoxb-` token, so tests can hardcode it.
    bot_token: Option<String>,
    /// Fixed `xapp-` token. Socket Mode apps get one either way.
    app_token: Option<String>,
    signing_secret: Option<String>,
    verification_token: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    /// Who creates and installs it (a handle); defaults to the first owner or admin.
    installer: Option<String>,
    #[serde(default = "yes")]
    install: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelFx {
    name: String,
    #[serde(default)]
    private: bool,
    /// Handles, or an app's name for its bot.
    #[serde(default)]
    members: Vec<String>,
    creator: Option<String>,
    topic: Option<String>,
    purpose: Option<String>,
    #[serde(default)]
    messages: Vec<MessageFx>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageFx {
    user: String,
    #[serde(default)]
    text: String,
    blocks: Option<Value>,
    /// Emoji name → who reacted.
    #[serde(default)]
    reactions: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    replies: Vec<MessageFx>,
}

/// Parses JSON, or YAML when the body isn't JSON.
fn parse(headers: &HeaderMap, body: &[u8]) -> ApiResult<Fixture> {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json"));
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) if is_json => return Err(error(StatusCode::BAD_REQUEST, "invalid_fixture", e.to_string())),
        Err(_) => {
            let text = std::str::from_utf8(body)
                .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_fixture", "the body isn't UTF-8"))?;
            serde_saphyr::from_str(text)
                .map_err(|e| error(StatusCode::BAD_REQUEST, "invalid_fixture", e.to_string()))?
        }
    };
    serde_json::from_value(value).map_err(|e| error(StatusCode::BAD_REQUEST, "invalid_fixture", e.to_string()))
}

pub(super) async fn seed(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<axum::Json<Value>> {
    let fixture = parse(&headers, &body)?;
    let out = apply(&state, fixture).await?;
    Ok(axum::Json(
        json!({"ok": true, "workspaces": out, "seq": state.bus.last_seq()}),
    ))
}

async fn apply(state: &AppState, fixture: Fixture) -> ApiResult<Map<String, Value>> {
    let mut out = Map::new();
    for ws in fixture.workspaces {
        let domain = ws.domain.clone();
        let result = Seeder::new(state, &ws).await?.run(ws).await?;
        out.insert(domain, result);
    }
    Ok(out)
}

/// `EMU_SEED_FILE`: loads a fixture at startup if nobody has signed up or
/// been seeded yet, so a fresh container comes up ready to use.
pub async fn seed_file_on_empty(state: &AppState, path: &str) -> anyhow::Result<()> {
    for ws in state.store.list_workspaces().await? {
        if !state.store.list_users(&ws.id).await?.is_empty() {
            tracing::info!("EMU_SEED_FILE: the database already has users; not seeding");
            return Ok(());
        }
    }
    let body = std::fs::read(path).map_err(|e| anyhow::anyhow!("reading EMU_SEED_FILE {path}: {e}"))?;
    let describe = |e: ApiError| {
        let message = e
            .extra
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or(&e.code)
            .to_owned();
        anyhow::anyhow!("EMU_SEED_FILE {path}: {message}")
    };
    let fixture = parse(&HeaderMap::new(), &body).map_err(describe)?;
    let out = apply(state, fixture).await.map_err(describe)?;
    tracing::info!("EMU_SEED_FILE: seeded {} workspace(s) from {path}", out.len());
    Ok(())
}

/// Adds what went wrong to an error's message.
fn context(e: ApiError, what: &str) -> ApiError {
    let detail = e
        .extra
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or(&e.code)
        .to_owned();
    error(StatusCode::BAD_REQUEST, "seed_failed", format!("{what}: {detail}")).with("cause", json!(e.body()))
}

fn args(v: Value) -> Args {
    match v {
        Value::Object(m) => m,
        _ => Args::new(),
    }
}

struct Seeder<'a> {
    state: &'a AppState,
    ws: Workspace,
    /// Handles (and app names) → users, for this workspace.
    people: HashMap<String, User>,
    /// Bot user ID → its bot token.
    bot_tokens: HashMap<String, ApiToken>,
}

impl<'a> Seeder<'a> {
    async fn new(state: &'a AppState, fx: &WorkspaceFx) -> ApiResult<Self> {
        let ws = match state.store.get_workspace_by_domain(&fx.domain).await? {
            Some(ws) => ws,
            None => {
                if !auth::valid_domain(&fx.domain) {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "seed_failed",
                        format!(
                            "workspace domain {:?}: use lowercase letters, numbers and dashes",
                            fx.domain
                        ),
                    ));
                }
                let name = fx.name.clone().unwrap_or_else(|| fx.domain.clone());
                let ws = state.store.create_workspace(&name, &fx.domain).await?;
                services::ensure_workspace_defaults(state, &ws.id).await?;
                ws
            }
        };
        Ok(Self {
            state,
            ws,
            people: HashMap::new(),
            bot_tokens: HashMap::new(),
        })
    }

    async fn run(mut self, fx: WorkspaceFx) -> ApiResult<Value> {
        let mut users = Map::new();
        for u in &fx.users {
            let user = self
                .user(u)
                .await
                .map_err(|e| context(e, &format!("user {}", u.name)))?;
            users.insert(u.name.clone(), json!(user.id));
        }
        let mut apps = Map::new();
        for a in fx.apps {
            let name = a.manifest["display_information"]["name"]
                .as_str()
                .unwrap_or("app")
                .to_owned();
            let result = self
                .app(a, &fx.users)
                .await
                .map_err(|e| context(e, &format!("app {name}")))?;
            apps.insert(name, result);
        }
        let mut channels = Map::new();
        for c in &fx.channels {
            let id = self
                .channel(c)
                .await
                .map_err(|e| context(e, &format!("channel #{}", c.name)))?;
            channels.insert(c.name.clone(), json!(id));
        }
        Ok(json!({
            "id": self.ws.id,
            "name": self.ws.name,
            "users": users,
            "apps": apps,
            "channels": channels,
        }))
    }

    /// Creates (or reuses) a person, with a sign-in account if they have a password.
    async fn user(&mut self, u: &UserFx) -> ApiResult<User> {
        let store = &self.state.store;
        if let Some(existing) = store
            .list_users(&self.ws.id)
            .await?
            .into_iter()
            .find(|x| x.name == u.name)
        {
            self.people.insert(u.name.clone(), existing.clone());
            return Ok(existing);
        }
        let email = u.email.as_ref().map(|e| e.trim().to_ascii_lowercase());
        let real_name = u.real_name.clone().unwrap_or_else(|| u.name.clone());
        let account = match (&u.password, &email) {
            (Some(_), None) => {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "seed_failed",
                    "a password needs an email",
                ));
            }
            (Some(password), Some(email)) => match store.find_identity(auth::PASSWORD_PROVIDER, email).await? {
                Some(found) => Some(found.account),
                None => {
                    let hash = auth::hash_password(password.clone()).await?;
                    Some(
                        store
                            .create_account(email, &real_name, auth::PASSWORD_PROVIDER, email, Some(&hash))
                            .await?,
                    )
                }
            },
            (None, _) => None,
        };
        let new = NewUser {
            team_id: self.ws.id.clone(),
            account_id: account.map(|a| a.id),
            name: u.name.clone(),
            real_name: real_name.clone(),
            display_name: u.display_name.clone().unwrap_or(real_name),
            email,
            is_bot: false,
            is_admin: u.admin || u.owner,
            is_owner: u.owner,
        };
        let user = match store.create_user(new).await {
            Err(StoreError::Conflict(_)) => {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "seed_failed",
                    "that handle or email is already taken in this workspace",
                ));
            }
            other => other?,
        };
        services::on_workspace_join(self.state, &user).await?;
        self.people.insert(u.name.clone(), user.clone());
        Ok(user)
    }

    /// A handle (or app name) in this workspace.
    async fn person(&self, name: &str) -> ApiResult<User> {
        if let Some(u) = self.people.get(name) {
            return Ok(u.clone());
        }
        match self
            .state
            .store
            .list_users(&self.ws.id)
            .await?
            .into_iter()
            .find(|u| u.name == name && !u.deleted)
        {
            Some(u) => Ok(u),
            None => Err(error(
                StatusCode::BAD_REQUEST,
                "seed_failed",
                format!("no user or app named {name:?}"),
            )),
        }
    }

    /// Acting as `user`: through their bot token for bots, the UI's permissions for people.
    fn caller(&self, user: &User) -> Caller {
        Caller {
            team: self.ws.clone(),
            user: Some(user.clone()),
            token: self.bot_tokens.get(&user.id).cloned(),
        }
    }

    async fn call(&self, user: &User, method: &str, a: Value) -> ApiResult {
        invoke(self.state, method, self.caller(user), args(a)).await
    }

    async fn app(&mut self, a: AppFx, fixture_users: &[UserFx]) -> ApiResult {
        let store = &self.state.store;
        let installer = match &a.installer {
            Some(name) => self.person(name).await?,
            None => {
                let preferred = fixture_users
                    .iter()
                    .find(|u| u.owner || u.admin)
                    .or(fixture_users.first())
                    .map(|u| u.name.clone());
                match preferred {
                    Some(name) => self.person(&name).await?,
                    None => store
                        .list_users(&self.ws.id)
                        .await?
                        .into_iter()
                        .find(|u| !u.is_bot && !u.deleted)
                        .ok_or_else(|| {
                            error(
                                StatusCode::BAD_REQUEST,
                                "seed_failed",
                                "an app needs a person to install it",
                            )
                        })?,
                }
            }
        };
        for (token, prefix) in [(&a.bot_token, "xoxb-"), (&a.app_token, "xapp-")] {
            if let Some(t) = token {
                if !t.starts_with(prefix) {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "seed_failed",
                        format!("{t:?} must start with {prefix}"),
                    ));
                }
                if store.get_token(t).await?.is_some() {
                    return Err(error(
                        StatusCode::BAD_REQUEST,
                        "seed_failed",
                        format!("token {t} already exists"),
                    ));
                }
            }
        }
        let fixed = FixedCredentials {
            app_id: a.id,
            client_id: a.client_id,
            client_secret: a.client_secret,
            signing_secret: a.signing_secret,
            verification_token: a.verification_token,
        };
        let manifest_args = args(json!({"manifest": a.manifest}));
        let (app, url_error) = create_app(self.state, &self.ws.id, &installer.id, &manifest_args, fixed).await?;
        let mut out = json!({"app_id": app.id, "credentials": credentials(&app)});
        if let Some(e) = url_error {
            out["warning"] = json!(format!("request URL not verified: {e}"));
        }

        if a.install {
            let inst = self
                .call(&installer, "emu.apps.install", json!({"app_id": app.id}))
                .await?;
            if let (Some(generated), Some(bot_user)) = (inst["access_token"].as_str(), inst["bot_user_id"].as_str()) {
                let mut token = store
                    .get_token(generated)
                    .await?
                    .ok_or_else(|| ApiError::from(slack_emu_core::SlackError::internal()))?;
                if let Some(fixed) = &a.bot_token {
                    store.revoke_token(generated).await?;
                    token.token = fixed.clone();
                    store.create_token(&token).await?;
                }
                let bot = store
                    .get_user(bot_user)
                    .await?
                    .ok_or_else(|| ApiError::from(slack_emu_core::SlackError::internal()))?;
                out["bot_user_id"] = json!(bot.id);
                out["bot_id"] = json!(token.bot_id);
                out["bot_token"] = json!(token.token);
                // Channel members can name the bot by handle, app name or bot display name.
                self.people.insert(bot.name.clone(), bot.clone());
                self.people.insert(app.name.clone(), bot.clone());
                self.people.insert(bot.real_name.clone(), bot.clone());
                self.bot_tokens.insert(bot.id.clone(), token);
            }
            if let Some(user_token) = inst["authed_user"]["access_token"].as_str() {
                out["user_token"] = json!(user_token);
            }
        }
        if a.app_token.is_some() || app.manifest().settings.socket_mode_enabled {
            let token = ApiToken {
                token: a.app_token.unwrap_or_else(|| new_api_token(TokenKind::App)),
                kind: TokenKind::App,
                team_id: self.ws.id.clone(),
                user_id: None,
                app_id: Some(app.id.clone()),
                bot_id: None,
                scopes: vec!["connections:write".into()],
                created: now(),
                revoked: false,
                name: Some("seeded".into()),
            };
            store.create_token(&token).await?;
            out["app_token"] = json!(token.token);
        }
        Ok(out)
    }

    async fn channel(&self, c: &ChannelFx) -> ApiResult<String> {
        let store = &self.state.store;
        let mut members = Vec::new();
        for m in &c.members {
            members.push(self.person(m).await?);
        }
        let creator = match &c.creator {
            Some(name) => self.person(name).await?,
            None => match members.iter().find(|u| !u.is_bot) {
                Some(u) => u.clone(),
                None => store
                    .list_users(&self.ws.id)
                    .await?
                    .into_iter()
                    .find(|u| !u.is_bot && !u.deleted)
                    .ok_or_else(|| error(StatusCode::BAD_REQUEST, "seed_failed", "a channel needs a person"))?,
            },
        };
        let conv = match store.conversation_by_name(&self.ws.id, &c.name).await? {
            Some(conv) => {
                if store.member_last_read(&conv.id, &creator.id).await?.is_none() {
                    self.call(&creator, "conversations.join", json!({"channel": conv.id}))
                        .await?;
                }
                conv.id
            }
            None => {
                let r = self
                    .call(
                        &creator,
                        "conversations.create",
                        json!({"name": c.name, "is_private": c.private}),
                    )
                    .await?;
                r["channel"]["id"].as_str().unwrap_or_default().to_owned()
            }
        };
        let mut invite = Vec::new();
        for m in &members {
            if m.id != creator.id && store.member_last_read(&conv, &m.id).await?.is_none() {
                invite.push(m.id.clone());
            }
        }
        if !invite.is_empty() {
            self.call(
                &creator,
                "conversations.invite",
                json!({"channel": conv, "users": invite.join(",")}),
            )
            .await?;
        }
        if let Some(topic) = &c.topic {
            self.call(
                &creator,
                "conversations.setTopic",
                json!({"channel": conv, "topic": topic}),
            )
            .await?;
        }
        if let Some(purpose) = &c.purpose {
            self.call(
                &creator,
                "conversations.setPurpose",
                json!({"channel": conv, "purpose": purpose}),
            )
            .await?;
        }
        for m in &c.messages {
            self.message(&conv, m, None).await?;
        }
        Ok(conv)
    }

    async fn message(&self, channel: &str, m: &MessageFx, thread_ts: Option<&str>) -> ApiResult<()> {
        let author = self.person(&m.user).await?;
        let mut a = json!({"channel": channel, "text": m.text});
        if let Some(b) = &m.blocks {
            a["blocks"] = b.clone();
        }
        if let Some(t) = thread_ts {
            a["thread_ts"] = json!(t);
        }
        let posted = self.call(&author, "chat.postMessage", a).await?;
        let ts = posted["ts"].as_str().unwrap_or_default().to_owned();
        for (name, who) in &m.reactions {
            for w in who {
                let reactor = self.person(w).await?;
                self.call(
                    &reactor,
                    "reactions.add",
                    json!({"channel": channel, "timestamp": ts, "name": name}),
                )
                .await?;
            }
        }
        for reply in &m.replies {
            Box::pin(self.message(channel, reply, Some(&ts))).await?;
        }
        Ok(())
    }
}
