//! Interactivity state: trigger IDs, response URLs, open modals and ephemeral
//! messages. Like Slack's, these handles are short-lived, so they live only in
//! memory. App Home views are stored (see [`save_view`]).
//!
//! Also serves response URLs (`/hooks/{kind}/{team}/{id}/{secret}`), which
//! apps POST to with `respond()` after an interaction.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use slack_emu_core::blocks::retain_state;
use slack_emu_core::ids::{IdKind, new_id, new_secret};
use slack_emu_core::models::{App, Conversation, Message, User, Workspace};
use slack_emu_core::{SlackError, now};

use crate::AppState;
use crate::api::chat;
use crate::api::{ApiError, ApiResult, Args, args, shapes};
use crate::events::{Audience, publish};
use crate::services;

/// Response URLs work for 30 minutes, up to 5 times.
const RESPONSE_URL_TTL: Duration = Duration::from_secs(30 * 60);
const RESPONSE_URL_USES: u32 = 5;
/// Ephemeral messages are remembered (for their buttons) about as long as a client keeps them.
const EPHEMERAL_TTL: Duration = Duration::from_secs(24 * 3600);
/// Slack allows three views in a modal stack.
pub const MAX_MODAL_STACK: usize = 3;
/// Used trigger IDs are kept a while so reuse reports `exchanged_trigger_id`.
const TRIGGER_MEMORY: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone)]
pub struct Trigger {
    pub team_id: String,
    pub user_id: String,
    pub app_id: String,
    issued: Instant,
    used: bool,
}

/// Where a response URL's messages go.
#[derive(Debug, Clone)]
pub struct ResponseTarget {
    pub team_id: String,
    pub app_id: String,
    pub user_id: String,
    pub channel_id: Option<String>,
    /// The message the interaction came from, for `replace_original` / `delete_original`.
    pub source: Option<Source>,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub ts: String,
    pub ephemeral: bool,
}

struct ResponseUrl {
    target: ResponseTarget,
    created: Instant,
    uses: u32,
}

#[derive(Debug, Clone)]
pub struct Ephemeral {
    pub app_id: Option<String>,
    pub user_id: String,
    /// The message as shown to the user.
    pub message: Value,
    created: Instant,
}

/// A modal or App Home view, as the server keeps it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct View {
    pub id: String,
    pub team_id: String,
    pub app_id: String,
    pub bot_id: Option<String>,
    /// Who the view is shown to.
    pub user_id: String,
    pub root_view_id: String,
    pub previous_view_id: Option<String>,
    pub hash: String,
    /// The view as the app sent it (validated, with block and action IDs filled in).
    pub definition: Value,
    /// `state.values`: what the user has entered so far.
    #[serde(default)]
    pub values: Map<String, Value>,
}

fn new_hash() -> String {
    format!("{}.{}", now(), &new_secret()[..8])
}

impl View {
    /// A new view; pushed views share the root of the view below them.
    pub fn new(app: &App, user_id: &str, definition: Value, below: Option<&View>) -> Self {
        let id = new_id(IdKind::View);
        Self {
            root_view_id: below.map_or_else(|| id.clone(), |b| b.root_view_id.clone()),
            previous_view_id: below.map(|b| b.id.clone()),
            id,
            team_id: app.team_id.clone(),
            app_id: app.id.clone(),
            bot_id: app.bot_id.clone(),
            user_id: user_id.to_owned(),
            hash: new_hash(),
            definition,
            values: Map::new(),
        }
    }

    pub fn is_home(&self) -> bool {
        self.definition["type"] == "home"
    }

    pub fn external_id(&self) -> Option<&str> {
        self.definition["external_id"].as_str().filter(|s| !s.is_empty())
    }

    fn flag(&self, key: &str) -> bool {
        self.definition[key].as_bool().unwrap_or(false)
    }

    /// `views.update`: new content, keeping entered values that still apply.
    pub fn replace(&mut self, definition: Value) {
        retain_state(&mut self.values, &definition["blocks"]);
        self.definition = definition;
        self.hash = new_hash();
    }

