//! Delivering events (and, later, interactions) to apps, over Socket Mode or
//! signed HTTP requests, with Slack's retry behaviour and a per-app log the
//! UI can show.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Value, json};
use sha2::Sha256;
use slack_emu_core::ids::new_secret;
use slack_emu_core::models::App;
use slack_emu_core::{now, ts};
use tokio::sync::{broadcast::error::RecvError, mpsc};

use crate::AppState;
use crate::events::{Audience, BusEvent};
use crate::socket_mode::SocketHub;

/// How long an app has to answer an HTTP request or ack an envelope.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
const LOG_LIMIT: usize = 200;

pub enum Body {
    Json(Value),
    /// Form-encoded (interactivity uses `payload=<json>`, slash commands plain fields).
    Form(Vec<(String, String)>),
}

/// One thing to send to an app.
pub struct Outbound {
    /// Socket Mode envelope type: `events_api`, `interactive` or `slash_commands`.
    pub kind: &'static str,
    /// What goes in the Socket Mode envelope's `payload`.
    pub payload: Value,
    /// What goes in the HTTP request body.
    pub body: Body,
    /// HTTP destination when the app isn't using Socket Mode.
    pub url: Option<String>,
    pub accepts_response_payload: bool,
    /// Shown in the delivery log, e.g. `message.channels` or `block_actions`.
    pub label: String,
}

pub struct Delivery {
    http: reqwest::Client,
    pub sockets: SocketHub,
    log: Mutex<HashMap<String, VecDeque<Value>>>,
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
    retry_delays: Vec<Duration>,
    seq: AtomicU64,
}

impl Delivery {
    pub fn new(retry_delays: Vec<Duration>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(RESPONSE_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .user_agent("Slackbot 1.0 (+https://api.slack.com/robots)")
                .build()
                .expect("http client"),
            sockets: SocketHub::default(),
            log: Mutex::default(),
            queues: Mutex::default(),
            retry_delays,
            seq: AtomicU64::new(1),
        }
    }

    /// Most recent deliveries to an app, newest first.
    pub fn deliveries(&self, app_id: &str) -> Vec<Value> {
        self.log
            .lock()
            .unwrap()
            .get(app_id)
            .map(|q| q.iter().rev().cloned().collect())
            .unwrap_or_default()
    }

    /// Deliveries to any app after `seq`, oldest first (the harness and inspector).
    pub fn since(&self, after: u64, app_ids: Option<&[String]>) -> Vec<Value> {
        let log = self.log.lock().unwrap();
        let mut out: Vec<Value> = log
            .iter()
            .filter(|(app, _)| app_ids.is_none_or(|ids| ids.contains(app)))
            .flat_map(|(_, q)| q.iter())
            .filter(|e| e["seq"].as_u64().is_some_and(|s| s > after))
            .cloned()
            .collect();
        out.sort_by_key(|e| e["seq"].as_u64());
        out
    }

    /// Forgets logs and queued events and drops every Socket Mode connection (`/_emu/reset`).
    pub fn reset(&self) {
        self.log.lock().unwrap().clear();
        // Dropping the senders ends each app's delivery task.
        self.queues.lock().unwrap().clear();
        self.sockets.disconnect_everyone();
    }

    fn record(&self, app_id: &str, mut entry: Value) {
        entry["seq"] = json!(self.seq.fetch_add(1, Ordering::Relaxed));
        entry["app_id"] = json!(app_id);
        let mut log = self.log.lock().unwrap();
        let q = log.entry(app_id.to_owned()).or_default();
        q.push_back(entry);
        while q.len() > LOG_LIMIT {
            q.pop_front();
        }
    }

    /// Sends once. `Ok` carries the app's response payload (HTTP body as JSON,
    /// or the ack's `payload`), if any.
    pub async fn send_once(
        &self,
        app: &App,
        out: &Outbound,
        attempt: u32,
        retry_reason: &str,
    ) -> Result<Option<Value>, String> {
        let started = Instant::now();
        let result = if app.manifest().settings.socket_mode_enabled {
            self.send_socket(app, out, attempt, retry_reason).await
        } else {
            self.send_http(app, out, attempt, retry_reason).await
        };
        self.record(
            &app.id,
            json!({
                "time": now(),
                "label": out.label,
                "transport": if app.manifest().settings.socket_mode_enabled { "socket" } else { "http" },
                "attempt": attempt,
                "ok": result.is_ok(),
                "error": result.as_ref().err(),
                "latency_ms": started.elapsed().as_millis() as u64,
                "payload": out.payload,
                "response": result.as_ref().ok().cloned().flatten(),
            }),
        );
        result
    }

