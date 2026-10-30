//! The SQL behind both backends. The statements are written once in portable
//! SQL with `?` placeholders. Each backend supplies `prep()`, which adapts a
//! statement to its dialect (Postgres numbers its placeholders `$1, $2, …`),
//! and its row and argument types.
//!
//! Portability rules for statements here: booleans are `TRUE`/`FALSE`,
//! substring tests use `LIKE`, and tables ordered by insertion (`api_tokens`,
//! `reactions`) have a `rowid`. SQLite provides it implicitly; the Postgres
//! schema declares it.

macro_rules! sql_store {
    ($store:ident, $row:ty, $args:ty) => {
        use std::collections::HashMap;

        use serde_json::Value;
        use slack_emu_core::ids::{IdKind, TokenKind, new_id, new_internal_id};
        use slack_emu_core::models::{
            Account, ApiToken, App, Bookmark, Conversation, ConversationCounts, File, FileShare, Message, NewConversation,
            NewUser, Pin, Reaction, ScheduledMessage, Usergroup, ThreadSummary, Topic, User, Workspace,
        };
        use slack_emu_core::now;
        use sqlx::{Arguments, Row};

        use crate::{ConversationQuery, IdentityMatch, MessageSearch, Result, Store, StoreError, TsRange};

        /// Messages that show at channel level: top-level posts, thread parents and broadcast replies.
        const CHANNEL_LEVEL: &str = "(x.thread_ts IS NULL OR x.thread_ts = x.ts OR x.reply_broadcast = TRUE)";

        /// Maps unique-constraint violations to [`StoreError::Conflict`].
        fn conflict(what: &str) -> impl FnOnce(sqlx::Error) -> StoreError + '_ {
            move |e| match &e {
                sqlx::Error::Database(db) if db.is_unique_violation() => StoreError::Conflict(what.into()),
                _ => StoreError::Db(e),
            }
        }

        fn workspace_row(r: $row) -> Workspace {
            Workspace {
                id: r.get("id"),
                name: r.get("name"),
                domain: r.get("domain"),
                created: r.get("created"),
            }
        }

        fn account_row(r: &$row) -> Account {
            Account {
                id: r.get("id"),
                email: r.get("email"),
                display_name: r.get("display_name"),
                created: r.get("created"),
            }
        }

        fn user_row(r: $row) -> User {
            User {
                id: r.get("id"),
                team_id: r.get("team_id"),
                account_id: r.get("account_id"),
                name: r.get("name"),
                real_name: r.get("real_name"),
                display_name: r.get("display_name"),
                email: r.get("email"),
                is_bot: r.get("is_bot"),
                is_admin: r.get("is_admin"),
                is_owner: r.get("is_owner"),
                deleted: r.get("deleted"),
                tz: r.get("tz"),
                created: r.get("created"),
            }
        }

        fn token_row(r: $row) -> ApiToken {
            let kind: String = r.get("kind");
            let scopes: String = r.get("scopes");
            ApiToken {
                token: r.get("token"),
                kind: TokenKind::parse(&kind).unwrap_or(TokenKind::Bot),
                team_id: r.get("team_id"),
                user_id: r.get("user_id"),
                app_id: r.get("app_id"),
                bot_id: r.get("bot_id"),
                scopes: scopes.split(',').filter(|s| !s.is_empty()).map(str::to_owned).collect(),
                created: r.get("created"),
                revoked: r.get("revoked"),
                name: r.get("name"),
            }
        }

        fn app_row(r: $row) -> App {
            App {
                id: r.get("id"),
                team_id: r.get("team_id"),
                name: r.get("name"),
                creator: r.get("creator"),
                created: r.get("created"),
                updated: r.get("updated"),
                manifest_json: json_col(&r, "manifest").unwrap_or(Value::Null),
                client_id: r.get("client_id"),
                client_secret: r.get("client_secret"),
                signing_secret: r.get("signing_secret"),
                verification_token: r.get("verification_token"),
                bot_user_id: r.get("bot_user_id"),
                bot_id: r.get("bot_id"),
                installed_by: r.get("installed_by"),
                installed_at: r.get("installed_at"),
                verified_request_url: r.get("verified_request_url"),
            }
        }

        fn conversation_row(r: &$row) -> Conversation {
            Conversation {
                id: r.get("id"),
                team_id: r.get("team_id"),
                name: r.get("name"),
                is_private: r.get("is_private"),
                is_im: r.get("is_im"),
                is_mpim: r.get("is_mpim"),
                is_general: r.get("is_general"),
                is_archived: r.get("is_archived"),
                creator: r.get("creator"),
                created: r.get("created"),
                topic: Topic {
                    value: r.get("topic"),
                    creator: r.get("topic_creator"),
                    last_set: r.get("topic_last_set"),
                },
                purpose: Topic {
                    value: r.get("purpose"),
                    creator: r.get("purpose_creator"),
                    last_set: r.get("purpose_last_set"),
                },
                dm_key: r.get("dm_key"),
            }
        }

        fn json_col(r: &$row, col: &str) -> Option<Value> {
            r.get::<Option<String>, _>(col)
                .and_then(|s| serde_json::from_str(&s).ok())
        }

        fn json_text(v: &Option<Value>) -> Option<String> {
            v.as_ref().map(Value::to_string)
        }

        fn message_row(r: $row) -> Message {
            Message {
                channel_id: r.get("channel_id"),
                ts: r.get("ts"),
                team_id: r.get("team_id"),
                user_id: r.get("user_id"),
                bot_id: r.get("bot_id"),
                app_id: r.get("app_id"),
                subtype: r.get("subtype"),
                text: r.get("text"),
                blocks: json_col(&r, "blocks"),
                attachments: json_col(&r, "attachments"),
                username: r.get("username"),
                icons: json_col(&r, "icons"),
                metadata: json_col(&r, "metadata"),
                client_msg_id: r.get("client_msg_id"),
                thread_ts: r.get("thread_ts"),
                reply_broadcast: r.get("reply_broadcast"),
                edited_ts: r.get("edited_ts"),
                edited_by: r.get("edited_by"),
                file_ids: split_list(&r.get::<String, _>("file_ids")),
            }
        }

        fn pin_row(r: $row) -> Pin {
            Pin {
                channel_id: r.get("channel_id"),
                ts: r.get("ts"),
                pinned_by: r.get("pinned_by"),
                created: r.get("created"),
            }
        }

        fn bookmark_row(r: $row) -> Bookmark {
            Bookmark {
                id: r.get("id"),
                channel_id: r.get("channel_id"),
                title: r.get("title"),
                link: r.get("link"),
                emoji: r.get("emoji"),
                app_id: r.get("app_id"),
                created: r.get("created"),
                updated: r.get("updated"),
                updated_by: r.get("updated_by"),
            }
        }

        fn scheduled_row(r: $row) -> ScheduledMessage {
            ScheduledMessage {
                id: r.get("id"),
                team_id: r.get("team_id"),
                channel_id: r.get("channel_id"),
                user_id: r.get("user_id"),
                token: r.get("token"),
                app_id: r.get("app_id"),
                post_at: r.get("post_at"),
                created: r.get("created"),
                args: json_col(&r, "args").unwrap_or(Value::Null),
            }
        }

        fn usergroup_row(r: $row) -> Usergroup {
            Usergroup {
                id: r.get("id"),
                team_id: r.get("team_id"),
                name: r.get("name"),
                handle: r.get("handle"),
                description: r.get("description"),
                channels: split_list(&r.get::<String, _>("channels")),
                created_by: r.get("created_by"),
                updated_by: r.get("updated_by"),
                created: r.get("created"),
                updated: r.get("updated"),
                disabled_at: r.get("disabled_at"),
                deleted_by: r.get("deleted_by"),
            }
        }

        fn split_list(s: &str) -> Vec<String> {
            s.split(',').filter(|x| !x.is_empty()).map(str::to_owned).collect()
        }

        fn file_row(r: $row) -> File {
            File {
                id: r.get("id"),
                team_id: r.get("team_id"),
                user_id: r.get("user_id"),
                bot_id: r.get("bot_id"),
                name: r.get("name"),
                title: r.get("title"),
                mimetype: r.get("mimetype"),
                filetype: r.get("filetype"),
                snippet_type: r.get("snippet_type"),
                alt_txt: r.get("alt_txt"),
                size: r.get("size"),
                created: r.get("created"),
                upload_token: r.get("upload_token"),
                uploaded: r.get("uploaded"),
                completed: r.get("completed"),
            }
        }

        /// `?, ?, ?` for an IN list.
        fn placeholders(n: usize) -> String {
            vec!["?"; n].join(", ")
        }

        /// Appends range conditions on `x.ts` to `sql`, binding their values.
        fn push_range(sql: &mut String, args: &mut $args, range: &TsRange) {
            if let Some(o) = &range.oldest {
                sql.push_str(if range.oldest_inclusive {
                    " AND x.ts >= ?"
                } else {
                    " AND x.ts > ?"
                });
                args.add(o.clone()).expect("bind");
            }
            if let Some(l) = &range.latest {
                sql.push_str(if range.latest_inclusive {
                    " AND x.ts <= ?"
                } else {
                    " AND x.ts < ?"
                });
                args.add(l.clone()).expect("bind");
            }
        }

        #[async_trait::async_trait]
        impl Store for $store {
            async fn create_workspace(&self, name: &str, domain: &str) -> Result<Workspace> {
                let ws = Workspace {
                    id: new_id(IdKind::Team),
                    name: name.into(),
                    domain: domain.into(),
                    created: now(),
                };
                sqlx::query(prep("INSERT INTO workspaces (id, name, domain, created) VALUES (?, ?, ?, ?)"))
                    .bind(&ws.id)
                    .bind(&ws.name)
                    .bind(&ws.domain)
                    .bind(ws.created)
                    .execute(&self.pool)
                    .await
                    .map_err(conflict("domain"))?;
                Ok(ws)
            }

            async fn get_workspace(&self, id: &str) -> Result<Option<Workspace>> {
                Ok(sqlx::query(prep("SELECT * FROM workspaces WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(workspace_row))
            }

            async fn get_workspace_by_domain(&self, domain: &str) -> Result<Option<Workspace>> {
                Ok(sqlx::query(prep("SELECT * FROM workspaces WHERE domain = ?"))
                    .bind(domain)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(workspace_row))
            }

            async fn list_workspaces(&self) -> Result<Vec<Workspace>> {
                Ok(sqlx::query(prep("SELECT * FROM workspaces ORDER BY created, name"))
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(workspace_row)
                    .collect())
            }

            async fn create_account(
                &self,
                email: &str,
                display_name: &str,
                provider: &str,
                subject: &str,
                secret_hash: Option<&str>,
            ) -> Result<Account> {
                let account = Account {
                    id: new_internal_id(),
                    email: email.into(),
                    display_name: display_name.into(),
                    created: now(),
                };
                let mut tx = self.pool.begin().await?;
                sqlx::query(prep("INSERT INTO accounts (id, email, display_name, created) VALUES (?, ?, ?, ?)"))
                    .bind(&account.id)
                    .bind(&account.email)
                    .bind(&account.display_name)
                    .bind(account.created)
                    .execute(&mut *tx)
                    .await
                    .map_err(conflict("email"))?;
                sqlx::query(
                    prep("INSERT INTO identities (id, account_id, provider, subject, secret_hash, created)
                     VALUES (?, ?, ?, ?, ?, ?)"),
                )
                .bind(new_internal_id())
                .bind(&account.id)
                .bind(provider)
                .bind(subject)
                .bind(secret_hash)
                .bind(account.created)
                .execute(&mut *tx)
                .await
                .map_err(conflict("identity"))?;
                tx.commit().await?;
                Ok(account)
            }

            async fn get_account(&self, id: &str) -> Result<Option<Account>> {
                Ok(sqlx::query(prep("SELECT * FROM accounts WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(|r| account_row(&r)))
            }

            async fn find_identity(&self, provider: &str, subject: &str) -> Result<Option<IdentityMatch>> {
                Ok(sqlx::query(
                    prep("SELECT a.*, i.secret_hash FROM identities i
                     JOIN accounts a ON a.id = i.account_id
                     WHERE i.provider = ? AND i.subject = ?"),
                )
                .bind(provider)
                .bind(subject)
                .fetch_optional(&self.pool)
                .await?
                .map(|r| IdentityMatch {
                    account: account_row(&r),
                    secret_hash: r.get("secret_hash"),
                }))
            }

            async fn create_session(&self, token_hash: &str, account_id: &str, expires: i64) -> Result<()> {
                sqlx::query(prep("INSERT INTO sessions (token_hash, account_id, created, expires) VALUES (?, ?, ?, ?)"))
                    .bind(token_hash)
                    .bind(account_id)
                    .bind(now())
                    .bind(expires)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn session_account(&self, token_hash: &str) -> Result<Option<Account>> {
                Ok(sqlx::query(
                    prep("SELECT a.* FROM sessions s JOIN accounts a ON a.id = s.account_id
                     WHERE s.token_hash = ? AND s.expires > ?"),
                )
                .bind(token_hash)
                .bind(now())
                .fetch_optional(&self.pool)
                .await?
                .map(|r| account_row(&r)))
            }

            async fn delete_session(&self, token_hash: &str) -> Result<()> {
                sqlx::query(prep("DELETE FROM sessions WHERE token_hash = ?"))
                    .bind(token_hash)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn create_user(&self, u: NewUser) -> Result<User> {
                let user = User {
                    id: new_id(IdKind::User),
                    team_id: u.team_id,
                    account_id: u.account_id,
                    name: u.name,
                    real_name: u.real_name,
                    display_name: u.display_name,
                    email: u.email,
                    is_bot: u.is_bot,
                    is_admin: u.is_admin,
                    is_owner: u.is_owner,
                    deleted: false,
                    tz: "UTC".into(),
                    created: now(),
                };
                sqlx::query(
                    prep("INSERT INTO users (id, team_id, account_id, name, real_name, display_name, email,
                                        is_bot, is_admin, is_owner, deleted, tz, created)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
                )
                .bind(&user.id)
                .bind(&user.team_id)
                .bind(&user.account_id)
                .bind(&user.name)
                .bind(&user.real_name)
                .bind(&user.display_name)
                .bind(&user.email)
                .bind(user.is_bot)
                .bind(user.is_admin)
                .bind(user.is_owner)
                .bind(user.deleted)
                .bind(&user.tz)
                .bind(user.created)
                .execute(&self.pool)
                .await
                .map_err(conflict("user"))?;
                Ok(user)
            }

            async fn update_user(&self, u: &User) -> Result<()> {
                sqlx::query(
                    prep("UPDATE users SET name = ?, real_name = ?, display_name = ?, email = ?, is_admin = ?, is_owner = ?,
                                      deleted = ?, tz = ?
                     WHERE id = ?"),
                )
                .bind(&u.name)
                .bind(&u.real_name)
                .bind(&u.display_name)
                .bind(&u.email)
                .bind(u.is_admin)
                .bind(u.is_owner)
                .bind(u.deleted)
                .bind(&u.tz)
                .bind(&u.id)
                .execute(&self.pool)
                .await
                .map_err(conflict("user"))?;
                Ok(())
            }

            async fn get_user(&self, id: &str) -> Result<Option<User>> {
                Ok(sqlx::query(prep("SELECT * FROM users WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(user_row))
            }

            async fn user_for_account(&self, team_id: &str, account_id: &str) -> Result<Option<User>> {
                Ok(sqlx::query(prep("SELECT * FROM users WHERE team_id = ? AND account_id = ?"))
                    .bind(team_id)
                    .bind(account_id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(user_row))
            }

            async fn users_for_account(&self, account_id: &str) -> Result<Vec<User>> {
                Ok(sqlx::query(prep("SELECT * FROM users WHERE account_id = ? ORDER BY created"))
                    .bind(account_id)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(user_row)
                    .collect())
            }

            async fn list_users(&self, team_id: &str) -> Result<Vec<User>> {
                Ok(sqlx::query(prep("SELECT * FROM users WHERE team_id = ? ORDER BY name"))
                    .bind(team_id)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(user_row)
                    .collect())
            }

            async fn create_token(&self, t: &ApiToken) -> Result<()> {
                sqlx::query(
                    prep("INSERT INTO api_tokens (token, kind, team_id, user_id, app_id, bot_id, scopes, created, revoked, name)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
                )
                .bind(&t.token)
                .bind(t.kind.as_str())
                .bind(&t.team_id)
                .bind(&t.user_id)
                .bind(&t.app_id)
                .bind(&t.bot_id)
                .bind(t.scopes.join(","))
                .bind(t.created)
                .bind(t.revoked)
                .bind(&t.name)
                .execute(&self.pool)
                .await
                .map_err(conflict("token"))?;
                Ok(())
            }

            async fn get_token(&self, token: &str) -> Result<Option<ApiToken>> {
                Ok(sqlx::query(prep("SELECT * FROM api_tokens WHERE token = ?"))
                    .bind(token)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(token_row))
            }

            async fn app_tokens(&self, app_id: &str) -> Result<Vec<ApiToken>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM api_tokens WHERE app_id = ? ORDER BY created, rowid"))
                        .bind(app_id)
                        .fetch_all(&self.pool)
                        .await?
                        .into_iter()
                        .map(token_row)
                        .collect(),
                )
            }

            async fn revoke_token(&self, token: &str) -> Result<()> {
                sqlx::query(prep("UPDATE api_tokens SET revoked = TRUE WHERE token = ?"))
                    .bind(token)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn set_token_scopes(&self, token: &str, scopes: &[String]) -> Result<()> {
                sqlx::query(prep("UPDATE api_tokens SET scopes = ? WHERE token = ?"))
                    .bind(scopes.join(","))
                    .bind(token)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn create_app(&self, a: &App) -> Result<()> {
                sqlx::query(
                    prep("INSERT INTO apps (id, team_id, name, creator, created, updated, manifest, client_id, client_secret,
                                       signing_secret, verification_token, bot_user_id, bot_id, installed_by, installed_at,
                                       verified_request_url)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
                )
                .bind(&a.id)
                .bind(&a.team_id)
                .bind(&a.name)
                .bind(&a.creator)
                .bind(a.created)
                .bind(a.updated)
                .bind(a.manifest_json.to_string())
                .bind(&a.client_id)
                .bind(&a.client_secret)
                .bind(&a.signing_secret)
                .bind(&a.verification_token)
                .bind(&a.bot_user_id)
                .bind(&a.bot_id)
                .bind(&a.installed_by)
                .bind(a.installed_at)
                .bind(&a.verified_request_url)
                .execute(&self.pool)
                .await
                .map_err(conflict("app"))?;
                Ok(())
            }

            async fn get_app(&self, id: &str) -> Result<Option<App>> {
                Ok(sqlx::query(prep("SELECT * FROM apps WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(app_row))
            }

            async fn app_by_bot_id(&self, bot_id: &str) -> Result<Option<App>> {
                Ok(sqlx::query(prep("SELECT * FROM apps WHERE bot_id = ?"))
                    .bind(bot_id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(app_row))
            }

            async fn list_apps(&self, team_id: &str) -> Result<Vec<App>> {
                Ok(sqlx::query(prep("SELECT * FROM apps WHERE team_id = ? ORDER BY name, id"))
                    .bind(team_id)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(app_row)
                    .collect())
            }

            async fn update_app(&self, a: &App) -> Result<()> {
                sqlx::query(
                    prep("UPDATE apps SET name = ?, updated = ?, manifest = ?, bot_user_id = ?, bot_id = ?, installed_by = ?,
                                     installed_at = ?, verified_request_url = ?
                     WHERE id = ?"),
                )
                .bind(&a.name)
                .bind(a.updated)
                .bind(a.manifest_json.to_string())
                .bind(&a.bot_user_id)
                .bind(&a.bot_id)
                .bind(&a.installed_by)
                .bind(a.installed_at)
                .bind(&a.verified_request_url)
                .bind(&a.id)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn delete_app(&self, id: &str) -> Result<()> {
                let mut tx = self.pool.begin().await?;
                sqlx::query(prep("DELETE FROM api_tokens WHERE app_id = ?"))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(prep("DELETE FROM apps WHERE id = ?"))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(())
            }

            async fn reset(&self) -> Result<()> {
                let mut tx = self.pool.begin().await?;
                // Children before parents, so foreign keys hold throughout.
                for table in [
                    "sessions",
                    "identities",
                    "usergroup_members",
                    "usergroups",
                    "scheduled_messages",
                    "pins",
                    "bookmarks",
                    "file_shares",
                    "file_contents",
                    "files",
                    "reactions",
                    "messages",
                    "conversation_members",
                    "conversations",
                    "home_views",
                    "api_tokens",
                    "apps",
                    "users",
                    "accounts",
                    "workspaces",
                ] {
                    sqlx::query(prep(&format!("DELETE FROM {table}")))
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(())
            }

            async fn home_view(&self, app_id: &str, user_id: &str) -> Result<Option<Value>> {
                let row: Option<String> = sqlx::query_scalar(prep("SELECT data FROM home_views WHERE app_id = ? AND user_id = ?"))
                    .bind(app_id)
                    .bind(user_id)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.and_then(|s| serde_json::from_str(&s).ok()))
            }

            async fn home_view_by_id(&self, view_id: &str) -> Result<Option<Value>> {
                let row: Option<String> = sqlx::query_scalar(prep("SELECT data FROM home_views WHERE view_id = ?"))
                    .bind(view_id)
                    .fetch_optional(&self.pool)
                    .await?;
                Ok(row.and_then(|s| serde_json::from_str(&s).ok()))
            }

            async fn home_view_by_external_id(&self, app_id: &str, external_id: &str) -> Result<Option<Value>> {
                let row: Option<String> =
                    sqlx::query_scalar(prep("SELECT data FROM home_views WHERE app_id = ? AND external_id = ? LIMIT 1"))
                        .bind(app_id)
                        .bind(external_id)
                        .fetch_optional(&self.pool)
                        .await?;
                Ok(row.and_then(|s| serde_json::from_str(&s).ok()))
            }

            async fn save_home_view(
                &self,
                view_id: &str,
                app_id: &str,
                user_id: &str,
                external_id: Option<&str>,
                data: &Value,
            ) -> Result<()> {
                sqlx::query(
                    prep("INSERT INTO home_views (view_id, app_id, user_id, external_id, data) VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT (app_id, user_id) DO UPDATE SET
                         view_id = excluded.view_id, external_id = excluded.external_id, data = excluded.data"),
                )
                .bind(view_id)
                .bind(app_id)
                .bind(user_id)
                .bind(external_id)
                .bind(data.to_string())
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn list_users_page(&self, team_id: &str, after_id: Option<&str>, limit: i64) -> Result<Vec<User>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM users WHERE team_id = ? AND id > ? ORDER BY id LIMIT ?"))
                        .bind(team_id)
                        .bind(after_id.unwrap_or(""))
                        .bind(limit)
                        .fetch_all(&self.pool)
                        .await?
                        .into_iter()
                        .map(user_row)
                        .collect(),
                )
            }

            async fn user_by_email(&self, team_id: &str, email: &str) -> Result<Option<User>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM users WHERE team_id = ? AND lower(email) = lower(?)"))
                        .bind(team_id)
                        .bind(email)
                        .fetch_optional(&self.pool)
                        .await?
                        .map(user_row),
                )
            }

            async fn create_conversation(&self, c: NewConversation, members: &[String]) -> Result<Conversation> {
                let kind = if c.is_im { IdKind::Dm } else { IdKind::Channel };
                let conv = Conversation {
                    id: new_id(kind),
                    team_id: c.team_id,
                    name: c.name,
                    is_private: c.is_private,
                    is_im: c.is_im,
                    is_mpim: c.is_mpim,
                    is_general: c.is_general,
                    is_archived: false,
                    creator: c.creator,
                    created: now(),
                    topic: Topic::default(),
                    purpose: Topic::default(),
                    dm_key: c.dm_key,
                };
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    prep("INSERT INTO conversations (id, team_id, name, is_private, is_im, is_mpim, is_general, creator, created, dm_key)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
                )
                .bind(&conv.id)
                .bind(&conv.team_id)
                .bind(&conv.name)
                .bind(conv.is_private)
                .bind(conv.is_im)
                .bind(conv.is_mpim)
                .bind(conv.is_general)
                .bind(&conv.creator)
                .bind(conv.created)
                .bind(&conv.dm_key)
                .execute(&mut *tx)
                .await
                .map_err(conflict("name"))?;
                for user in members {
                    sqlx::query(
                        prep("INSERT INTO conversation_members (conversation_id, user_id, joined) VALUES (?, ?, ?)
                         ON CONFLICT DO NOTHING"),
                    )
                    .bind(&conv.id)
                    .bind(user)
                    .bind(conv.created)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(conv)
            }

            async fn get_conversation(&self, id: &str) -> Result<Option<Conversation>> {
                Ok(sqlx::query(prep("SELECT * FROM conversations WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(|r| conversation_row(&r)))
            }

            async fn conversation_by_name(&self, team_id: &str, name: &str) -> Result<Option<Conversation>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM conversations WHERE team_id = ? AND name = ?"))
                        .bind(team_id)
                        .bind(name)
                        .fetch_optional(&self.pool)
                        .await?
                        .map(|r| conversation_row(&r)),
                )
            }

            async fn conversation_by_dm_key(&self, team_id: &str, dm_key: &str) -> Result<Option<Conversation>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM conversations WHERE team_id = ? AND dm_key = ?"))
                        .bind(team_id)
                        .bind(dm_key)
                        .fetch_optional(&self.pool)
                        .await?
                        .map(|r| conversation_row(&r)),
                )
            }

            async fn general_conversation(&self, team_id: &str) -> Result<Option<Conversation>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM conversations WHERE team_id = ? AND is_general = TRUE"))
                        .bind(team_id)
                        .fetch_optional(&self.pool)
                        .await?
                        .map(|r| conversation_row(&r)),
                )
            }

            async fn update_conversation(&self, c: &Conversation) -> Result<()> {
                sqlx::query(
                    prep("UPDATE conversations SET name = ?, is_private = ?, is_archived = ?,
                         topic = ?, topic_creator = ?, topic_last_set = ?,
                         purpose = ?, purpose_creator = ?, purpose_last_set = ?
                     WHERE id = ?"),
                )
                .bind(&c.name)
                .bind(c.is_private)
                .bind(c.is_archived)
                .bind(&c.topic.value)
                .bind(&c.topic.creator)
                .bind(c.topic.last_set)
                .bind(&c.purpose.value)
                .bind(&c.purpose.creator)
                .bind(c.purpose.last_set)
                .bind(&c.id)
                .execute(&self.pool)
                .await
                .map_err(conflict("name"))?;
                Ok(())
            }

            async fn list_conversations(&self, q: ConversationQuery<'_>) -> Result<Vec<Conversation>> {
                const MEMBER: &str =
                    "EXISTS (SELECT 1 FROM conversation_members m WHERE m.conversation_id = c.id AND m.user_id = ?)";
                let mut args = <$args>::default();
                let mut kinds = Vec::new();
                let mut viewer_binds = 0;
                if q.types.public {
                    if q.member_only {
                        kinds.push(format!(
                            "(c.is_private = FALSE AND c.is_im = FALSE AND c.is_mpim = FALSE AND {MEMBER})"
                        ));
                        viewer_binds += 1;
                    } else {
                        kinds.push("(c.is_private = FALSE AND c.is_im = FALSE AND c.is_mpim = FALSE)".to_owned());
                    }
                }
                for (wanted, cond) in [
                    (q.types.private, "c.is_private = TRUE AND c.is_im = FALSE AND c.is_mpim = FALSE"),
                    (q.types.im, "c.is_im = TRUE"),
                    (q.types.mpim, "c.is_mpim = TRUE"),
                ] {
                    if wanted {
                        kinds.push(format!("({cond} AND {MEMBER})"));
                        viewer_binds += 1;
                    }
                }
                if kinds.is_empty() {
                    return Ok(Vec::new());
                }
                let mut sql = format!(
                    "SELECT c.* FROM conversations c WHERE c.team_id = ? AND c.id > ? AND ({})",
                    kinds.join(" OR ")
                );
                args.add(q.team_id).expect("bind");
                args.add(q.after_id.unwrap_or("")).expect("bind");
                for _ in 0..viewer_binds {
                    args.add(q.viewer).expect("bind");
                }
                if q.exclude_archived {
                    sql.push_str(" AND c.is_archived = FALSE");
                }
                sql.push_str(" ORDER BY c.id LIMIT ?");
                args.add(q.limit).expect("bind");
                Ok(sqlx::query_with(prep(&sql), args)
                    .fetch_all(&self.pool)
                    .await?
                    .iter()
                    .map(conversation_row)
                    .collect())
            }

            async fn add_member(&self, conversation_id: &str, user_id: &str) -> Result<bool> {
                let r = sqlx::query(
                    prep("INSERT INTO conversation_members (conversation_id, user_id, joined) VALUES (?, ?, ?)
                     ON CONFLICT DO NOTHING"),
                )
                .bind(conversation_id)
                .bind(user_id)
                .bind(now())
                .execute(&self.pool)
                .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn remove_member(&self, conversation_id: &str, user_id: &str) -> Result<bool> {
                let r = sqlx::query(prep("DELETE FROM conversation_members WHERE conversation_id = ? AND user_id = ?"))
                    .bind(conversation_id)
                    .bind(user_id)
                    .execute(&self.pool)
                    .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn member_last_read(&self, conversation_id: &str, user_id: &str) -> Result<Option<String>> {
                Ok(
                    sqlx::query(prep("SELECT last_read FROM conversation_members WHERE conversation_id = ? AND user_id = ?"))
                        .bind(conversation_id)
                        .bind(user_id)
                        .fetch_optional(&self.pool)
                        .await?
                        .map(|r| r.get("last_read")),
                )
            }

            async fn list_members(&self, conversation_id: &str, after_id: Option<&str>, limit: i64) -> Result<Vec<String>> {
                Ok(sqlx::query(
                    prep("SELECT user_id FROM conversation_members WHERE conversation_id = ? AND user_id > ?
                     ORDER BY user_id LIMIT ?"),
                )
                .bind(conversation_id)
                .bind(after_id.unwrap_or(""))
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(|r| r.get("user_id"))
                .collect())
            }

            async fn member_count(&self, conversation_id: &str) -> Result<i64> {
                Ok(
                    sqlx::query(prep("SELECT COUNT(*) AS n FROM conversation_members WHERE conversation_id = ?"))
                        .bind(conversation_id)
                        .fetch_one(&self.pool)
                        .await?
                        .get("n"),
                )
            }

            async fn set_last_read(&self, conversation_id: &str, user_id: &str, ts: &str) -> Result<()> {
                sqlx::query(prep("UPDATE conversation_members SET last_read = ? WHERE conversation_id = ? AND user_id = ?"))
                    .bind(ts)
                    .bind(conversation_id)
                    .bind(user_id)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn counts_for_user(&self, user_id: &str) -> Result<Vec<ConversationCounts>> {
                // Unread = channel-level messages after last_read from someone else,
                // excluding join/leave/topic-style system messages.
                let unread = format!(
                    "x.channel_id = c.id AND x.ts > m.last_read AND {CHANNEL_LEVEL}
                     AND (x.user_id IS NULL OR x.user_id != m.user_id)
                     AND (x.subtype IS NULL OR x.subtype IN ('bot_message', 'thread_broadcast', 'me_message', 'file_share'))"
                );
                let sql = format!(
                    "SELECT c.*, m.last_read,
                        (SELECT MAX(x.ts) FROM messages x WHERE x.channel_id = c.id AND {CHANNEL_LEVEL}) AS latest,
                        (SELECT COUNT(*) FROM messages x WHERE {unread}) AS unread_count,
                        (SELECT COUNT(*) FROM messages x WHERE {unread}
                            AND (x.text LIKE '%<@' || m.user_id || '>%' OR x.text LIKE '%<!here%'
                                 OR x.text LIKE '%<!channel%' OR x.text LIKE '%<!everyone%'
                            OR EXISTS (SELECT 1 FROM usergroup_members g JOIN usergroups ug ON ug.id = g.usergroup_id
                                       WHERE g.user_id = m.user_id AND ug.disabled_at IS NULL
                                         AND x.text LIKE '%<!subteam^' || g.usergroup_id || '%'))) AS mention_count
                     FROM conversation_members m JOIN conversations c ON c.id = m.conversation_id
                     WHERE m.user_id = ?
                     ORDER BY c.id"
                );
                Ok(sqlx::query(prep(&sql))
                    .bind(user_id)
                    .fetch_all(&self.pool)
                    .await?
                    .iter()
                    .map(|r| ConversationCounts {
                        conversation: conversation_row(r),
                        last_read: r.get("last_read"),
                        latest: r.get("latest"),
                        unread_count: r.get("unread_count"),
                        mention_count: r.get("mention_count"),
                    })
                    .collect())
            }

            async fn insert_message(&self, m: &Message) -> Result<()> {
                let mut tx = self.pool.begin().await?;
                sqlx::query(
                    prep("INSERT INTO messages (channel_id, ts, team_id, user_id, bot_id, app_id, subtype, text, blocks,
                                           attachments, username, icons, metadata, client_msg_id, thread_ts,
                                           reply_broadcast, edited_ts, edited_by, file_ids)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"),
                )
                .bind(&m.channel_id)
                .bind(&m.ts)
                .bind(&m.team_id)
                .bind(&m.user_id)
                .bind(&m.bot_id)
                .bind(&m.app_id)
                .bind(&m.subtype)
                .bind(&m.text)
                .bind(json_text(&m.blocks))
                .bind(json_text(&m.attachments))
                .bind(&m.username)
                .bind(json_text(&m.icons))
                .bind(json_text(&m.metadata))
                .bind(&m.client_msg_id)
                .bind(&m.thread_ts)
                .bind(m.reply_broadcast)
                .bind(&m.edited_ts)
                .bind(&m.edited_by)
                .bind(m.file_ids.join(","))
                .execute(&mut *tx)
                .await
                .map_err(conflict("ts"))?;
                if m.is_reply() {
                    sqlx::query(prep("UPDATE messages SET thread_ts = ts WHERE channel_id = ? AND ts = ? AND thread_ts IS NULL"))
                        .bind(&m.channel_id)
                        .bind(&m.thread_ts)
                        .execute(&mut *tx)
                        .await?;
                }
                tx.commit().await?;
                Ok(())
            }

            async fn get_message(&self, channel_id: &str, ts: &str) -> Result<Option<Message>> {
                Ok(sqlx::query(prep("SELECT * FROM messages WHERE channel_id = ? AND ts = ?"))
                    .bind(channel_id)
                    .bind(ts)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(message_row))
            }

            async fn update_message(&self, m: &Message) -> Result<()> {
                sqlx::query(
                    prep("UPDATE messages SET user_id = ?, subtype = ?, text = ?, blocks = ?, attachments = ?, metadata = ?,
                                         edited_ts = ?, edited_by = ?
                     WHERE channel_id = ? AND ts = ?"),
                )
                .bind(&m.user_id)
                .bind(&m.subtype)
                .bind(&m.text)
                .bind(json_text(&m.blocks))
                .bind(json_text(&m.attachments))
                .bind(json_text(&m.metadata))
                .bind(&m.edited_ts)
                .bind(&m.edited_by)
                .bind(&m.channel_id)
                .bind(&m.ts)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn delete_message(&self, channel_id: &str, ts: &str) -> Result<()> {
                sqlx::query(prep("DELETE FROM messages WHERE channel_id = ? AND ts = ?"))
                    .bind(channel_id)
                    .bind(ts)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn history(&self, channel_id: &str, range: &TsRange, limit: i64) -> Result<Vec<Message>> {
                let mut args = <$args>::default();
                args.add(channel_id).expect("bind");
                let mut sql = format!("SELECT x.* FROM messages x WHERE x.channel_id = ? AND {CHANNEL_LEVEL}");
                push_range(&mut sql, &mut args, range);
                sql.push_str(" ORDER BY x.ts DESC LIMIT ?");
                args.add(limit).expect("bind");
                Ok(sqlx::query_with(prep(&sql), args)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(message_row)
                    .collect())
            }

            async fn replies(&self, channel_id: &str, thread_ts: &str, range: &TsRange, limit: i64) -> Result<Vec<Message>> {
                let mut args = <$args>::default();
                args.add(channel_id).expect("bind");
                args.add(thread_ts).expect("bind");
                args.add(thread_ts).expect("bind");
                // The parent matches by ts too, before it has any replies.
                let mut sql = "SELECT x.* FROM messages x WHERE x.channel_id = ? AND (x.thread_ts = ? OR x.ts = ?)".to_owned();
                push_range(&mut sql, &mut args, range);
                sql.push_str(" ORDER BY x.ts ASC LIMIT ?");
                args.add(limit).expect("bind");
                Ok(sqlx::query_with(prep(&sql), args)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(message_row)
                    .collect())
            }

            async fn thread_summaries(&self, channel_id: &str, parents: &[String]) -> Result<HashMap<String, ThreadSummary>> {
                let mut out: HashMap<String, ThreadSummary> = HashMap::new();
                if parents.is_empty() {
                    return Ok(out);
                }
                let sql = format!(
                    "SELECT thread_ts, ts, user_id FROM messages
                     WHERE channel_id = ? AND thread_ts IN ({}) AND ts != thread_ts ORDER BY ts",
                    placeholders(parents.len())
                );
                let mut q = sqlx::query(prep(&sql)).bind(channel_id);
                for p in parents {
                    q = q.bind(p);
                }
                for r in q.fetch_all(&self.pool).await? {
                    let s = out.entry(r.get("thread_ts")).or_default();
                    s.reply_count += 1;
                    s.latest_reply = Some(r.get("ts"));
                    if let Some(u) = r.get::<Option<String>, _>("user_id")
                        && !s.reply_users.contains(&u)
                    {
                        s.reply_users.push(u);
                    }
                }
                Ok(out)
            }

            async fn search_messages(&self, q: &MessageSearch<'_>) -> Result<Vec<Message>> {
                let mut args = <$args>::default();
                let mut sql = "SELECT x.* FROM messages x JOIN conversations c ON c.id = x.channel_id
                     WHERE c.team_id = ?
                       AND ((c.is_private = FALSE AND c.is_im = FALSE AND c.is_mpim = FALSE)
                            OR EXISTS (SELECT 1 FROM conversation_members m WHERE m.conversation_id = c.id AND m.user_id = ?))
                       AND (x.subtype IS NULL OR x.subtype IN ('bot_message', 'thread_broadcast', 'me_message', 'file_share'))"
                    .to_owned();
                args.add(q.team_id).expect("bind");
                args.add(q.viewer).expect("bind");
                for term in &q.terms {
                    // Literal match: escape LIKE's wildcards.
                    let escaped = term
                        .to_lowercase()
                        .replace('\\', "\\\\")
                        .replace('%', "\\%")
                        .replace('_', "\\_");
                    sql.push_str(" AND lower(x.text) LIKE ? ESCAPE '\\'");
                    args.add(format!("%{escaped}%")).expect("bind");
                }
                if let Some(c) = &q.channel_id {
                    sql.push_str(" AND x.channel_id = ?");
                    args.add(c.clone()).expect("bind");
                }
                if let Some(u) = &q.user_id {
                    sql.push_str(" AND x.user_id = ?");
                    args.add(u.clone()).expect("bind");
                }
                if let Some(a) = &q.after {
                    sql.push_str(" AND x.ts >= ?");
                    args.add(a.clone()).expect("bind");
                }
                if let Some(b) = &q.before {
                    sql.push_str(" AND x.ts < ?");
                    args.add(b.clone()).expect("bind");
                }
                sql.push_str(" ORDER BY x.ts DESC LIMIT ?");
                args.add(q.limit).expect("bind");
                Ok(sqlx::query_with(prep(&sql), args)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(message_row)
                    .collect())
            }

            async fn max_ts(&self) -> Result<Option<String>> {
                Ok(sqlx::query(prep("SELECT MAX(ts) AS ts FROM messages"))
                    .fetch_one(&self.pool)
                    .await?
                    .get("ts"))
            }

            async fn create_file(&self, f: &File) -> Result<()> {
                sqlx::query(prep(
                    "INSERT INTO files (id, team_id, user_id, bot_id, name, title, mimetype, filetype, snippet_type,
                                        alt_txt, size, created, upload_token, uploaded, completed)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                ))
                .bind(&f.id)
                .bind(&f.team_id)
                .bind(&f.user_id)
                .bind(&f.bot_id)
                .bind(&f.name)
                .bind(&f.title)
                .bind(&f.mimetype)
                .bind(&f.filetype)
                .bind(&f.snippet_type)
                .bind(&f.alt_txt)
                .bind(f.size)
                .bind(f.created)
                .bind(&f.upload_token)
                .bind(f.uploaded)
                .bind(f.completed)
                .execute(&self.pool)
                .await
                .map_err(conflict("file"))?;
                Ok(())
            }

            async fn get_file(&self, id: &str) -> Result<Option<File>> {
                Ok(sqlx::query(prep("SELECT * FROM files WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(file_row))
            }

            async fn get_files(&self, ids: &[String]) -> Result<Vec<File>> {
                if ids.is_empty() {
                    return Ok(Vec::new());
                }
                let sql = format!("SELECT * FROM files WHERE id IN ({})", placeholders(ids.len()));
                let mut q = sqlx::query(prep(&sql));
                for id in ids {
                    q = q.bind(id);
                }
                Ok(q.fetch_all(&self.pool).await?.into_iter().map(file_row).collect())
            }

            async fn file_by_upload_token(&self, token: &str) -> Result<Option<File>> {
                Ok(sqlx::query(prep("SELECT * FROM files WHERE upload_token = ?"))
                    .bind(token)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(file_row))
            }

            async fn update_file(&self, f: &File) -> Result<()> {
                sqlx::query(prep(
                    "UPDATE files SET title = ?, size = ?, upload_token = ?, uploaded = ?, completed = ? WHERE id = ?",
                ))
                .bind(&f.title)
                .bind(f.size)
                .bind(&f.upload_token)
                .bind(f.uploaded)
                .bind(f.completed)
                .bind(&f.id)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn set_file_data(&self, id: &str, data: &[u8]) -> Result<()> {
                sqlx::query(prep(
                    "INSERT INTO file_contents (file_id, data) VALUES (?, ?)
                     ON CONFLICT (file_id) DO UPDATE SET data = excluded.data",
                ))
                .bind(id)
                .bind(data)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn file_data(&self, id: &str) -> Result<Option<Vec<u8>>> {
                Ok(sqlx::query_scalar(prep("SELECT data FROM file_contents WHERE file_id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?)
            }

            async fn delete_file(&self, id: &str) -> Result<()> {
                sqlx::query(prep("DELETE FROM files WHERE id = ?"))
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn list_files(&self, team_id: &str) -> Result<Vec<File>> {
                Ok(sqlx::query(prep(
                    "SELECT * FROM files WHERE team_id = ? AND completed = TRUE ORDER BY created DESC, id DESC",
                ))
                .bind(team_id)
                .fetch_all(&self.pool)
                .await?
                .into_iter()
                .map(file_row)
                .collect())
            }

            async fn add_file_share(&self, share: &FileShare) -> Result<()> {
                sqlx::query(prep(
                    "INSERT INTO file_shares (file_id, channel_id, ts) VALUES (?, ?, ?) ON CONFLICT DO NOTHING",
                ))
                .bind(&share.file_id)
                .bind(&share.channel_id)
                .bind(&share.ts)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn file_shares(&self, file_ids: &[String]) -> Result<Vec<FileShare>> {
                if file_ids.is_empty() {
                    return Ok(Vec::new());
                }
                let sql = format!(
                    "SELECT * FROM file_shares WHERE file_id IN ({}) ORDER BY ts",
                    placeholders(file_ids.len())
                );
                let mut q = sqlx::query(prep(&sql));
                for id in file_ids {
                    q = q.bind(id);
                }
                Ok(q
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(|r| FileShare {
                        file_id: r.get("file_id"),
                        channel_id: r.get("channel_id"),
                        ts: r.get("ts"),
                    })
                    .collect())
            }

            async fn add_pin(&self, p: &Pin) -> Result<bool> {
                let r = sqlx::query(prep(
                    "INSERT INTO pins (channel_id, ts, pinned_by, created) VALUES (?, ?, ?, ?) ON CONFLICT DO NOTHING",
                ))
                .bind(&p.channel_id)
                .bind(&p.ts)
                .bind(&p.pinned_by)
                .bind(p.created)
                .execute(&self.pool)
                .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn remove_pin(&self, channel_id: &str, ts: &str) -> Result<bool> {
                let r = sqlx::query(prep("DELETE FROM pins WHERE channel_id = ? AND ts = ?"))
                    .bind(channel_id)
                    .bind(ts)
                    .execute(&self.pool)
                    .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn pins(&self, channel_id: &str) -> Result<Vec<Pin>> {
                Ok(sqlx::query(prep("SELECT * FROM pins WHERE channel_id = ? ORDER BY created, ts"))
                    .bind(channel_id)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(pin_row)
                    .collect())
            }

            async fn pinned(&self, channel_id: &str, ts: &[String]) -> Result<Vec<String>> {
                if ts.is_empty() {
                    return Ok(Vec::new());
                }
                let sql = format!(
                    "SELECT ts FROM pins WHERE channel_id = ? AND ts IN ({})",
                    placeholders(ts.len())
                );
                let mut q = sqlx::query_scalar(prep(&sql)).bind(channel_id);
                for t in ts {
                    q = q.bind(t);
                }
                Ok(q.fetch_all(&self.pool).await?)
            }

            async fn add_bookmark(&self, b: &Bookmark) -> Result<()> {
                sqlx::query(prep(
                    "INSERT INTO bookmarks (id, channel_id, title, link, emoji, app_id, created, updated, updated_by)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                ))
                .bind(&b.id)
                .bind(&b.channel_id)
                .bind(&b.title)
                .bind(&b.link)
                .bind(&b.emoji)
                .bind(&b.app_id)
                .bind(b.created)
                .bind(b.updated)
                .bind(&b.updated_by)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn get_bookmark(&self, id: &str) -> Result<Option<Bookmark>> {
                Ok(sqlx::query(prep("SELECT * FROM bookmarks WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(bookmark_row))
            }

            async fn update_bookmark(&self, b: &Bookmark) -> Result<()> {
                sqlx::query(prep(
                    "UPDATE bookmarks SET title = ?, link = ?, emoji = ?, updated = ?, updated_by = ? WHERE id = ?",
                ))
                .bind(&b.title)
                .bind(&b.link)
                .bind(&b.emoji)
                .bind(b.updated)
                .bind(&b.updated_by)
                .bind(&b.id)
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn remove_bookmark(&self, id: &str) -> Result<()> {
                sqlx::query(prep("DELETE FROM bookmarks WHERE id = ?"))
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(())
            }

            async fn bookmarks(&self, channel_id: &str) -> Result<Vec<Bookmark>> {
                Ok(sqlx::query(prep("SELECT * FROM bookmarks WHERE channel_id = ? ORDER BY created, id"))
                    .bind(channel_id)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(bookmark_row)
                    .collect())
            }

            async fn add_scheduled(&self, m: &ScheduledMessage) -> Result<()> {
                sqlx::query(prep(
                    "INSERT INTO scheduled_messages (id, team_id, channel_id, user_id, token, app_id, post_at, created, args)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                ))
                .bind(&m.id)
                .bind(&m.team_id)
                .bind(&m.channel_id)
                .bind(&m.user_id)
                .bind(&m.token)
                .bind(&m.app_id)
                .bind(m.post_at)
                .bind(m.created)
                .bind(m.args.to_string())
                .execute(&self.pool)
                .await?;
                Ok(())
            }

            async fn get_scheduled(&self, id: &str) -> Result<Option<ScheduledMessage>> {
                Ok(sqlx::query(prep("SELECT * FROM scheduled_messages WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(scheduled_row))
            }

            async fn list_scheduled(&self, team_id: &str) -> Result<Vec<ScheduledMessage>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM scheduled_messages WHERE team_id = ? ORDER BY post_at, id"))
                        .bind(team_id)
                        .fetch_all(&self.pool)
                        .await?
                        .into_iter()
                        .map(scheduled_row)
                        .collect(),
                )
            }

            async fn due_scheduled(&self, now: i64) -> Result<Vec<ScheduledMessage>> {
                Ok(
                    sqlx::query(prep("SELECT * FROM scheduled_messages WHERE post_at <= ? ORDER BY post_at, id"))
                        .bind(now)
                        .fetch_all(&self.pool)
                        .await?
                        .into_iter()
                        .map(scheduled_row)
                        .collect(),
                )
            }

            async fn delete_scheduled(&self, id: &str) -> Result<bool> {
                let r = sqlx::query(prep("DELETE FROM scheduled_messages WHERE id = ?"))
                    .bind(id)
                    .execute(&self.pool)
                    .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn create_usergroup(&self, g: &Usergroup) -> Result<()> {
                sqlx::query(prep(
                    "INSERT INTO usergroups (id, team_id, name, handle, description, channels, created_by, updated_by,
                                             created, updated, disabled_at, deleted_by)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                ))
                .bind(&g.id)
                .bind(&g.team_id)
                .bind(&g.name)
                .bind(&g.handle)
                .bind(&g.description)
                .bind(g.channels.join(","))
                .bind(&g.created_by)
                .bind(&g.updated_by)
                .bind(g.created)
                .bind(g.updated)
                .bind(g.disabled_at)
                .bind(&g.deleted_by)
                .execute(&self.pool)
                .await
                .map_err(conflict("usergroup"))?;
                Ok(())
            }

            async fn get_usergroup(&self, id: &str) -> Result<Option<Usergroup>> {
                Ok(sqlx::query(prep("SELECT * FROM usergroups WHERE id = ?"))
                    .bind(id)
                    .fetch_optional(&self.pool)
                    .await?
                    .map(usergroup_row))
            }

            async fn list_usergroups(&self, team_id: &str) -> Result<Vec<Usergroup>> {
                Ok(sqlx::query(prep("SELECT * FROM usergroups WHERE team_id = ? ORDER BY name, id"))
                    .bind(team_id)
                    .fetch_all(&self.pool)
                    .await?
                    .into_iter()
                    .map(usergroup_row)
                    .collect())
            }

            async fn update_usergroup(&self, g: &Usergroup) -> Result<()> {
                sqlx::query(prep(
                    "UPDATE usergroups SET name = ?, handle = ?, description = ?, channels = ?, updated_by = ?,
                                           updated = ?, disabled_at = ?, deleted_by = ?
                     WHERE id = ?",
                ))
                .bind(&g.name)
                .bind(&g.handle)
                .bind(&g.description)
                .bind(g.channels.join(","))
                .bind(&g.updated_by)
                .bind(g.updated)
                .bind(g.disabled_at)
                .bind(&g.deleted_by)
                .bind(&g.id)
                .execute(&self.pool)
                .await
                .map_err(conflict("usergroup"))?;
                Ok(())
            }

            async fn set_usergroup_members(&self, id: &str, users: &[String]) -> Result<()> {
                let mut tx = self.pool.begin().await?;
                sqlx::query(prep("DELETE FROM usergroup_members WHERE usergroup_id = ?"))
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;
                for u in users {
                    sqlx::query(prep(
                        "INSERT INTO usergroup_members (usergroup_id, user_id) VALUES (?, ?) ON CONFLICT DO NOTHING",
                    ))
                    .bind(id)
                    .bind(u)
                    .execute(&mut *tx)
                    .await?;
                }
                tx.commit().await?;
                Ok(())
            }

            async fn usergroup_members(&self, id: &str) -> Result<Vec<String>> {
                Ok(sqlx::query_scalar(prep(
                    "SELECT user_id FROM usergroup_members WHERE usergroup_id = ? ORDER BY user_id",
                ))
                .bind(id)
                .fetch_all(&self.pool)
                .await?)
            }

            async fn add_reaction(&self, channel_id: &str, ts: &str, name: &str, user_id: &str) -> Result<bool> {
                let r = sqlx::query(
                    prep("INSERT INTO reactions (channel_id, ts, name, user_id, created) VALUES (?, ?, ?, ?, ?)
                     ON CONFLICT DO NOTHING"),
                )
                .bind(channel_id)
                .bind(ts)
                .bind(name)
                .bind(user_id)
                .bind(now())
                .execute(&self.pool)
                .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn remove_reaction(&self, channel_id: &str, ts: &str, name: &str, user_id: &str) -> Result<bool> {
                let r = sqlx::query(prep("DELETE FROM reactions WHERE channel_id = ? AND ts = ? AND name = ? AND user_id = ?"))
                    .bind(channel_id)
                    .bind(ts)
                    .bind(name)
                    .bind(user_id)
                    .execute(&self.pool)
                    .await?;
                Ok(r.rows_affected() > 0)
            }

            async fn reactions(&self, channel_id: &str, ts_list: &[String]) -> Result<HashMap<String, Vec<Reaction>>> {
                let mut out: HashMap<String, Vec<Reaction>> = HashMap::new();
                if ts_list.is_empty() {
                    return Ok(out);
                }
                let sql = format!(
                    "SELECT ts, name, user_id FROM reactions WHERE channel_id = ? AND ts IN ({})
                     ORDER BY created, rowid",
                    placeholders(ts_list.len())
                );
                let mut q = sqlx::query(prep(&sql)).bind(channel_id);
                for t in ts_list {
                    q = q.bind(t);
                }
                for r in q.fetch_all(&self.pool).await? {
                    let list = out.entry(r.get("ts")).or_default();
                    let name: String = r.get("name");
                    let user: String = r.get("user_id");
                    match list.iter_mut().find(|x| x.name == name) {
                        Some(x) => x.users.push(user),
                        None => list.push(Reaction {
                            name,
                            users: vec![user],
                        }),
                    }
                }
                Ok(out)
            }
        }
    };
}

pub(crate) use sql_store;
