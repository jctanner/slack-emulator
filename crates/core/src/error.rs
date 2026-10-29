//! Slack API error codes.

/// An error returned to API callers as `{"ok": false, "error": "<code>"}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}")]
pub struct SlackError {
    pub code: String,
}

impl SlackError {
    pub fn new(code: impl Into<String>) -> Self {
        Self { code: code.into() }
    }

    pub fn not_authed() -> Self {
        Self::new("not_authed")
    }

    pub fn invalid_auth() -> Self {
        Self::new("invalid_auth")
    }

    pub fn unknown_method() -> Self {
        Self::new("unknown_method")
    }

    pub fn internal() -> Self {
        Self::new("internal_error")
    }
}
