use std::env;

use anyhow::Context;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    /// Externally reachable base URL, without a trailing slash.
    pub public_url: String,
    pub database_url: String,
    /// Workspace created on first start and joined by new signups.
    pub default_workspace_name: String,
    pub default_workspace_domain: String,
    pub allow_signup: bool,
    pub session_ttl_secs: i64,
    /// Delays before each Events API retry (Slack: about immediately, 1 min, 5 min).
    pub event_retry_delays: Vec<std::time::Duration>,
    /// How long a `trigger_id` stays usable (Slack: 3 seconds).
    pub trigger_ttl: std::time::Duration,
    /// Bearer token for the test harness (`/_emu/*`); the harness is off without one.
    pub admin_token: Option<String>,
    /// Start with Slack-like rate limits on (they can be toggled at runtime).
    pub rate_limits: bool,
    /// A fixture to load at startup when the database has no users yet.
    pub seed_file: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:8080".into(),
            public_url: "http://localhost:8080".into(),
            database_url: "sqlite://slack-emu.db".into(),
            default_workspace_name: "Emulator".into(),
            default_workspace_domain: "emulator".into(),
            allow_signup: true,
            session_ttl_secs: 30 * 24 * 3600,
            event_retry_delays: [1, 60, 300].map(std::time::Duration::from_secs).to_vec(),
            trigger_ttl: std::time::Duration::from_secs(3),
            admin_token: None,
            rate_limits: false,
            seed_file: None,
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let d = Self::default();
        let var = |k: &str, default: String| env::var(k).unwrap_or(default);
        Ok(Self {
            bind_addr: var("BIND_ADDR", d.bind_addr),
            public_url: var("PUBLIC_URL", d.public_url).trim_end_matches('/').into(),
            database_url: var("DATABASE_URL", d.database_url),
            default_workspace_name: var("EMU_DEFAULT_WORKSPACE_NAME", d.default_workspace_name),
            default_workspace_domain: var("EMU_DEFAULT_WORKSPACE_DOMAIN", d.default_workspace_domain),
            allow_signup: match env::var("EMU_ALLOW_SIGNUP") {
                Ok(v) => parse_bool(&v).context("EMU_ALLOW_SIGNUP")?,
                Err(_) => d.allow_signup,
            },
            session_ttl_secs: match env::var("EMU_SESSION_TTL_SECS") {
                Ok(v) => v.parse().context("EMU_SESSION_TTL_SECS")?,
                Err(_) => d.session_ttl_secs,
            },
            event_retry_delays: match env::var("EMU_EVENT_RETRY_DELAYS") {
                Ok(v) => parse_delays(&v).context("EMU_EVENT_RETRY_DELAYS")?,
                Err(_) => d.event_retry_delays,
            },
            trigger_ttl: match env::var("EMU_TRIGGER_TTL") {
                Ok(v) => std::time::Duration::from_secs_f64(v.parse().context("EMU_TRIGGER_TTL")?),
                Err(_) => d.trigger_ttl,
            },
            admin_token: env::var("EMU_ADMIN_TOKEN").ok().filter(|t| !t.is_empty()),
            rate_limits: match env::var("EMU_RATE_LIMITS") {
                Ok(v) => parse_bool(&v).context("EMU_RATE_LIMITS")?,
                Err(_) => d.rate_limits,
            },
            seed_file: env::var("EMU_SEED_FILE").ok().filter(|f| !f.is_empty()),
        })
    }

    pub fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

/// Comma-separated seconds, e.g. `1,60,300` or `0.1,0.2`; empty disables retries.
fn parse_delays(v: &str) -> anyhow::Result<Vec<std::time::Duration>> {
    v.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| Ok(std::time::Duration::from_secs_f64(s.parse()?)))
        .collect()
}

fn parse_bool(v: &str) -> anyhow::Result<bool> {
    match v.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("expected a boolean, got {v:?}"),
    }
}
