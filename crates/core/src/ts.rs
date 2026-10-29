//! Message timestamps (`ts`): `"<seconds>.<microseconds>"`, e.g. `"1730212345.000100"`.
//!
//! Slack's `ts` is both a timestamp and a unique message ID. We hand them out
//! from one process-wide monotonic clock so they never collide, and keep the
//! seconds part zero-padded to 10 digits so they sort correctly as text.

use std::sync::atomic::{AtomicI64, Ordering};

pub const ZERO: &str = "0000000000.000000";

#[derive(Debug, Default)]
pub struct TsClock {
    last_micros: AtomicI64,
}

impl TsClock {
    /// Creates a clock that only issues values after `floor` (e.g. the newest stored ts).
    pub fn starting_after(floor: Option<&str>) -> Self {
        Self {
            last_micros: AtomicI64::new(floor.and_then(parse).unwrap_or(0)),
        }
    }

    pub fn next(&self) -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before 1970")
            .as_micros() as i64;
        let mut prev = self.last_micros.load(Ordering::Relaxed);
        loop {
            let candidate = now.max(prev + 1);
            match self
                .last_micros
                .compare_exchange(prev, candidate, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return format(candidate),
                Err(actual) => prev = actual,
            }
        }
    }
}

pub fn format(micros: i64) -> String {
    format!("{:010}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

/// Parses a ts in any common spelling (`"123.45"`, `"123"`) into microseconds.
pub fn parse(ts: &str) -> Option<i64> {
    let (secs, frac) = ts.split_once('.').unwrap_or((ts, ""));
    if secs.is_empty() || !secs.bytes().all(|b| b.is_ascii_digit()) || frac.len() > 6 {
        return None;
    }
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let secs: i64 = secs.parse().ok()?;
    let frac: i64 = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<6}").parse().ok()?
    };
    Some(secs * 1_000_000 + frac)
}

/// Canonical fixed-width form of a caller-supplied ts, or `None` if malformed.
pub fn normalize(ts: &str) -> Option<String> {
    parse(ts).map(format)
}

/// The seconds part, for `event_time`-style integer fields.
pub fn seconds(ts: &str) -> i64 {
    parse(ts).map(|m| m / 1_000_000).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_fixed_width() {
        let clock = TsClock::starting_after(Some("4000000000.999998"));
        let a = clock.next();
        let b = clock.next();
        assert_eq!(a, "4000000000.999999");
        assert_eq!(b, "4000000001.000000");
        assert!(b > a);
        let fresh = TsClock::default().next();
        assert_eq!(fresh.len(), 17);
    }

    #[test]
    fn normalizes() {
        assert_eq!(normalize("1730212345.0001").as_deref(), Some("1730212345.000100"));
        assert_eq!(normalize("1730212345").as_deref(), Some("1730212345.000000"));
        assert_eq!(normalize("abc"), None);
        assert_eq!(normalize("1.1234567"), None);
        assert_eq!(seconds("1730212345.000100"), 1730212345);
    }
}
