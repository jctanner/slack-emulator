//! Domain types and helpers shared by the store and server crates.

pub mod blocks;
pub mod error;
pub mod ids;
pub mod manifest;
pub mod models;
pub mod ts;

pub use error::SlackError;

/// Current time as Unix seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before 1970")
        .as_secs() as i64
}