    /// The view object apps see.
    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "id": self.id,
            "team_id": self.team_id,
            "type": self.definition["type"],
            "blocks": self.definition["blocks"],
            "private_metadata": self.definition["private_metadata"].as_str().unwrap_or(""),
            "callback_id": self.definition["callback_id"].as_str().unwrap_or(""),
            "state": {"values": self.values},
            "hash": self.hash,
            "title": self.definition["title"],
            "clear_on_close": self.flag("clear_on_close"),
            "notify_on_close": self.flag("notify_on_close"),
            "close": self.definition["close"],
            "submit": self.definition["submit"],
            "previous_view_id": self.previous_view_id,
            "root_view_id": self.root_view_id,
            "app_id": self.app_id,
            "external_id": self.external_id().unwrap_or(""),
            "app_installed_team_id": self.team_id,
            "bot_id": self.bot_id,
        });
        if let Some(disabled) = self.definition.get("submit_disabled") {
            v["submit_disabled"] = disabled.clone();
        }
        v
    }
}

#[derive(Default)]
struct Inner {
    triggers: HashMap<String, Trigger>,
    responses: HashMap<String, ResponseUrl>,
    /// User ID → their modal stack, bottom first.
    modals: HashMap<String, Vec<View>>,
    /// `{channel}/{ts}` → ephemeral message.
    ephemerals: HashMap<String, Ephemeral>,
}

#[derive(Default)]
pub struct Interactions {
    inner: Mutex<Inner>,
}

impl Interactions {
    fn issue_trigger(&self, team_id: &str, user_id: &str, app_id: &str) -> String {
        let secret = new_secret().to_lowercase();
        let id = format!(
            "{}.{}.{}",
            &digits(&secret, 13),
            &digits(&secret[13..], 12),
            &secret[..32]
        );
        let mut inner = self.inner.lock().unwrap();
        inner.triggers.retain(|_, t| t.issued.elapsed() < TRIGGER_MEMORY);
        inner.triggers.insert(
            id.clone(),
            Trigger {
                team_id: team_id.into(),
                user_id: user_id.into(),
                app_id: app_id.into(),
                issued: Instant::now(),
                used: false,
            },
        );
        id
    }

    /// Uses up a trigger ID. Errors are Slack's codes.
    fn redeem_trigger(&self, id: &str, app_id: &str, ttl: Duration) -> Result<Trigger, &'static str> {
        let mut inner = self.inner.lock().unwrap();
        let Some(t) = inner.triggers.get_mut(id).filter(|t| t.app_id == app_id) else {
            return Err("invalid_trigger_id");
        };
        if t.used {
            return Err("exchanged_trigger_id");
        }
        if t.issued.elapsed() > ttl {
            return Err("expired_trigger_id");
        }
        t.used = true;
        Ok(t.clone())
    }

    fn issue_response_url(&self, target: ResponseTarget) -> (String, String) {
        let id = digits(&new_secret(), 13);
        let secret = new_secret()[..24].to_owned();
        let mut inner = self.inner.lock().unwrap();
        inner.responses.retain(|_, r| r.created.elapsed() < RESPONSE_URL_TTL);
        inner.responses.insert(
            format!("{id}/{secret}"),
            ResponseUrl {
                target,
                created: Instant::now(),
                uses: 0,
            },
        );
        (id, secret)
    }

    fn use_response_url(&self, key: &str) -> Option<ResponseTarget> {
        let mut inner = self.inner.lock().unwrap();
        let r = inner.responses.get_mut(key)?;
        if r.created.elapsed() > RESPONSE_URL_TTL || r.uses >= RESPONSE_URL_USES {
            return None;
        }
        r.uses += 1;
        Some(r.target.clone())
    }

    fn remember_ephemeral(&self, channel: &str, ts: &str, e: Ephemeral) {
        let mut inner = self.inner.lock().unwrap();
        inner.ephemerals.retain(|_, e| e.created.elapsed() < EPHEMERAL_TTL);
        inner.ephemerals.insert(format!("{channel}/{ts}"), e);
    }

    fn forget_ephemeral(&self, channel: &str, ts: &str) {
        self.inner.lock().unwrap().ephemerals.remove(&format!("{channel}/{ts}"));
    }

    pub fn ephemeral(&self, channel: &str, ts: &str) -> Option<Ephemeral> {
        self.inner
            .lock()
            .unwrap()
            .ephemerals
            .get(&format!("{channel}/{ts}"))
            .cloned()
    }

    /// Forgets everything (`/_emu/reset`).
    pub fn reset(&self) {
        *self.inner.lock().unwrap() = Inner::default();
    }

    pub fn modal_stack(&self, user_id: &str) -> Vec<View> {
        self.inner
            .lock()
            .unwrap()
            .modals
            .get(user_id)
            .cloned()
            .unwrap_or_default()
    }

    /// Changes a user's modal stack atomically.
    pub fn with_stack<R>(&self, user_id: &str, f: impl FnOnce(&mut Vec<View>) -> R) -> R {
        let mut inner = self.inner.lock().unwrap();
        let stack = inner.modals.entry(user_id.to_owned()).or_default();
        let r = f(stack);
        if stack.is_empty() {
            inner.modals.remove(user_id);
        }
        r
    }

    pub fn find_modal(&self, pred: impl Fn(&View) -> bool) -> Option<View> {
        self.inner
            .lock()
            .unwrap()
            .modals
            .values()
            .flatten()
            .find(|v| pred(v))
            .cloned()
    }
}

