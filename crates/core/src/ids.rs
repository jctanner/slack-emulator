//! Slack-style identifiers and opaque secrets.

use rand::RngExt;
use sha2::{Digest, Sha256};

const ID_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const TOKEN_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// Object kinds that carry a Slack-style ID prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdKind {
    Team,
    User,
    Channel,
    Dm,
    App,
    Bot,
    File,
    View,
    Usergroup,
}

impl IdKind {
    fn prefix(self) -> char {
        match self {
            IdKind::Team => 'T',
            IdKind::User => 'U',
            IdKind::Channel => 'C',
            IdKind::Dm => 'D',
            IdKind::App => 'A',
            IdKind::Bot => 'B',
            IdKind::File => 'F',
            IdKind::View => 'V',
            IdKind::Usergroup => 'S',
        }
    }
}

fn random_string(alphabet: &[u8], len: usize) -> String {
    let mut rng = rand::rng();
    (0..len)
        .map(|_| alphabet[rng.random_range(0..alphabet.len())] as char)
        .collect()
}

/// A new ID like `U07ABCDEF12`: prefix plus 10 uppercase alphanumerics.
pub fn new_id(kind: IdKind) -> String {
    format!("{}{}", kind.prefix(), random_string(ID_ALPHABET, 10))
}

/// An emulator-internal ID (accounts, identities) that is never shown to bots.
pub fn new_internal_id() -> String {
    random_string(TOKEN_ALPHABET, 20)
}

/// A short random identifier, like the `block_id`s Slack fills in (`"Xk3pQ"`).
pub fn new_short_id(len: usize) -> String {
    random_string(TOKEN_ALPHABET, len)
}

/// A random secret suitable for session cookies.
pub fn new_secret() -> String {
    random_string(TOKEN_ALPHABET, 40)
}

/// Kinds of Slack API token, distinguished by prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TokenKind {
    /// `xoxb-`: bot token.
    Bot,
    /// `xoxp-`: user token issued by an app install.
    User,
    /// `xapp-`: app-level token (Socket Mode).
    App,
}

impl TokenKind {
    pub fn prefix(self) -> &'static str {
        match self {
            TokenKind::Bot => "xoxb",
            TokenKind::User => "xoxp",
            TokenKind::App => "xapp",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Bot => "bot",
            TokenKind::User => "user",
            TokenKind::App => "app",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "bot" => Some(TokenKind::Bot),
            "user" => Some(TokenKind::User),
            "app" => Some(TokenKind::App),
            _ => None,
        }
    }
}

/// A token shaped like Slack's, e.g. `xoxb-1234567890-1234567890-AbC...`.
pub fn new_api_token(kind: TokenKind) -> String {
    let digits = b"0123456789";
    format!(
        "{}-{}-{}-{}",
        kind.prefix(),
        random_string(digits, 12),
        random_string(digits, 13),
        random_string(TOKEN_ALPHABET, 24)
    )
}

/// SHA-256 hex digest, used to store session secrets at rest.
pub fn sha256_hex(input: &str) -> String {
    hex::encode(Sha256::digest(input.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_shape() {
        let id = new_id(IdKind::User);
        assert_eq!(id.len(), 11);
        assert!(id.starts_with('U'));
        assert!(id.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()));
    }

    #[test]
    fn token_shape() {
        let t = new_api_token(TokenKind::Bot);
        assert!(t.starts_with("xoxb-"));
        assert_eq!(t.split('-').count(), 4);
    }
}