    async fn send_socket(
        &self,
        app: &App,
        out: &Outbound,
        attempt: u32,
        reason: &str,
    ) -> Result<Option<Value>, String> {
        let envelope_id = new_envelope_id();
        let mut envelope = json!({
            "envelope_id": envelope_id,
            "payload": out.payload,
            "type": out.kind,
            "accepts_response_payload": out.accepts_response_payload,
        });
        if out.kind == "events_api" {
            envelope["retry_attempt"] = json!(attempt);
            envelope["retry_reason"] = json!(reason);
        }
        let Some(ack) = self.sockets.send(&app.id, &envelope_id, &envelope) else {
            return Err("no_socket_mode_connection".into());
        };
        match tokio::time::timeout(RESPONSE_TIMEOUT, ack).await {
            Ok(Ok(payload)) => Ok((!payload.is_null()).then_some(payload)),
            Ok(Err(_)) => Err("connection_closed".into()),
            Err(_) => {
                self.sockets.forget(&envelope_id);
                Err("ack_timeout".into())
            }
        }
    }

    async fn send_http(&self, app: &App, out: &Outbound, attempt: u32, reason: &str) -> Result<Option<Value>, String> {
        let Some(url) = &out.url else {
            return Err("no_request_url".into());
        };
        let (body, content_type) = match &out.body {
            Body::Json(v) => (v.to_string(), "application/json"),
            Body::Form(pairs) => (
                serde_urlencoded::to_string(pairs).expect("form encoding"),
                "application/x-www-form-urlencoded",
            ),
        };
        let timestamp = now().to_string();
        let mut req = self
            .http
            .post(url)
            .header("Content-Type", content_type)
            .header("X-Slack-Request-Timestamp", &timestamp)
            .header("X-Slack-Signature", sign(&app.signing_secret, &timestamp, &body));
        if attempt > 0 {
            req = req
                .header("X-Slack-Retry-Num", attempt.to_string())
                .header("X-Slack-Retry-Reason", reason);
        }
        let res = req.body(body).send().await.map_err(|e| {
            if e.is_timeout() {
                "http_timeout".to_owned()
            } else {
                "connection_failed".to_owned()
            }
        })?;
        let status = res.status();
        if !status.is_success() {
            return Err(format!("http_error ({})", status.as_u16()));
        }
        let text = res.text().await.unwrap_or_default();
        // JSON, or plain text (a slash command's reply), or nothing.
        Ok(match serde_json::from_str::<Value>(&text) {
            Ok(v) => (!v.is_null()).then_some(v),
            Err(_) => (!text.trim().is_empty()).then_some(Value::String(text)),
        })
    }

    /// Sends an interaction (`interactive`) or slash command (`slash_commands`)
    /// once, without retries, and returns the app's response payload.
    pub async fn interact(
        &self,
        app: &App,
        kind: &'static str,
        label: String,
        payload: Value,
        url: Option<String>,
    ) -> Result<Option<Value>, String> {
        let body = if kind == "slash_commands" {
            Body::Form(
                payload
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| v.to_string(), str::to_owned)))
                    .collect(),
            )
        } else {
            Body::Form(vec![("payload".into(), payload.to_string())])
        };
        // Apps answer commands and view submissions in the ack.
        let accepts_response_payload = kind == "slash_commands" || payload["type"] == "view_submission";
        let out = Outbound {
            kind,
            payload,
            body,
            url,
            accepts_response_payload,
            label,
        };
        self.send_once(app, &out, 0, "").await
    }

    /// Logs something the app sent us (e.g. a response URL post) in its delivery log.
    pub fn log_inbound(&self, app_id: &str, label: &str, payload: &Value, error: Option<&str>) {
        self.record(
            app_id,
            json!({
                "time": now(),
                "label": label,
                "transport": "inbound",
                "attempt": 0,
                "ok": error.is_none(),
                "error": error,
                "latency_ms": 0,
                "payload": payload,
                "response": null,
            }),
        );
    }

    /// Slack's `url_verification` handshake: the endpoint must echo the challenge.
    pub async fn verify_url(&self, app: &App, url: &str) -> Result<(), String> {
        let challenge = new_secret();
        let body =
            json!({"token": app.verification_token, "challenge": challenge, "type": "url_verification"}).to_string();
        let timestamp = now().to_string();
        let res = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-Slack-Request-Timestamp", &timestamp)
            .header("X-Slack-Signature", sign(&app.signing_secret, &timestamp, &body))
            .body(body)
            .send()
            .await
            .map_err(|e| format!("couldn't reach the URL: {e}"))?;
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("the URL answered HTTP {}", status.as_u16()));
        }
        if !text.contains(&challenge) {
            return Err("the response didn't contain the challenge value".into());
        }
        Ok(())
    }

    /// Queues an Events API callback for an app. Events for one app go out in
    /// order; failed ones are retried in the background.
    pub fn enqueue_event(&self, state: &AppState, app_id: &str, callback: Value) {
        let mut queues = self.queues.lock().unwrap();
        let tx = queues.entry(app_id.to_owned()).or_insert_with(|| {
            let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
            let state = state.clone();
            let app_id = app_id.to_owned();
            tokio::spawn(async move {
                while let Some(callback) = rx.recv().await {
                    deliver_event(&state, &app_id, callback).await;
                }
            });
            tx
        });
        let _ = tx.send(callback);
    }
}

