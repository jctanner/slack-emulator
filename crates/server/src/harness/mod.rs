//! The test harness (`/_emu/*`): reset, fixtures, call and delivery logs,
//! acting as a person, waiting for events, and rate-limit controls. It isn't
//! part of Slack's API. It's off unless `EMU_ADMIN_TOKEN` is set, and every
//! request must send that token as `Authorization: Bearer …`.

mod seed;

pub use seed::seed_file_on_empty;

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use slack_emu_core::ids::sha256_hex;
use slack_emu_core::models::{User, Workspace};

use crate::AppState;
use crate::api::{self, ApiError, ApiResult};
use crate::events::{Audience, BusEvent};

const API_LOG_SIZE: usize = 2000;
const DEFAULT_LIMIT: usize = 200;
const MAX_WAIT: Duration = Duration::from_secs(120);

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before 1970")
        .as_millis() as u64
}

/// Recent Web API calls, each with a `seq`.
#[derive(Default)]
pub struct ApiLog {
    inner: Mutex<(u64, VecDeque<Value>)>,
}

impl ApiLog {
    pub fn record(&self, mut entry: Value) {
        let mut inner = self.inner.lock().unwrap();
        inner.0 += 1;
        entry["seq"] = json!(inner.0);
        inner.1.push_back(entry);
        if inner.1.len() > API_LOG_SIZE {
            inner.1.pop_front();
        }
    }

    /// Entries after `after`, oldest first.
    pub fn since(&self, after: u64) -> Vec<Value> {
        let inner = self.inner.lock().unwrap();
        inner
            .1
            .iter()
            .filter(|e| e["seq"].as_u64().is_some_and(|s| s > after))
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().1.clear();
    }
}

pub fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/reset", post(reset))
        .route("/seed", post(seed::seed))
        .route("/seq", get(seq))
        .route("/api-log", get(api_log))
        .route("/deliveries", get(deliveries))
        .route("/events", get(events))
        .route("/wait", get(wait))
        .route("/as/{user}/{method}", post(act_as))
        .route("/rate-limits", get(rate_limits).post(set_rate_limits))
        .route_layer(middleware::from_fn_with_state(state, require_admin))
}

fn error(status: StatusCode, code: &str, message: impl Into<String>) -> ApiError {
    ApiError::new(status, code).with("message", json!(message.into()))
}

async fn require_admin(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let Some(expected) = &state.config.admin_token else {
        return error(
            StatusCode::NOT_FOUND,
            "harness_disabled",
            "Set EMU_ADMIN_TOKEN to turn on the test harness.",
        )
        .into_response();
    };
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    // Comparing digests keeps the comparison time independent of the token.
    if sha256_hex(given) != sha256_hex(expected) {
        return error(
            StatusCode::UNAUTHORIZED,
            "invalid_auth",
            "Send EMU_ADMIN_TOKEN as a bearer token.",
        )
        .into_response();
    }
    next.run(req).await
}

/// `POST /_emu/reset`: deletes everything, then recreates the default workspace.
async fn reset(State(state): State<AppState>) -> ApiResult<axum::Json<Value>> {
    state.delivery.reset();
    state.interactions.reset();
    state.store.reset().await?;
    state.api_log.clear();
    state.bus.clear();
    state.rate_limits.reset();
    state.ensure_default_workspace().await?;
    let ws = state
        .store
        .get_workspace_by_domain(&state.config.default_workspace_domain)
        .await?;
    Ok(axum::Json(json!({
        "ok": true,
        "team": ws.as_ref().map(api::shapes::team),
        "seq": state.bus.last_seq(),
    })))
}

async fn seq(State(state): State<AppState>) -> axum::Json<Value> {
    axum::Json(json!({"ok": true, "seq": state.bus.last_seq()}))
}

type Params = Query<HashMap<String, String>>;

fn param_u64(q: &HashMap<String, String>, key: &str) -> ApiResult<Option<u64>> {
    q.get(key)
        .map(|v| {
            v.parse().map_err(|_| {
                error(
                    StatusCode::BAD_REQUEST,
                    "invalid_arguments",
                    format!("{key} must be a number"),
                )
            })
        })
        .transpose()
}

