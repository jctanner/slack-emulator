//! SQLite backend (the default): one file, nothing to run alongside.

use std::str::FromStr;

use sqlx::sqlite::{SqliteArguments, SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{AssertSqlSafe, SqlitePool};

use crate::Result as StoreResult;

/// SQLite takes the shared statements as they are.
fn prep(sql: &str) -> AssertSqlSafe<String> {
    AssertSqlSafe(sql.to_owned())
}

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Opens (creating if needed) the database at `url` and runs migrations.
    pub async fn connect(url: &str) -> StoreResult<Self> {
        let in_memory = url.contains(":memory:");
        let opts = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        // Each in-memory connection is a separate database, so pin to one.
        let pool = SqlitePoolOptions::new()
            .max_connections(if in_memory { 1 } else { 8 })
            .connect_with(opts)
            .await?;
        sqlx::migrate!("./migrations/sqlite").run(&pool).await?;
        Ok(Self { pool })
    }
}

crate::backend::sql_store!(SqliteStore, SqliteRow, SqliteArguments);

#[cfg(test)]
mod tests {
    use super::SqliteStore;

    crate::conformance::suite!(Some(SqliteStore::connect("sqlite::memory:").await.unwrap()));
}
