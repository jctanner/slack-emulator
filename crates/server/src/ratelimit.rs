//! Slack-like rate limits for API tokens. Each method has a tier (calls per
//! minute, with bursts), counted per app (or user) per workspace, as Slack
//! does. `chat.postMessage` is limited per channel instead. The web UI is
//! never limited.
//!
//! Tests can also force the next N calls of a method to fail with
//! `ratelimited`, whether or not limits are on, to exercise a bot's retries.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use serde_json::{Value, json};

/// Calls per minute for Slack's tiers 1–4.
const TIERS: [u32; 4] = [1, 20, 50, 100];

/// Approximately Slack's published tier for each method (tier 3 if unlisted).
pub fn tier(method: &str) -> u8 {
    match method {
        "apps.connections.open"
        | "apps.manifest.create"
        | "apps.manifest.delete"
        | "apps.manifest.export"
        | "apps.manifest.update"
        | "apps.manifest.validate" => 1,
        "conversations.list"
        | "conversations.create"
        | "conversations.archive"
        | "conversations.unarchive"
        | "conversations.rename"
        | "conversations.setPurpose"
        | "conversations.setTopic"
        | "users.list"
        | "users.conversations" => 2,
        "auth.test"
        | "api.test"
        | "users.info"
        | "users.profile.get"
        | "conversations.members"
        | "chat.postEphemeral"
        | "chat.getPermalink"
        | "views.open"
        | "views.push"
        | "views.update"
        | "views.publish" => 4,
        _ => 3,
    }
}

/// How a method is limited: a bucket of `capacity` calls refilled at `per_sec`.
#[derive(Debug, Clone, Copy)]
struct Limit {
    capacity: f64,
    per_sec: f64,
}

impl Limit {
    fn per_minute(n: u32) -> Self {
        let n = f64::from(n.max(1));
        Self {
            capacity: n,
            per_sec: n / 60.0,
        }
    }
}

struct Bucket {
    tokens: f64,
    last: Instant,
}

struct Forced {
    remaining: u32,
    retry_after: u64,
}

struct Inner {
    enabled: bool,
    /// Per-minute overrides by method.
    overrides: HashMap<String, u32>,
    forced: HashMap<String, Forced>,
    buckets: HashMap<String, Bucket>,
}

pub struct RateLimiter {
    inner: Mutex<Inner>,
}

impl RateLimiter {
    pub fn new(enabled: bool) -> Self {
        Self {
            inner: Mutex::new(Inner {
                enabled,
                overrides: HashMap::new(),
                forced: HashMap::new(),
                buckets: HashMap::new(),
            }),
        }
    }

    fn limit(inner: &Inner, method: &str) -> Limit {
        if let Some(n) = inner.overrides.get(method) {
            return Limit::per_minute(*n);
        }
        if method == "chat.postMessage" {
            // About one message per second per channel, with short bursts.
            return Limit {
                capacity: 5.0,
                per_sec: 1.0,
            };
        }
        Limit::per_minute(TIERS[usize::from(tier(method) - 1)])
    }

