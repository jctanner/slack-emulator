//! Postgres backend, for shared or long-lived deployments.

use sqlx::postgres::{PgArguments, PgPoolOptions, PgRow};
use sqlx::{AssertSqlSafe, PgPool};

use crate::Result as StoreResult;

/// Numbers the shared statements' `?` placeholders as Postgres wants (`$1`, `$2`, …).
fn prep(sql: &str) -> AssertSqlSafe<String> {
    let mut out = String::with_capacity(sql.len() + 16);
    let mut n = 0;
    let mut in_string = false;
    for ch in sql.chars() {
        match ch {
            '\'' => {
                in_string = !in_string;
                out.push(ch);
            }
            '?' if !in_string => {
                n += 1;
                out.push('$');
                out.push_str(&n.to_string());
            }
            _ => out.push(ch),
        }
    }
    AssertSqlSafe(out)
}

pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    /// Connects to `url` (`postgres://…`) and runs migrations.
    pub async fn connect(url: &str) -> StoreResult<Self> {
        let pool = PgPoolOptions::new().max_connections(16).connect(url).await?;
        sqlx::migrate!("./migrations/postgres").run(&pool).await?;
        Ok(Self { pool })
    }
}

crate::backend::sql_store!(PgStore, PgRow, PgArguments);

/// For tests: creates an empty database on the server `url` points at and
/// returns a URL for it, so each test gets its own. Test databases are named
/// `emu_test_*` and aren't dropped; use a throwaway server.
pub async fn fresh_test_database(url: &str) -> StoreResult<String> {
    let name = format!("emu_test_{}", slack_emu_core::ids::new_internal_id().to_lowercase());
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
        .execute(&pool)
        .await?;
    pool.close().await;
    let (base, query) = match url.split_once('?') {
        Some((b, q)) => (b, format!("?{q}")),
        None => (url, String::new()),
    };
    let server = match base.rsplit_once('/') {
        // `postgres://host/db` → replace the database; `postgres://host` has none yet.
        Some((server, _)) if server.contains("://") && !server.ends_with('/') => server.to_owned(),
        _ => base.to_owned(),
    };
    Ok(format!("{server}/{name}{query}"))
}

#[cfg(test)]
mod tests {
    use super::{PgStore, fresh_test_database, prep};

    /// A store on a fresh database, if `EMU_TEST_POSTGRES` is set.
    async fn store() -> Option<PgStore> {
        let url = std::env::var("EMU_TEST_POSTGRES").ok().filter(|s| !s.is_empty())?;
        let db = fresh_test_database(&url).await.unwrap();
        Some(PgStore::connect(&db).await.unwrap())
    }

    crate::conformance::suite!(store().await);

    #[test]
    fn numbers_placeholders_outside_strings() {
        let AssertSqlSafe(sql) = prep("SELECT '?' || x FROM t WHERE a = ? AND b IN (?, ?)");
        assert_eq!(sql, "SELECT '?' || x FROM t WHERE a = $1 AND b IN ($2, $3)");
    }

    use sqlx::AssertSqlSafe;
}
