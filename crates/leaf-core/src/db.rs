//! Database access: pool construction, migrations, and typed repositories.
//!
//! All SQL lives here (and in submodules); callers see domain types only.
//! Every query is compile-checked (`sqlx::query!`); every multi-statement
//! write is a transaction.

use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

pub mod guilds;
pub mod posts;
pub mod series;

pub use guilds::GuildSettingsRepo;
pub use posts::PostRepo;
pub use series::SeriesRepo;

/// Errors produced by the repository layer.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Underlying driver error.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// Migration failure at startup.
    #[error(transparent)]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A stored string column held a value outside its enum domain.
    #[error(transparent)]
    Corrupt(#[from] crate::domain::InvalidEnumValue),
    /// A stored JSON column failed to parse.
    #[error("corrupt JSON column: {0}")]
    CorruptJson(#[from] serde_json::Error),
    /// A series name already exists in this guild.
    #[error("a series with this name already exists in this guild")]
    SeriesNameTaken,
    /// The day is already archived for this series.
    #[error("day {0} is already archived for this series")]
    DuplicateDay(i64),
}

/// Result alias for repository operations.
pub type DbResult<T> = Result<T, DbError>;

/// Opens (creating if missing) the `SQLite` database at `path` and runs all
/// pending migrations. WAL mode, foreign keys ON, 5s busy timeout.
pub async fn connect(path: &Path) -> DbResult<SqlitePool> {
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(options)
        .await?;

    sqlx::migrate!("../../migrations").run(&pool).await?;
    Ok(pool)
}

/// True when `err` is a UNIQUE-constraint violation.
fn is_unique_violation(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Database(db) if db.is_unique_violation())
}

/// Serializes a list of snowflakes into the JSON TEXT column form.
fn to_json_ids(ids: &[String]) -> DbResult<String> {
    Ok(serde_json::to_string(ids)?)
}

/// Parses the JSON TEXT column form back into snowflakes.
fn from_json_ids(raw: &str) -> DbResult<Vec<String>> {
    Ok(serde_json::from_str(raw)?)
}

#[cfg(test)]
pub(crate) mod testutil {
    #![allow(clippy::unwrap_used, reason = "test fixtures may panic")]

    use super::*;

    /// A migrated, file-backed (tempdir) pool for repository tests.
    /// File-backed rather than `:memory:` so WAL and the real pool size
    /// behave exactly as production does.
    pub async fn test_pool() -> (tempfile::TempDir, SqlitePool) {
        let dir = tempfile::tempdir().unwrap();
        let pool = connect(&dir.path().join("test.db")).await.unwrap();
        (dir, pool)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    #[tokio::test]
    async fn migrations_apply_cleanly_twice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        let pool = connect(&path).await.unwrap();
        pool.close().await;
        // Reconnecting re-runs the migrator against an up-to-date DB.
        let pool = connect(&path).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(n >= 1);
    }

    #[test]
    fn json_id_round_trip() {
        let ids = vec!["123".to_owned(), "456".to_owned()];
        let raw = to_json_ids(&ids).unwrap();
        assert_eq!(from_json_ids(&raw).unwrap(), ids);
        assert_eq!(from_json_ids("[]").unwrap(), Vec::<String>::new());
    }
}
