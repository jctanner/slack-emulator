//! Persistence. Everything goes through the [`Store`] trait so the SQLite and
//! Postgres backends are interchangeable. Both run the same SQL (see `backend`).

mod backend;
#[cfg(test)]
mod conformance;
mod postgres;
mod sqlite;

use std::collections::HashMap;

use async_trait::async_trait;
use serde_json::Value;
use slack_emu_core::models::{
    Account, ApiToken, App, Bookmark, Conversation, ConversationCounts, ConversationTypes, File, FileShare, Message,
    NewConversation, NewUser, Pin, Reaction, ScheduledMessage, ThreadSummary, User, Workspace,
};

pub use postgres::{PgStore, fresh_test_database};
pub use sqlite::SqliteStore;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A unique constraint was violated; the string names what collided.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Bounds on a message listing. Unset bounds are open.
#[derive(Debug, Clone, Default)]
pub struct TsRange {
    pub oldest: Option<String>,
    pub latest: Option<String>,
    pub oldest_inclusive: bool,
    pub latest_inclusive: bool,
}

/// Which conversations to list.
#[derive(Debug, Clone)]
pub struct ConversationQuery<'a> {
    pub team_id: &'a str,
    /// Private channels, DMs and group DMs are only listed if this user is a member.
    pub viewer: &'a str,
    pub types: ConversationTypes,
    pub exclude_archived: bool,
    /// Only conversations `viewer` belongs to (also for public channels).
    pub member_only: bool,
    /// Keyset pagination: return IDs after this one.
    pub after_id: Option<&'a str>,
    pub limit: i64,
}

/// Login credentials looked up by provider and subject.
#[derive(Debug, Clone)]
pub struct IdentityMatch {
    pub account: Account,
    pub secret_hash: Option<String>,
}