/// Applies `after` / `limit`: with `after`, the oldest entries after it (for
/// polling); without, the most recent ones. Both oldest first.
fn page(mut items: Vec<Value>, q: &HashMap<String, String>) -> ApiResult<Vec<Value>> {
    let limit = param_u64(q, "limit")?.map_or(DEFAULT_LIMIT, |n| n as usize);
    if q.contains_key("after") {
        items.truncate(limit);
    } else if items.len() > limit {
        items.drain(..items.len() - limit);
    }
    Ok(items)
}

fn field_matches(entry: &Value, key: &str, want: &str) -> bool {
    match &entry[key] {
        Value::String(s) => s == want,
        Value::Null => false,
        // Numbers and booleans: `?status=429`, `?ok=false`.
        v => serde_json::from_str::<Value>(want).is_ok_and(|w| w == *v),
    }
}

/// `GET /_emu/api-log`: Web API calls. Filters: `method`, `app_id`, `team_id`,
/// `user_id`, `via` (token, session, harness), `errors=true`.
async fn api_log(State(state): State<AppState>, Query(q): Params) -> ApiResult<axum::Json<Value>> {
    let after = param_u64(&q, "after")?.unwrap_or(0);
    let items = state
        .api_log
        .since(after)
        .into_iter()
        .filter(|e| {
            ["method", "app_id", "team_id", "user_id", "via"]
                .iter()
                .all(|k| q.get(*k).is_none_or(|v| v.split(',').any(|v| field_matches(e, k, v))))
                && (q.get("errors").map(String::as_str) != Some("true") || e["ok"] == false)
        })
        .collect();
    Ok(axum::Json(json!({"ok": true, "entries": page(items, &q)?})))
}

/// `GET /_emu/deliveries`: events, interactions and commands sent to apps,
/// with each attempt's outcome. Filters: `app_id`, `label`, `errors=true`.
async fn deliveries(State(state): State<AppState>, Query(q): Params) -> ApiResult<axum::Json<Value>> {
    let after = param_u64(&q, "after")?.unwrap_or(0);
    let items = state
        .delivery
        .since(after, None)
        .into_iter()
        .filter(|e| {
            ["app_id", "label"]
                .iter()
                .all(|k| q.get(*k).is_none_or(|v| field_matches(e, k, v)))
                && (q.get("errors").map(String::as_str) != Some("true") || e["ok"] == false)
        })
        .collect();
    Ok(axum::Json(json!({"ok": true, "deliveries": page(items, &q)?})))
}

/// Event filters for `/_emu/events` and `/_emu/wait`. Any other parameter
/// matches a field of the event, or of its `message` (edits, ephemerals).
fn event_matches(ev: &BusEvent, q: &HashMap<String, String>) -> bool {
    let field = |k: &str| -> Option<String> {
        let v = match ev.event.get(k) {
            Some(v) if !v.is_null() => v,
            _ => ev.event.get("message")?.get(k).filter(|v| !v.is_null())?,
        };
        Some(v.as_str().map_or_else(|| v.to_string(), str::to_owned))
    };
    q.iter().all(|(k, want)| match k.as_str() {
        "after" | "timeout" | "limit" => true,
        "team" => ev.team_id == *want,
        "to" => match &ev.audience {
            Audience::Team => true,
            Audience::Users(ids) => ids.iter().any(|id| id == want),
        },
        "contains" => field("text").is_some_and(|t| t.contains(want.as_str())),
        "subtype" if want == "none" => field("subtype").is_none(),
        _ => field(k).is_some_and(|v| v == *want),
    })
}

fn event_json(ev: &BusEvent) -> Value {
    json!({
        "seq": ev.seq,
        "team_id": ev.team_id,
        "audience": match &ev.audience {
            Audience::Team => json!("team"),
            Audience::Users(ids) => json!(ids),
        },
        "ui_only": ev.ui_only,
        "event": ev.event,
    })
}

/// `GET /_emu/events`: recent events (everything the web UI and apps hear about).
async fn events(State(state): State<AppState>, Query(q): Params) -> ApiResult<axum::Json<Value>> {
    let after = param_u64(&q, "after")?.unwrap_or(0);
    let items = state
        .bus
        .since(after)
        .iter()
        .filter(|e| event_matches(e, &q))
        .map(|e| event_json(e))
        .collect();
    Ok(axum::Json(
        json!({"ok": true, "events": page(items, &q)?, "seq": state.bus.last_seq()}),
    ))
}