/// `count` decimal digits derived from a random string.
fn digits(seed: &str, count: usize) -> String {
    seed.bytes()
        .cycle()
        .take(count)
        .enumerate()
        .map(|(i, b)| char::from(b'0' + ((b as usize + i * 7) % 10) as u8))
        .collect()
}

/// A trigger ID for an interaction by `user` with `app`, usable once for `views.open`/`push`.
pub fn trigger(state: &AppState, team_id: &str, user_id: &str, app_id: &str) -> String {
    state.interactions.issue_trigger(team_id, user_id, app_id)
}

pub fn redeem_trigger(state: &AppState, id: &str, app_id: &str) -> ApiResult<Trigger> {
    state
        .interactions
        .redeem_trigger(id, app_id, state.config.trigger_ttl)
        .map_err(|code| SlackError::new(code).into())
}

/// A response URL (`kind` is `actions` or `commands`, as in Slack's URLs).
pub fn response_url(state: &AppState, kind: &str, target: ResponseTarget) -> String {
    let team = target.team_id.clone();
    let (id, secret) = state.interactions.issue_response_url(target);
    format!("{}/hooks/{kind}/{team}/{id}/{secret}", state.config.public_url)
}

/// The `user` object in interaction payloads.
pub fn user_obj(u: &User) -> Value {
    json!({"id": u.id, "username": u.name, "name": u.name, "team_id": u.team_id})
}

pub fn team_obj(ws: &Workspace) -> Value {
    json!({"id": ws.id, "domain": ws.domain})
}

/// A conversation's name as interaction payloads give it.
pub fn channel_name(conv: &Conversation) -> String {
    if conv.is_im {
        "directmessage".into()
    } else {
        conv.name.clone().unwrap_or_default()
    }
}

/// Shows an ephemeral message to one user (replacing any earlier one with the same ts).
pub async fn send_ephemeral(state: &AppState, conv: &Conversation, user_id: &str, msg: &Message) -> ApiResult<()> {
    let mut shown = shapes::one_message(state, msg).await?;
    shown["is_ephemeral"] = json!(true);
    state.interactions.remember_ephemeral(
        &conv.id,
        &msg.ts,
        Ephemeral {
            app_id: msg.app_id.clone(),
            user_id: user_id.to_owned(),
            message: shown.clone(),
            created: Instant::now(),
        },
    );
    publish(
        state,
        &conv.team_id,
        Audience::Users(vec![user_id.to_owned()]),
        true,
        json!({"type": "emu_ephemeral", "channel": conv.id, "message": shown}),
    );
    Ok(())
}

/// An ephemeral notice from Slackbot (unknown commands, apps that didn't answer).
pub async fn slackbot_says(state: &AppState, conv: &Conversation, user_id: &str, text: &str) -> ApiResult<()> {
    let msg = chat::build_message(state, conv, Some("USLACKBOT".into()), None, None, text.into());
    send_ephemeral(state, conv, user_id, &msg).await
}

fn delete_ephemeral(state: &AppState, conv: &Conversation, user_id: &str, ts: &str) {
    state.interactions.forget_ephemeral(&conv.id, ts);
    publish(
        state,
        &conv.team_id,
        Audience::Users(vec![user_id.to_owned()]),
        true,
        json!({"type": "emu_ephemeral_deleted", "channel": conv.id, "ts": ts}),
    );
}

