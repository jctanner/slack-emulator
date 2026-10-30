//! The Slack Web API: `/api/{method}`.
//!
//! Requests may be GET or POST, with arguments in the query string, a
//! form-encoded body, or a JSON body. Callers authenticate with a bearer token
//! (`Authorization: Bearer xoxb-...` or a `token` argument) or, for the web UI,
//! a session cookie plus an optional `X-Slack-Emu-Team` header.

pub(crate) mod apps;
pub mod args;
pub(crate) mod chat;
mod conversations;
pub(crate) mod files;
mod inspector;
mod interactive;
mod methods;
mod pins;
mod reactions;
mod scheduled;
pub mod shapes;
mod usergroups;
mod users;
mod views;

use std::time::Instant;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum_extra::extract::CookieJar;
use serde_json::{Map, Value, json};
use slack_emu_core::SlackError;
use slack_emu_core::ids::TokenKind;
use slack_emu_core::models::{ApiToken, Conversation, User, Workspace};
use slack_emu_store::StoreError;

use crate::{AppState, auth};

/// Header the web UI uses to pick which workspace a session-authenticated call acts in.
pub const TEAM_HEADER: &str = "x-slack-emu-team";

pub type Args = Map<String, Value>;

pub use chat::resolve_permalink;

/// An error rendered in Slack's envelope: `{"ok": false, "error": "<code>"}`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: String,
    /// Extra top-level fields, e.g. `needed`/`provided` for `missing_scope`.
    pub extra: Map<String, Value>,
    /// Seconds for a `Retry-After` header (rate limits).
    pub retry_after: Option<u64>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<String>) -> Self {
        Self {
            status,
            code: code.into(),
            extra: Map::new(),
            retry_after: None,
        }
    }

    /// `ratelimited` (HTTP 429) with `Retry-After`.
    pub fn ratelimited(retry_after: u64) -> Self {
        let mut e = Self::new(StatusCode::TOO_MANY_REQUESTS, "ratelimited");
        e.retry_after = Some(retry_after);
        e
    }

    /// The JSON body this error renders as.
    pub fn body(&self) -> Value {
        let mut body = Map::new();
        body.insert("ok".into(), Value::Bool(false));
        body.insert("error".into(), Value::String(self.code.clone()));
        body.extend(self.extra.clone());
        Value::Object(body)
    }

    pub fn with(mut self, key: &str, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// `invalid_arguments` with a human-readable explanation in `response_metadata.messages`.
    pub fn invalid_arguments(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::OK, "invalid_arguments").with(
            "response_metadata",
            json!({"messages": [format!("[ERROR] {}", detail.into())]}),
        )
    }
}

impl From<SlackError> for ApiError {
    fn from(e: SlackError) -> Self {
        // Slack reports most errors with HTTP 200.
        Self::new(StatusCode::OK, e.code)
    }
}

impl From<StoreError> for ApiError {
    fn from(e: StoreError) -> Self {
        tracing::error!("store error: {e}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut res = (self.status, axum::Json(self.body())).into_response();
        if let Some(secs) = self.retry_after {
            res.headers_mut().insert(header::RETRY_AFTER, secs.into());
        }
        res
    }
}

pub type ApiResult<T = Value> = Result<T, ApiError>;

/// Who is making a Web API call.
#[derive(Debug, Clone)]
pub struct Caller {
    pub team: Workspace,
    /// The acting user: a human (session or `xoxp-`) or bot user (`xoxb-`).
    pub user: Option<User>,
    /// Absent for session-cookie calls from the web UI.
    pub token: Option<ApiToken>,
}

impl Caller {
    pub fn token_kind(&self) -> Option<TokenKind> {
        self.token.as_ref().map(|t| t.kind)
    }

    /// The acting user. Every caller except an `xapp-` token has one.
    pub fn user(&self) -> ApiResult<&User> {
        self.user
            .as_ref()
            .ok_or_else(|| SlackError::new("not_allowed_token_type").into())
    }

    pub fn is_bot(&self) -> bool {
        self.token_kind() == Some(TokenKind::Bot)
    }

    /// True for the web UI (session cookie) rather than an API token.
    pub fn is_session(&self) -> bool {
        self.token.is_none()
    }

    /// Requires one of `any_of` on the token. Session callers have every permission.
    pub fn require_scope(&self, any_of: &[&str]) -> ApiResult<()> {
        let Some(token) = &self.token else { return Ok(()) };
        if any_of.is_empty() || any_of.iter().any(|s| token.has_scope(s)) {
            return Ok(());
        }
        Err(ApiError::from(SlackError::new("missing_scope"))
            .with("needed", json!(any_of.join(",")))
            .with("provided", json!(token.scopes.join(","))))
    }