/// `GET /_emu/wait`: the first event after `after` matching the filters,
/// waiting up to `timeout` seconds (default 10) for one. 408 on timeout.
async fn wait(State(state): State<AppState>, Query(q): Params) -> ApiResult<Response> {
    let timeout = param_u64(&q, "timeout")?.map_or(Duration::from_secs(10), Duration::from_secs);
    let deadline = tokio::time::Instant::now() + timeout.min(MAX_WAIT);
    // Subscribe before looking, so nothing published in between is missed.
    let mut rx = state.bus.subscribe();
    let mut checked = param_u64(&q, "after")?.unwrap_or(0);
    loop {
        for ev in state.bus.since(checked) {
            if event_matches(&ev, &q) {
                return Ok(axum::Json(json!({"ok": true, "seq": ev.seq, "event": event_json(&ev)})).into_response());
            }
            checked = ev.seq;
        }
        // Any new event (or a lag notice) means: look again.
        if tokio::time::timeout_at(deadline, rx.recv()).await.is_err() {
            let body = json!({"ok": false, "error": "timeout", "seq": state.bus.last_seq()});
            return Ok((StatusCode::REQUEST_TIMEOUT, axum::Json(body)).into_response());
        }
    }
}

/// Finds a person by ID, email or handle, optionally within one workspace (ID or domain).
async fn find_person(state: &AppState, who: &str, team: Option<&str>) -> ApiResult<(Workspace, User)> {
    let mut found = Vec::new();
    for ws in state.store.list_workspaces().await? {
        if team.is_some_and(|t| t != ws.id && t != ws.domain) {
            continue;
        }
        for u in state.store.list_users(&ws.id).await? {
            let hit = u.id == who || u.name == who || u.email.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(who));
            if hit && !u.deleted {
                found.push((ws.clone(), u));
            }
        }
    }
    match found.len() {
        0 => Err(error(
            StatusCode::NOT_FOUND,
            "user_not_found",
            format!("no user {who:?}"),
        )),
        1 => {
            let (ws, u) = found.pop().expect("one");
            if u.is_bot {
                return Err(error(
                    StatusCode::BAD_REQUEST,
                    "cannot_act_as_bot",
                    "Bots act through their own tokens.",
                ));
            }
            Ok((ws, u))
        }
        _ => Err(error(
            StatusCode::BAD_REQUEST,
            "ambiguous_user",
            format!("{who:?} is in several workspaces; add ?team=<id or domain>"),
        )),
    }
}

/// `POST /_emu/as/{user}/{method}`: calls a Web API method as a person, with
/// the web UI's permissions (so `emu.*` and `chat.command` work too). The
/// response is the method's; `X-Emu-Seq` is the event seq just before the
/// call, to pass to `/_emu/wait?after=`.
async fn act_as(
    State(state): State<AppState>,
    Path((who, method)): Path<(String, String)>,
    Query(q): Params,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Response> {
    let (ws, user) = find_person(&state, &who, q.get("team").map(String::as_str)).await?;
    let mut args = api::parse_args(&headers, None, &body)?;
    for (k, v) in q.into_iter().filter(|(k, _)| k != "team") {
        args.entry(k).or_insert(Value::String(v));
    }
    let seq = state.bus.last_seq();
    let mut res = api::call_as(&state, ws, user, &method, args).await;
    res.headers_mut().insert("x-emu-seq", HeaderValue::from(seq));
    Ok(res)
}

async fn rate_limits(State(state): State<AppState>) -> axum::Json<Value> {
    let mut status = state.rate_limits.status();
    status["ok"] = json!(true);
    axum::Json(status)
}

/// `POST /_emu/rate-limits`: `{enabled, methods: {name: per_minute|null}, force: {name: count|{count, retry_after}}}`.
async fn set_rate_limits(State(state): State<AppState>, body: Bytes) -> ApiResult<axum::Json<Value>> {
    let v: Value = serde_json::from_slice(&body)
        .map_err(|_| error(StatusCode::BAD_REQUEST, "invalid_json", "expected a JSON object"))?;
    state
        .rate_limits
        .configure(&v)
        .map_err(|m| error(StatusCode::BAD_REQUEST, "invalid_arguments", m))?;
    Ok(rate_limits(State(state)).await)
}