    /// Counts a call by `who` (an app or user in a workspace). `Err` carries
    /// the seconds to wait (`Retry-After`).
    pub fn check(&self, method: &str, who: &str, channel: Option<&str>) -> Result<(), u64> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(f) = inner.forced.get_mut(method)
            && f.remaining > 0
        {
            f.remaining -= 1;
            return Err(f.retry_after);
        }
        if !inner.enabled {
            return Ok(());
        }
        let limit = Self::limit(&inner, method);
        let key = match channel {
            Some(c) => format!("{who}/{method}/{c}"),
            None => format!("{who}/{method}"),
        };
        let now = Instant::now();
        let bucket = inner.buckets.entry(key).or_insert(Bucket {
            tokens: limit.capacity,
            last: now,
        });
        let elapsed = now.duration_since(bucket.last).as_secs_f64();
        bucket.tokens = (bucket.tokens + elapsed * limit.per_sec).min(limit.capacity);
        bucket.last = now;
        if bucket.tokens >= 1.0 {
            bucket.tokens -= 1.0;
            Ok(())
        } else {
            Err(((1.0 - bucket.tokens) / limit.per_sec).ceil().max(1.0) as u64)
        }
    }

    pub fn status(&self) -> Value {
        let inner = self.inner.lock().unwrap();
        let forced: serde_json::Map<String, Value> = inner
            .forced
            .iter()
            .filter(|(_, f)| f.remaining > 0)
            .map(|(m, f)| (m.clone(), json!({"count": f.remaining, "retry_after": f.retry_after})))
            .collect();
        json!({
            "enabled": inner.enabled,
            "tiers_per_minute": {"1": TIERS[0], "2": TIERS[1], "3": TIERS[2], "4": TIERS[3]},
            "methods": inner.overrides,
            "force": forced,
        })
    }

    /// Applies `{enabled, methods: {name: per_minute|null}, force: {name: count|{count, retry_after}}}`.
    pub fn configure(&self, v: &Value) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(e) = v.get("enabled") {
            inner.enabled = e.as_bool().ok_or("enabled must be true or false")?;
        }
        for (method, n) in v["methods"].as_object().into_iter().flatten() {
            match n {
                Value::Null => {
                    inner.overrides.remove(method);
                }
                n => {
                    let n = n
                        .as_u64()
                        .filter(|n| *n > 0)
                        .ok_or("methods values are calls per minute")?;
                    inner.overrides.insert(method.clone(), n as u32);
                }
            }
        }
        for (method, f) in v["force"].as_object().into_iter().flatten() {
            let (count, retry_after) = match f {
                Value::Number(n) => (n.as_u64(), Some(1)),
                Value::Object(o) => (
                    o.get("count").and_then(Value::as_u64),
                    Some(o.get("retry_after").and_then(Value::as_u64).unwrap_or(1)),
                ),
                _ => (None, None),
            };
            let (Some(count), Some(retry_after)) = (count, retry_after) else {
                return Err("force values are a count or {count, retry_after}".into());
            };
            inner.forced.insert(
                method.clone(),
                Forced {
                    remaining: count as u32,
                    retry_after,
                },
            );
        }
        // Changed limits start from full buckets.
        inner.buckets.clear();
        Ok(())
    }

    /// Forgets counts, overrides and forced limits (`/_emu/reset`).
    pub fn reset(&self) {
        let mut inner = self.inner.lock().unwrap();
        inner.overrides.clear();
        inner.forced.clear();
        inner.buckets.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_and_bursts() {
        let r = RateLimiter::new(true);
        for _ in 0..20 {
            assert!(r.check("users.list", "T1/A1", None).is_ok());
        }
        let wait = r.check("users.list", "T1/A1", None).unwrap_err();
        assert!((1..=3).contains(&wait), "{wait}");
        // Counted per app and per channel.
        assert!(r.check("users.list", "T1/A2", None).is_ok());
        for _ in 0..5 {
            assert!(r.check("chat.postMessage", "T1/A1", Some("C1")).is_ok());
        }
        assert_eq!(r.check("chat.postMessage", "T1/A1", Some("C1")), Err(1));
        assert!(r.check("chat.postMessage", "T1/A1", Some("C2")).is_ok());
    }

    #[test]
    fn forced_limits_work_while_disabled() {
        let r = RateLimiter::new(false);
        r.configure(&json!({"force": {"chat.update": {"count": 2, "retry_after": 7}}}))
            .unwrap();
        assert_eq!(r.check("chat.update", "x", None), Err(7));
        assert_eq!(r.check("chat.update", "x", None), Err(7));
        assert!(r.check("chat.update", "x", None).is_ok());
        r.configure(&json!({"enabled": true, "methods": {"chat.update": 1}}))
            .unwrap();
        assert!(r.check("chat.update", "x", None).is_ok());
        assert_eq!(r.check("chat.update", "x", None), Err(60));
    }
}