    /// Requires the scope for this kind of access to this kind of conversation.
    pub fn require_conv_scope(&self, conv: &Conversation, access: ConvAccess) -> ApiResult<()> {
        let kind = match conv.channel_type() {
            "channel" => 0,
            "group" => 1,
            "im" => 2,
            _ => 3,
        };
        let table: [&[&str]; 4] = match access {
            ConvAccess::Read => [&["channels:read"], &["groups:read"], &["im:read"], &["mpim:read"]],
            ConvAccess::History => [
                &["channels:history"],
                &["groups:history"],
                &["im:history"],
                &["mpim:history"],
            ],
            ConvAccess::Manage => [
                &["channels:manage", "channels:write"],
                &["groups:write"],
                &["im:write"],
                &["mpim:write"],
            ],
            ConvAccess::Invite => [
                &["channels:manage", "channels:write", "channels:write.invites"],
                &["groups:write", "groups:write.invites"],
                &["im:write"],
                &["mpim:write"],
            ],
            ConvAccess::Topic => [
                &["channels:manage", "channels:write", "channels:write.topic"],
                &["groups:write", "groups:write.topic"],
                &["im:write", "im:write.topic"],
                &["mpim:write", "mpim:write.topic"],
            ],
        };
        self.require_scope(table[kind])
    }
}

/// What a method does to a conversation, for scope checks.
#[derive(Debug, Clone, Copy)]
pub enum ConvAccess {
    Read,
    History,
    Manage,
    Invite,
    Topic,
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/{method}", any(dispatch))
}

async fn dispatch(
    State(state): State<AppState>,
    Path(method): Path<String>,
    headers: HeaderMap,
    jar: CookieJar,
    RawQuery(query): RawQuery,
    body: Bytes,
) -> Response {
    let started = Instant::now();
    let mut seen_args = Args::new();
    let mut seen_caller = None;
    let result = async {
        let Some(spec) = methods::lookup(&method) else {
            return Err(ApiError::new(StatusCode::NOT_FOUND, "unknown_method"));
        };
        let args = parse_args(&headers, query.as_deref(), &body)?;
        seen_args = args.clone();
        let caller = if spec.callers != methods::Callers::Anyone {
            Some(
                resolve_caller(&state, &headers, &jar, &args)
                    .await?
                    .ok_or_else(SlackError::not_authed)?,
            )
        } else {
            None
        };
        seen_caller = caller.clone();
        run(&state, &method, caller, args).await
    }
    .await;
    // Logged even when the token was rejected, so `invalid_auth` is easy to trace.
    let presented = presented_token(&headers, &seen_args).map(redact);
    let via = match &seen_caller {
        Some(c) if c.token.is_some() => "token",
        Some(_) => "session",
        None if presented.is_some() => "token",
        None => "anonymous",
    };
    let call = LoggedCall {
        method: &method,
        via,
        caller: seen_caller.as_ref(),
        presented,
    };
    log_call(&state, call, seen_args, &result, started);
    match result {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Runs a method for an authenticated caller, applying rate limits to API tokens.
pub(crate) async fn run(state: &AppState, method: &str, caller: Option<Caller>, args: Args) -> ApiResult {
    if let Some(c) = &caller
        && let Some(token) = &c.token
    {
        let who = token
            .app_id
            .as_deref()
            .or(token.user_id.as_deref())
            .unwrap_or(&token.token);
        let channel = (method == "chat.postMessage")
            .then(|| args.get("channel").and_then(Value::as_str))
            .flatten();
        if let Err(secs) = state
            .rate_limits
            .check(method, &format!("{}/{who}", c.team.id), channel)
        {
            return Err(ApiError::ratelimited(secs));
        }
    }
    methods::call(state, method, caller, args).await
}

/// Runs a method as a person, with the web UI's permissions (the harness's `/_emu/as`).
pub(crate) async fn call_as(state: &AppState, team: Workspace, user: User, method: &str, args: Args) -> Response {
    let started = Instant::now();
    let caller = Caller {
        team,
        user: Some(user),
        token: None,
    };
    let result = match methods::lookup(method) {
        None => Err(ApiError::new(StatusCode::NOT_FOUND, "unknown_method")),
        Some(_) => run(state, method, Some(caller.clone()), args.clone()).await,
    };
    let call = LoggedCall {
        method,
        via: "harness",
        caller: Some(&caller),
        presented: None,
    };
    log_call(state, call, args, &result, started);
    match result {
        Ok(v) => axum::Json(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Runs a method directly, without rate limits or logging (fixtures).
pub(crate) async fn invoke(state: &AppState, method: &str, caller: Caller, args: Args) -> ApiResult {
    methods::call(state, method, Some(caller), args).await
}

/// The API token sent with a request (`Authorization: Bearer` or a `token` argument).
fn presented_token<'a>(headers: &'a HeaderMap, args: &'a Args) -> Option<&'a str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .or_else(|| args.get("token").and_then(Value::as_str))
}

/// Larger responses are summarised in the call log.
const MAX_LOGGED_RESPONSE: usize = 64 * 1024;

/// Who made a call, for the call log.
struct LoggedCall<'a> {
    method: &'a str,
    /// `token`, `session`, `harness` or `anonymous`.
    via: &'a str,
    caller: Option<&'a Caller>,
    /// The redacted token sent, kept even when it was rejected.
    presented: Option<String>,
}

fn log_call(state: &AppState, call: LoggedCall, mut args: Args, result: &ApiResult, started: Instant) {
    let LoggedCall {
        method,
        via,
        caller,
        presented,
    } = call;
    // The inspector's own polling would drown out everything else.
    if method.starts_with("emu.inspector.") {
        return;
    }
    args.remove("token");
    let (status, response) = match result {
        Ok(v) => (StatusCode::OK, v.clone()),
        Err(e) => (e.status, e.body()),
    };
    let size = response.to_string().len();
    let token = caller.and_then(|c| c.token.as_ref());
    state.api_log.record(json!({
        "time": crate::harness::now_ms(),
        "method": method,
        "via": via,
        "team_id": caller.map(|c| c.team.id.as_str()),
        "user_id": caller.and_then(|c| c.user.as_ref()).map(|u| u.id.as_str()),
        "app_id": token.and_then(|t| t.app_id.as_deref()),
        "token_type": token.map(|t| t.kind.as_str()),
        "token": token.map(|t| redact(&t.token)).or(presented),
        "args": args,
        "status": status.as_u16(),
        "ok": result.is_ok(),
        "error": result.as_ref().err().map(|e| e.code.as_str()),
        "response": if size > MAX_LOGGED_RESPONSE { json!({"_truncated_bytes": size}) } else { response },
        "latency_ms": started.elapsed().as_millis() as u64,
    }));
}

/// `xoxb-…a1b2`: enough to tell tokens apart without logging them whole.
fn redact(token: &str) -> String {
    let prefix = token.split('-').next().unwrap_or("");
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{prefix}-…{tail}")
}

/// Merges query-string and body arguments. Form values stay strings, as in Slack.
pub(crate) fn parse_args(headers: &HeaderMap, query: Option<&str>, body: &[u8]) -> ApiResult<Args> {
    let mut args = Args::new();
    let mut add_form = |bytes: &[u8]| -> ApiResult<()> {
        let pairs: Vec<(String, String)> =
            serde_urlencoded::from_bytes(bytes).map_err(|_| SlackError::new("invalid_form_data"))?;
        for (k, v) in pairs {
            args.insert(k, Value::String(v));
        }
        Ok(())
    };
    if let Some(q) = query {
        add_form(q.as_bytes())?;
    }
    if body.is_empty() {
        return Ok(args);
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("application/json") {
        match serde_json::from_slice::<Value>(body) {
            Ok(Value::Object(obj)) => args.extend(obj),
            _ => return Err(SlackError::new("invalid_json").into()),
        }
    } else {
        add_form(body)?;
    }
    Ok(args)
}

pub(crate) async fn resolve_caller(
    state: &AppState,
    headers: &HeaderMap,
    jar: &CookieJar,
    args: &Args,
) -> ApiResult<Option<Caller>> {
    if let Some(token) = presented_token(headers, args) {
        let token = match state.store.get_token(token).await? {
            Some(t) if !t.revoked => t,
            _ => return Err(SlackError::invalid_auth().into()),
        };
        let team = state
            .store
            .get_workspace(&token.team_id)
            .await?
            .ok_or_else(SlackError::invalid_auth)?;
        let user = match &token.user_id {
            Some(id) => Some(state.store.get_user(id).await?.ok_or_else(SlackError::invalid_auth)?),
            None => None,
        };
        return Ok(Some(Caller {
            team,
            user,
            token: Some(token),
        }));
    }

    let Some(account) = auth::session_account(state, jar).await? else {
        return Ok(None);
    };
    let requested = headers
        .get(TEAM_HEADER)
        .and_then(|v| v.to_str().ok())
        .or_else(|| args.get("team_id").and_then(Value::as_str));
    let user = match requested {
        Some(team_id) => state.store.user_for_account(team_id, &account.id).await?,
        // Without an explicit team, act in the account's first workspace.
        None => state.store.users_for_account(&account.id).await?.into_iter().next(),
    };
    let Some(user) = user.filter(|u| !u.deleted) else {
        return Err(SlackError::invalid_auth().into());
    };
    let team = state
        .store
        .get_workspace(&user.team_id)
        .await?
        .ok_or_else(SlackError::invalid_auth)?;
    Ok(Some(Caller {
        team,
        user: Some(user),
        token: None,
    }))
}