/// `v0=` HMAC-SHA256 of `v0:{timestamp}:{body}`, as in `X-Slack-Signature`.
pub fn sign(secret: &str, timestamp: &str, body: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("any key length");
    mac.update(format!("v0:{timestamp}:{body}").as_bytes());
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

fn new_envelope_id() -> String {
    let s = new_secret().to_lowercase();
    format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    )
}

fn new_event_id() -> String {
    format!("Ev{}", &new_secret().to_uppercase()[..10])
}

fn event_outbound(app: &App, callback: &Value) -> Outbound {
    Outbound {
        kind: "events_api",
        payload: callback.clone(),
        body: Body::Json(callback.clone()),
        // Slack only delivers to a request URL that passed verification.
        url: app
            .manifest()
            .request_url()
            .filter(|u| app.verified_request_url.as_deref() == Some(*u))
            .map(str::to_owned),
        accepts_response_payload: false,
        label: event_label(callback),
    }
}

/// First attempt inline (preserving order); retries in the background.
async fn deliver_event(state: &AppState, app_id: &str, callback: Value) {
    let Ok(Some(app)) = state.store.get_app(app_id).await else {
        return;
    };
    let Err(first) = state
        .delivery
        .send_once(&app, &event_outbound(&app, &callback), 0, "")
        .await
    else {
        return;
    };
    let state = state.clone();
    let app_id = app_id.to_owned();
    tokio::spawn(async move {
        let mut reason = retry_reason(&first);
        for (i, delay) in state.delivery.retry_delays.clone().into_iter().enumerate() {
            tokio::time::sleep(delay).await;
            let Ok(Some(app)) = state.store.get_app(&app_id).await else {
                return;
            };
            if !app.is_installed() {
                return;
            }
            match state
                .delivery
                .send_once(&app, &event_outbound(&app, &callback), i as u32 + 1, reason)
                .await
            {
                Ok(_) => return,
                Err(e) => reason = retry_reason(&e),
            }
        }
    });
}

fn retry_reason(err: &str) -> &'static str {
    if err.starts_with("http_timeout") || err == "ack_timeout" {
        "timeout"
    } else if err.starts_with("http_error") {
        "http_error"
    } else {
        "connection_failed"
    }
}

fn event_label(callback: &Value) -> String {
    subscription_name(&callback["event"]).unwrap_or_else(|| "event".into())
}

/// The Events API subscription that covers this event (`message.channels`, `reaction_added`, …).
pub fn subscription_name(event: &Value) -> Option<String> {
    let ty = event["type"].as_str()?;
    if ty == "message" {
        let name = match event["channel_type"].as_str()? {
            "channel" => "message.channels",
            "group" => "message.groups",
            "im" => "message.im",
            "mpim" => "message.mpim",
            _ => return None,
        };
        return Some(name.into());
    }
    Some(ty.into())
}

/// Wraps an event in the Events API `event_callback` envelope for this app.
pub fn event_callback(app: &App, event: Value) -> Value {
    let event_time = event["event_ts"].as_str().map(ts::seconds).unwrap_or_else(now);
    json!({
        "token": app.verification_token,
        "team_id": app.team_id,
        "context_team_id": app.team_id,
        "context_enterprise_id": null,
        "api_app_id": app.id,
        "event": event,
        "type": "event_callback",
        "event_id": new_event_id(),
        "event_time": event_time,
        "authorizations": [{
            "enterprise_id": null,
            "team_id": app.team_id,
            "user_id": app.bot_user_id,
            "is_bot": true,
            "is_enterprise_install": false,
        }],
        "is_ext_shared_channel": false,
        "event_context": format!("4-{}", new_secret()),
    })
}