#[async_trait]
pub trait Store: Send + Sync + 'static {
    // Workspaces
    async fn create_workspace(&self, name: &str, domain: &str) -> Result<Workspace>;
    async fn get_workspace(&self, id: &str) -> Result<Option<Workspace>>;
    async fn get_workspace_by_domain(&self, domain: &str) -> Result<Option<Workspace>>;
    async fn list_workspaces(&self) -> Result<Vec<Workspace>>;

    // Accounts and login
    /// Creates an account with a single identity, atomically.
    async fn create_account(
        &self,
        email: &str,
        display_name: &str,
        provider: &str,
        subject: &str,
        secret_hash: Option<&str>,
    ) -> Result<Account>;
    async fn get_account(&self, id: &str) -> Result<Option<Account>>;
    async fn find_identity(&self, provider: &str, subject: &str) -> Result<Option<IdentityMatch>>;

    // Sessions
    async fn create_session(&self, token_hash: &str, account_id: &str, expires: i64) -> Result<()>;
    /// The account for a live (unexpired) session.
    async fn session_account(&self, token_hash: &str) -> Result<Option<Account>>;
    async fn delete_session(&self, token_hash: &str) -> Result<()>;

    // Users
    async fn create_user(&self, user: NewUser) -> Result<User>;
    /// Saves names, email, roles and the deleted flag.
    async fn update_user(&self, user: &User) -> Result<()>;
    async fn get_user(&self, id: &str) -> Result<Option<User>>;
    async fn user_for_account(&self, team_id: &str, account_id: &str) -> Result<Option<User>>;
    /// Every workspace profile belonging to an account.
    async fn users_for_account(&self, account_id: &str) -> Result<Vec<User>>;
    async fn list_users(&self, team_id: &str) -> Result<Vec<User>>;

    async fn list_users_page(&self, team_id: &str, after_id: Option<&str>, limit: i64) -> Result<Vec<User>>;
    async fn user_by_email(&self, team_id: &str, email: &str) -> Result<Option<User>>;

    // API tokens
    async fn create_token(&self, token: &ApiToken) -> Result<()>;
    async fn get_token(&self, token: &str) -> Result<Option<ApiToken>>;
    /// All tokens issued for an app, including revoked ones.
    async fn app_tokens(&self, app_id: &str) -> Result<Vec<ApiToken>>;
    async fn revoke_token(&self, token: &str) -> Result<()>;
    async fn set_token_scopes(&self, token: &str, scopes: &[String]) -> Result<()>;

    // Apps
    async fn create_app(&self, app: &App) -> Result<()>;
    async fn get_app(&self, id: &str) -> Result<Option<App>>;
    async fn app_by_bot_id(&self, bot_id: &str) -> Result<Option<App>>;
    async fn list_apps(&self, team_id: &str) -> Result<Vec<App>>;
    /// Saves everything except IDs and credentials.
    async fn update_app(&self, app: &App) -> Result<()>;
    /// Deletes the app and its tokens.
    async fn delete_app(&self, id: &str) -> Result<()>;

    /// Deletes everything (the test harness's `/_emu/reset`).
    async fn reset(&self) -> Result<()>;

    // App Home views: one per app and user, stored as opaque JSON.
    async fn home_view(&self, app_id: &str, user_id: &str) -> Result<Option<Value>>;
    async fn home_view_by_id(&self, view_id: &str) -> Result<Option<Value>>;
    async fn home_view_by_external_id(&self, app_id: &str, external_id: &str) -> Result<Option<Value>>;
    /// Inserts or replaces the app's home view for the user.
    async fn save_home_view(
        &self,
        view_id: &str,
        app_id: &str,
        user_id: &str,
        external_id: Option<&str>,
        data: &Value,
    ) -> Result<()>;

    // Conversations
    /// Creates a conversation and its initial members, atomically.
    async fn create_conversation(&self, conv: NewConversation, members: &[String]) -> Result<Conversation>;
    async fn get_conversation(&self, id: &str) -> Result<Option<Conversation>>;
    async fn conversation_by_name(&self, team_id: &str, name: &str) -> Result<Option<Conversation>>;
    async fn conversation_by_dm_key(&self, team_id: &str, dm_key: &str) -> Result<Option<Conversation>>;
    async fn general_conversation(&self, team_id: &str) -> Result<Option<Conversation>>;
    /// Saves name, privacy, archive state, topic and purpose.
    async fn update_conversation(&self, conv: &Conversation) -> Result<()>;
    async fn list_conversations(&self, q: ConversationQuery<'_>) -> Result<Vec<Conversation>>;

    // Membership
    /// Returns false if the user was already a member.
    async fn add_member(&self, conversation_id: &str, user_id: &str) -> Result<bool>;
    /// Returns false if the user wasn't a member.
    async fn remove_member(&self, conversation_id: &str, user_id: &str) -> Result<bool>;
    /// The member's `last_read` ts, or `None` if not a member.
    async fn member_last_read(&self, conversation_id: &str, user_id: &str) -> Result<Option<String>>;
    async fn list_members(&self, conversation_id: &str, after_id: Option<&str>, limit: i64) -> Result<Vec<String>>;
    async fn member_count(&self, conversation_id: &str) -> Result<i64>;
    async fn set_last_read(&self, conversation_id: &str, user_id: &str, ts: &str) -> Result<()>;
    async fn counts_for_user(&self, user_id: &str) -> Result<Vec<ConversationCounts>>;

    // Messages
    /// Inserts a message; a reply also marks its parent as a thread parent.
    async fn insert_message(&self, msg: &Message) -> Result<()>;
    async fn get_message(&self, channel_id: &str, ts: &str) -> Result<Option<Message>>;
    /// Saves text, blocks, attachments, subtype, user and edit info.
    async fn update_message(&self, msg: &Message) -> Result<()>;
    async fn delete_message(&self, channel_id: &str, ts: &str) -> Result<()>;
    /// Channel-level messages (top-level and broadcast replies), newest first.
    async fn history(&self, channel_id: &str, range: &TsRange, limit: i64) -> Result<Vec<Message>>;
    /// A thread's parent and replies, oldest first.
    async fn replies(&self, channel_id: &str, thread_ts: &str, range: &TsRange, limit: i64) -> Result<Vec<Message>>;
    async fn thread_summaries(&self, channel_id: &str, parents: &[String]) -> Result<HashMap<String, ThreadSummary>>;
    /// Newest ts across all messages, for seeding the ts clock.
    async fn max_ts(&self) -> Result<Option<String>>;

    // Files
    async fn create_file(&self, file: &File) -> Result<()>;
    async fn get_file(&self, id: &str) -> Result<Option<File>>;
    /// The files that still exist among `ids`.
    async fn get_files(&self, ids: &[String]) -> Result<Vec<File>>;
    async fn file_by_upload_token(&self, token: &str) -> Result<Option<File>>;
    /// Saves title, size, upload token and state.
    async fn update_file(&self, file: &File) -> Result<()>;
    async fn set_file_data(&self, id: &str, data: &[u8]) -> Result<()>;
    async fn file_data(&self, id: &str) -> Result<Option<Vec<u8>>>;
    /// Deletes the file, its contents and shares (messages keep its ID).
    async fn delete_file(&self, id: &str) -> Result<()>;
    /// Completed files in a workspace, newest first.
    async fn list_files(&self, team_id: &str) -> Result<Vec<File>>;
    async fn add_file_share(&self, share: &FileShare) -> Result<()>;
    async fn file_shares(&self, file_ids: &[String]) -> Result<Vec<FileShare>>;

    // Pins
    /// Returns false if it was already pinned.
    async fn add_pin(&self, pin: &Pin) -> Result<bool>;
    /// Returns false if it wasn't pinned.
    async fn remove_pin(&self, channel_id: &str, ts: &str) -> Result<bool>;
    /// A channel's pins, oldest first.
    async fn pins(&self, channel_id: &str) -> Result<Vec<Pin>>;
    /// Which of these messages are pinned.
    async fn pinned(&self, channel_id: &str, ts: &[String]) -> Result<Vec<String>>;

    // Bookmarks
    async fn add_bookmark(&self, bookmark: &Bookmark) -> Result<()>;
    async fn get_bookmark(&self, id: &str) -> Result<Option<Bookmark>>;
    /// Saves title, link, emoji and who updated it when.
    async fn update_bookmark(&self, bookmark: &Bookmark) -> Result<()>;
    async fn remove_bookmark(&self, id: &str) -> Result<()>;
    /// A channel's bookmarks, oldest first.
    async fn bookmarks(&self, channel_id: &str) -> Result<Vec<Bookmark>>;

    // Scheduled messages
    async fn add_scheduled(&self, msg: &ScheduledMessage) -> Result<()>;
    async fn get_scheduled(&self, id: &str) -> Result<Option<ScheduledMessage>>;
    /// A workspace's pending messages, soonest first.
    async fn list_scheduled(&self, team_id: &str) -> Result<Vec<ScheduledMessage>>;
    /// Messages due at or before `now`, soonest first.
    async fn due_scheduled(&self, now: i64) -> Result<Vec<ScheduledMessage>>;
    /// Returns false if it was already gone (posted or deleted).
    async fn delete_scheduled(&self, id: &str) -> Result<bool>;

    // Reactions
    /// Returns false if the user had already reacted with that emoji.
    async fn add_reaction(&self, channel_id: &str, ts: &str, name: &str, user_id: &str) -> Result<bool>;
    /// Returns false if there was no such reaction.
    async fn remove_reaction(&self, channel_id: &str, ts: &str, name: &str, user_id: &str) -> Result<bool>;
    /// Reactions per message ts, in order of first use.
    async fn reactions(&self, channel_id: &str, ts: &[String]) -> Result<HashMap<String, Vec<Reaction>>>;
}
