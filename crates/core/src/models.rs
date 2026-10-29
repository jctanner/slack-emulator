//! Persistent entities.

use serde::{Deserialize, Serialize};

use crate::ids::TokenKind;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Workspace {
    pub id: String,
    pub name: String,
    /// Short unique handle, like the `acme` in `acme.slack.com`.
    pub domain: String,
    pub created: i64,
}

/// A person who can log in. Global across workspaces and never exposed to bots.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub display_name: String,
    pub created: i64,
}

/// A per-workspace profile (`U…`). Humans link to an account; bot users don't.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub team_id: String,
    pub account_id: Option<String>,
    /// Unique handle within the workspace (legacy `name` field).
    pub name: String,
    pub real_name: String,
    pub display_name: String,
    pub email: Option<String>,
    pub is_bot: bool,
    pub is_admin: bool,
    pub is_owner: bool,
    pub deleted: bool,
    pub tz: String,
    pub created: i64,
}

#[derive(Debug, Clone)]
pub struct NewUser {
    pub team_id: String,
    pub account_id: Option<String>,
    pub name: String,
    pub real_name: String,
    pub display_name: String,
    pub email: Option<String>,
    pub is_bot: bool,
    pub is_admin: bool,
    pub is_owner: bool,
}

/// A Slack API token. Stored in plaintext so the admin UI can show it, which
/// is acceptable for a test emulator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiToken {
    pub token: String,
    pub kind: TokenKind,
    pub team_id: String,
    /// The user the token acts as (the bot user for `xoxb-`); none for `xapp-`.
    pub user_id: Option<String>,
    pub app_id: Option<String>,
    pub bot_id: Option<String>,
    pub scopes: Vec<String>,
    pub created: i64,
    pub revoked: bool,
    /// Label for app-level tokens.
    pub name: Option<String>,
}

impl ApiToken {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Topic {
    pub value: String,
    pub creator: String,
    pub last_set: i64,
}

/// A channel (public or private), DM, or group DM.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Conversation {
    pub id: String,
    pub team_id: String,
    /// `None` for DMs.
    pub name: Option<String>,
    pub is_private: bool,
    pub is_im: bool,
    pub is_mpim: bool,
    pub is_general: bool,
    pub is_archived: bool,
    pub creator: Option<String>,
    pub created: i64,
    pub topic: Topic,
    pub purpose: Topic,
    /// Sorted participant IDs joined by `,` (DMs and group DMs only).
    pub dm_key: Option<String>,
}

impl Conversation {
    pub fn is_channel(&self) -> bool {
        !self.is_im && !self.is_mpim
    }

    /// The Events API `channel_type`.
    pub fn channel_type(&self) -> &'static str {
        if self.is_im {
            "im"
        } else if self.is_mpim {
            "mpim"
        } else if self.is_private {
            "group"
        } else {
            "channel"
        }
    }

    /// DM participant key: sorted, de-duplicated user IDs.
    pub fn dm_key_for(users: &[String]) -> String {
        let mut ids: Vec<&str> = users.iter().map(String::as_str).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.join(",")
    }
}

#[derive(Debug, Clone)]
pub struct NewConversation {
    pub team_id: String,
    pub name: Option<String>,
    pub is_private: bool,
    pub is_im: bool,
    pub is_mpim: bool,
    pub is_general: bool,
    pub creator: Option<String>,
    pub dm_key: Option<String>,
}

/// Which conversation kinds a listing should include.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConversationTypes {
    pub public: bool,
    pub private: bool,
    pub im: bool,
    pub mpim: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Message {
    pub channel_id: String,
    pub ts: String,
    pub team_id: String,
    pub user_id: Option<String>,
    pub bot_id: Option<String>,
    pub app_id: Option<String>,
    pub subtype: Option<String>,
    pub text: String,
    pub blocks: Option<serde_json::Value>,
    pub attachments: Option<serde_json::Value>,
    pub username: Option<String>,
    pub icons: Option<serde_json::Value>,
    pub metadata: Option<serde_json::Value>,
    pub client_msg_id: Option<String>,
    pub thread_ts: Option<String>,
    pub reply_broadcast: bool,
    pub edited_ts: Option<String>,
    pub edited_by: Option<String>,
    /// Files posted with the message (`file_share`).
    #[serde(default)]
    pub file_ids: Vec<String>,
}

impl Message {
    pub fn is_thread_parent(&self) -> bool {
        self.thread_ts.as_deref() == Some(&self.ts)
    }

    /// A reply inside a thread (broadcast or not).
    pub fn is_reply(&self) -> bool {
        self.thread_ts.is_some() && !self.is_thread_parent()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Reaction {
    pub name: String,
    pub users: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ThreadSummary {
    pub reply_count: i64,
    /// Distinct repliers, in order of first reply.
    pub reply_users: Vec<String>,
    pub latest_reply: Option<String>,
}

/// Read state of one conversation for one member (Slack's internal `client.counts`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationCounts {
    pub conversation: Conversation,
    pub last_read: String,
    pub latest: Option<String>,
    /// Unread messages from other people.
    pub unread_count: i64,
    /// Unread messages that mention the member, `@here`, `@channel` or `@everyone`.
    pub mention_count: i64,
}

/// An app, created from a manifest and installable into its workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct App {
    pub id: String,
    pub team_id: String,
    pub name: String,
    pub creator: Option<String>,
    pub created: i64,
    pub updated: i64,
    /// Raw manifest JSON as saved; parse with [`App::manifest`].
    pub manifest_json: serde_json::Value,
    pub client_id: String,
    pub client_secret: String,
    pub signing_secret: String,
    pub verification_token: String,
    pub bot_user_id: Option<String>,
    pub bot_id: Option<String>,
    pub installed_by: Option<String>,
    pub installed_at: Option<i64>,
    pub verified_request_url: Option<String>,
}

impl App {
    pub fn manifest(&self) -> crate::manifest::Manifest {
        serde_json::from_value(self.manifest_json.clone()).unwrap_or_default()
    }

    pub fn is_installed(&self) -> bool {
        self.installed_at.is_some()
    }
}

/// An uploaded file. Its bytes are stored separately.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct File {
    pub id: String,
    pub team_id: String,
    /// The uploader (a person or a bot user).
    pub user_id: String,
    pub bot_id: Option<String>,
    pub name: String,
    pub title: String,
    pub mimetype: String,
    pub filetype: String,
    pub snippet_type: Option<String>,
    pub alt_txt: Option<String>,
    pub size: i64,
    pub created: i64,
    pub upload_token: Option<String>,
    /// The bytes have arrived.
    pub uploaded: bool,
    /// `files.completeUploadExternal` has been called.
    pub completed: bool,
}

/// Where a file was posted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileShare {
    pub file_id: String,
    pub channel_id: String,
    pub ts: String,
}