/// Routes bus events to subscribed apps. Runs for the life of the server.
pub fn spawn_dispatcher(state: AppState) {
    let mut rx = state.bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) if !ev.ui_only => {
                    if let Err(e) = route(&state, &ev).await {
                        tracing::warn!("event routing failed: {e}");
                    }
                }
                Ok(_) => {}
                Err(RecvError::Lagged(n)) => tracing::warn!("event dispatcher dropped {n} events"),
                Err(RecvError::Closed) => break,
            }
        }
    });
}

async fn route(state: &AppState, ev: &BusEvent) -> Result<(), slack_emu_store::StoreError> {
    let Some(name) = subscription_name(&ev.event) else {
        return Ok(());
    };
    for app in state.store.list_apps(&ev.team_id).await? {
        let Some(bot_user) = app.bot_user_id.as_deref().filter(|_| app.is_installed()) else {
            continue;
        };
        let reaches_bot = match &ev.audience {
            Audience::Team => true,
            Audience::Users(ids) => ids.iter().any(|id| id == bot_user),
        };
        if !reaches_bot {
            continue;
        }
        let manifest = app.manifest();
        let subscribed = |n: &str| manifest.bot_events().iter().any(|e| e == n);
        if subscribed(&name) {
            state
                .delivery
                .enqueue_event(state, &app.id, event_callback(&app, ev.event.clone()));
        }
        if subscribed("app_mention")
            && let Some(mention) = app_mention(&ev.event, bot_user)
        {
            state
                .delivery
                .enqueue_event(state, &app.id, event_callback(&app, mention));
        }
    }
    Ok(())
}

/// An `app_mention` event if this is a new message (not an edit or system
/// message) from someone else that mentions the bot.
fn app_mention(event: &Value, bot_user: &str) -> Option<Value> {
    if event["type"] != "message" {
        return None;
    }
    if !matches!(
        event["subtype"].as_str(),
        None | Some("thread_broadcast" | "bot_message" | "file_share")
    ) {
        return None;
    }
    let text = event["text"].as_str()?;
    if !text.contains(&format!("<@{bot_user}>")) || event["user"].as_str() == Some(bot_user) {
        return None;
    }
    let mut m = json!({"type": "app_mention"});
    for key in [
        "user",
        "text",
        "ts",
        "channel",
        "event_ts",
        "thread_ts",
        "blocks",
        "client_msg_id",
        "team",
        "bot_id",
        "attachments",
        "files",
        "edited",
    ] {
        if let Some(v) = event.get(key) {
            m[key] = v.clone();
        }
    }
    Some(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_matches_slack_docs() {
        // Example from https://api.slack.com/authentication/verifying-requests-from-slack
        let body = "token=xyzz0WbapA4vBCDEFasx0q6G&team_id=T1DC2JH3J&team_domain=testteamnow&channel_id=G8PSS9T3V&channel_name=foobar&user_id=U2CERLKJA&user_name=roadrunner&command=%2Fwebhook-collect&text=&response_url=https%3A%2F%2Fhooks.slack.com%2Fcommands%2FT1DC2JH3J%2F397700885554%2F96rGlfmibIGlgcZRskXaIFfN&trigger_id=398738663015.47445629121.803a0bc887a14d10d2c447fce8b6703c";
        assert_eq!(
            sign("8f742231b10e8888abcd99yyyzzz85a5", "1531420618", body),
            "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503"
        );
    }

    #[test]
    fn names_and_mentions() {
        assert_eq!(
            subscription_name(&json!({"type": "message", "channel_type": "group"})).as_deref(),
            Some("message.groups")
        );
        assert_eq!(
            subscription_name(&json!({"type": "reaction_added"})).as_deref(),
            Some("reaction_added")
        );
        let msg = json!({"type": "message", "user": "U1", "text": "hi <@UBOT>", "ts": "1.000001", "channel": "C1"});
        let m = app_mention(&msg, "UBOT").unwrap();
        assert_eq!(m["type"], "app_mention");
        assert_eq!(m["channel"], "C1");
        assert!(app_mention(&msg, "UOTHER").is_none());
        let edit = json!({"type": "message", "subtype": "message_changed", "text": "<@UBOT>"});
        assert!(app_mention(&edit, "UBOT").is_none());
    }
}