/// Sends the user's modal stack to their browser sessions (empty means closed).
pub fn publish_modals(state: &AppState, team_id: &str, user_id: &str) {
    let views: Vec<Value> = state
        .interactions
        .modal_stack(user_id)
        .iter()
        .map(View::to_json)
        .collect();
    publish(
        state,
        team_id,
        Audience::Users(vec![user_id.to_owned()]),
        true,
        json!({"type": "emu_modals", "views": views}),
    );
}

/// Loads a view by ID: an open modal, or a stored App Home view.
pub async fn find_view(state: &AppState, view_id: &str) -> ApiResult<Option<View>> {
    if let Some(v) = state.interactions.find_modal(|v| v.id == view_id) {
        return Ok(Some(v));
    }
    Ok(state
        .store
        .home_view_by_id(view_id)
        .await?
        .and_then(|d| serde_json::from_value(d).ok()))
}

/// Saves a changed view and shows the change: modals in memory, home views in the store.
pub async fn save_view(state: &AppState, view: &View) -> ApiResult<()> {
    if view.is_home() {
        state
            .store
            .save_home_view(
                &view.id,
                &view.app_id,
                &view.user_id,
                view.external_id(),
                &serde_json::to_value(view).expect("view serializes"),
            )
            .await?;
        publish(
            state,
            &view.team_id,
            Audience::Users(vec![view.user_id.clone()]),
            true,
            json!({"type": "emu_home_view", "app_id": view.app_id, "view": view.to_json()}),
        );
    } else {
        let found = state.interactions.with_stack(&view.user_id, |stack| {
            stack
                .iter_mut()
                .find(|v| v.id == view.id)
                .map(|slot| *slot = view.clone())
                .is_some()
        });
        if found {
            publish_modals(state, &view.team_id, &view.user_id);
        }
    }
    Ok(())
}

/// Answers an interaction: posts, replaces or deletes messages as the app
/// (a response URL POST, or a slash command's immediate response).
pub async fn respond(state: &AppState, target: &ResponseTarget, body: &Args) -> ApiResult<()> {
    let app = match state.store.get_app(&target.app_id).await? {
        Some(a) if a.is_installed() => a,
        _ => return Err(SlackError::invalid_auth().into()),
    };
    let channel = target.channel_id.as_deref().unwrap_or_default();
    let Some(conv) = state.store.get_conversation(channel).await? else {
        return Err(SlackError::new("channel_not_found").into());
    };
    let flag = |k: &str| args::bool_arg(body, k, false);

    if flag("delete_original") {
        let Some(src) = &target.source else {
            return Err(SlackError::new("no_original_message").into());
        };
        if src.ephemeral {
            delete_ephemeral(state, &conv, &target.user_id, &src.ts);
        } else {
            let msg = own_message(state, &app, &conv, &src.ts).await?;
            chat::remove_message(state, &conv, msg).await?;
        }
        return Ok(());
    }

    let c = chat::content(body)?;
    if c.is_empty() {
        return Err(SlackError::new("no_text").into());
    }
    if flag("replace_original")
        && let Some(src) = &target.source
    {
        if src.ephemeral {
            let before = state.interactions.ephemeral(&conv.id, &src.ts);
            let mut msg = chat::app_message(state, &conv, &app, c);
            msg.ts = src.ts.clone();
            msg.thread_ts = before.and_then(|e| e.message["thread_ts"].as_str().map(str::to_owned));
            send_ephemeral(state, &conv, &target.user_id, &msg).await?;
        } else {
            // The whole message is replaced: blocks not resent are dropped.
            let mut msg = own_message(state, &app, &conv, &src.ts).await?;
            msg.blocks = None;
            msg.attachments = None;
            chat::apply_update(state, &conv, &mut msg, c, None).await?;
        }
        return Ok(());
    }

    let mut msg = chat::app_message(state, &conv, &app, c);
    if let Some(tts) = args::opt_ts(body, "thread_ts")? {
        let Some(parent) = state.store.get_message(&conv.id, &tts).await? else {
            return Err(SlackError::new("thread_not_found").into());
        };
        msg.thread_ts = Some(parent.thread_ts.unwrap_or(parent.ts));
    }
    if body.get("response_type").and_then(Value::as_str) == Some("in_channel") {
        post_as(state, &conv, msg).await
    } else {
        send_ephemeral(state, &conv, &target.user_id, &msg).await
    }
}

