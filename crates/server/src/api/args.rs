//! Typed access to Web API arguments, which arrive as strings (form/query)
//! or JSON values.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use slack_emu_core::{SlackError, ts};

use super::{ApiError, ApiResult, Args};

/// A non-empty string argument. Numbers are accepted and stringified (e.g. a JSON `ts`).
pub fn opt_str(args: &Args, key: &str) -> Option<String> {
    match args.get(key)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub fn req_str(args: &Args, key: &str) -> ApiResult<String> {
    opt_str(args, key).ok_or_else(|| ApiError::invalid_arguments(format!("missing required field: {key}")))
}

pub fn bool_arg(args: &Args, key: &str, default: bool) -> bool {
    match args.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_i64() != Some(0),
        Some(Value::String(s)) => match s.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" => true,
            "false" | "0" | "no" => false,
            _ => default,
        },
        _ => default,
    }
}

/// `limit`, defaulting when absent or 0, clamped to `max`.
pub fn limit(args: &Args, default: i64, max: i64) -> i64 {
    let n = match args.get("limit") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse().unwrap_or(0),
        _ => 0,
    };
    if n <= 0 { default } else { n.min(max) }
}

/// A comma-separated list argument (or a JSON array of strings).
pub fn list(args: &Args, key: &str) -> Vec<String> {
    match args.get(key) {
        Some(Value::String(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    }
}

/// A JSON argument: either embedded JSON (form-encoded `blocks=[...]`) or a JSON value.
pub fn json(args: &Args, key: &str) -> ApiResult<Option<Value>> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if s.trim().is_empty() => Ok(None),
        Some(Value::String(s)) => serde_json::from_str(s)
            .map(Some)
            .map_err(|_| ApiError::invalid_arguments(format!("{key} is not valid JSON"))),
        Some(v) => Ok(Some(v.clone())),
    }
}

/// A message ts argument in canonical fixed-width form.
pub fn opt_ts(args: &Args, key: &str) -> ApiResult<Option<String>> {
    match opt_str(args, key) {
        None => Ok(None),
        Some(raw) => ts::normalize(&raw)
            .map(Some)
            .ok_or_else(|| ApiError::invalid_arguments(format!("{key} is not a valid timestamp"))),
    }
}

pub fn req_ts(args: &Args, key: &str) -> ApiResult<String> {
    opt_ts(args, key)?.ok_or_else(|| ApiError::invalid_arguments(format!("missing required field: {key}")))
}

/// Pagination cursors look like Slack's: base64 of `"<kind>:<value>"`.
pub fn encode_cursor(kind: &str, value: &str) -> String {
    STANDARD.encode(format!("{kind}:{value}"))
}

pub fn cursor(args: &Args, kind: &str) -> ApiResult<Option<String>> {
    let Some(raw) = opt_str(args, "cursor") else {
        return Ok(None);
    };
    STANDARD
        .decode(raw)
        .ok()
        .and_then(|b| String::from_utf8(b).ok())
        .and_then(|s| s.strip_prefix(&format!("{kind}:")).map(str::to_owned))
        .map(Some)
        .ok_or_else(|| SlackError::new("invalid_cursor").into())
}

/// `response_metadata` with the next cursor (empty string when there are no more pages).
pub fn response_metadata(next: Option<String>) -> Value {
    serde_json::json!({"next_cursor": next.unwrap_or_default()})
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn args(v: Value) -> Args {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn parsing() {
        let a = args(json!({"b": "true", "n": "5", "l": "a, b,,c", "j": "[1]", "t": 1730000000.0001}));
        assert!(bool_arg(&a, "b", false));
        assert_eq!(limit(&args(json!({"limit": "5000"})), 100, 1000), 1000);
        assert_eq!(limit(&a, 100, 1000), 100);
        assert_eq!(list(&a, "l"), vec!["a", "b", "c"]);
        assert_eq!(json(&a, "j").unwrap(), Some(json!([1])));
        assert_eq!(opt_ts(&a, "t").unwrap().as_deref(), Some("1730000000.000100"));
    }

    #[test]
    fn cursors_round_trip() {
        let c = encode_cursor("ts", "1.000001");
        let a = args(json!({"cursor": c}));
        assert_eq!(cursor(&a, "ts").unwrap().as_deref(), Some("1.000001"));
        assert!(cursor(&a, "id").is_err());
    }
}