/// Stores and announces a message on someone's behalf (no membership checks:
/// responses reach any channel the interaction came from).
pub async fn post_as(state: &AppState, conv: &Conversation, msg: Message) -> ApiResult<()> {
    state.store.insert_message(&msg).await?;
    services::announce_message(state, conv, &msg).await
}

/// A stored message the app posted (apps can only change their own).
async fn own_message(state: &AppState, app: &App, conv: &Conversation, ts: &str) -> ApiResult<Message> {
    match state.store.get_message(&conv.id, ts).await? {
        Some(m) if m.app_id.as_deref() == Some(&app.id) => Ok(m),
        Some(_) => Err(SlackError::new("cant_update_message").into()),
        None => Err(SlackError::new("message_not_found").into()),
    }
}

/// A plain-text or `{text, blocks, …}` response as message content arguments.
pub fn response_args(resp: &Value) -> Option<Args> {
    match resp {
        Value::String(t) if !t.trim().is_empty() => {
            let mut a = Args::new();
            a.insert("text".into(), json!(t));
            Some(a)
        }
        Value::Object(o) if ["text", "blocks", "attachments"].iter().any(|k| o.contains_key(*k)) => Some(o.clone()),
        _ => None,
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route("/{kind}/{team}/{id}/{secret}", post(hook))
}

/// Parses a response URL body: JSON, or a form with a `payload` field.
fn parse_body(headers: &HeaderMap, body: &[u8]) -> Option<Args> {
    let is_form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));
    if is_form {
        let pairs: Vec<(String, String)> = serde_urlencoded::from_bytes(body).ok()?;
        if let Some((_, p)) = pairs.iter().find(|(k, _)| k == "payload") {
            return serde_json::from_str(p).ok();
        }
        return Some(pairs.into_iter().map(|(k, v)| (k, Value::String(v))).collect());
    }
    serde_json::from_slice(body).ok()
}

async fn hook(
    State(state): State<AppState>,
    Path((_kind, team, id, secret)): Path<(String, String, String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(target) = state
        .interactions
        .use_response_url(&format!("{id}/{secret}"))
        .filter(|t| t.team_id == team)
    else {
        return ApiError::new(StatusCode::NOT_FOUND, "expired_url").into_response();
    };
    let Some(args) = parse_body(&headers, &body) else {
        return ApiError::new(StatusCode::BAD_REQUEST, "invalid_payload").into_response();
    };
    let result = respond(&state, &target, &args).await;
    let logged = Value::Object(args);
    state.delivery.log_inbound(
        &target.app_id,
        "response_url",
        &logged,
        result.as_ref().err().map(|e| e.code.as_str()),
    );
    match result {
        Ok(()) => axum::Json(json!({"ok": true})).into_response(),
        Err(e) => e.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triggers_are_single_use_and_expire() {
        let i = Interactions::default();
        let t = i.issue_trigger("T1", "U1", "A1");
        assert_eq!(t.split('.').count(), 3);
        assert_eq!(
            i.redeem_trigger(&t, "A2", Duration::from_secs(3)).unwrap_err(),
            "invalid_trigger_id"
        );
        assert_eq!(
            i.redeem_trigger(&t, "A1", Duration::from_secs(3)).unwrap().user_id,
            "U1"
        );
        assert_eq!(
            i.redeem_trigger(&t, "A1", Duration::from_secs(3)).unwrap_err(),
            "exchanged_trigger_id"
        );
        let t2 = i.issue_trigger("T1", "U1", "A1");
        assert_eq!(
            i.redeem_trigger(&t2, "A1", Duration::ZERO).unwrap_err(),
            "expired_trigger_id"
        );
    }

    #[test]
    fn response_urls_allow_five_uses() {
        let i = Interactions::default();
        let target = ResponseTarget {
            team_id: "T1".into(),
            app_id: "A1".into(),
            user_id: "U1".into(),
            channel_id: None,
            source: None,
        };
        let (id, secret) = i.issue_response_url(target);
        let key = format!("{id}/{secret}");
        for _ in 0..5 {
            assert!(i.use_response_url(&key).is_some());
        }
        assert!(i.use_response_url(&key).is_none());
    }
}
