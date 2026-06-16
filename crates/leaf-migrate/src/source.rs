//! Loading the walpurgisbot-v2 archive that is being imported.
//!
//! The source is either v2's `SQLite` database or its JSON export. Both are
//! normalized to [`TransferPost`] (leaf's v2-compatible transfer shape), so
//! the rest of the importer is source-agnostic.

use std::collections::HashMap;
use std::path::Path;

use anyhow::Context as _;
use leaf_core::transfer::{self, TransferPost};
use sqlx::Row as _;
use sqlx::sqlite::SqliteConnectOptions;

/// Magic bytes every `SQLite` database file begins with.
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// Loads the source archive, auto-detecting `SQLite` vs JSON by file content
/// (not extension), and returns its posts ordered by day.
pub async fn load(path: &Path) -> anyhow::Result<Vec<TransferPost>> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("reading source {}", path.display()))?;

    let mut posts = if bytes.starts_with(SQLITE_MAGIC) {
        load_sqlite(path).await?
    } else {
        load_json(&bytes)?
    };
    posts.sort_by_key(|p| p.day);
    Ok(posts)
}

/// Parses a v2 JSON export (an array of posts).
fn load_json(bytes: &[u8]) -> anyhow::Result<Vec<TransferPost>> {
    transfer::parse(bytes).map_err(|e| anyhow::anyhow!("invalid v2 JSON export: {e}"))
}

/// Reads a walpurgisbot-v2 `SQLite` database.
///
/// These are **runtime** queries (`sqlx::query`), deliberately not the
/// compile-checked `query!` macro: the macro validates against *leaf's*
/// schema, but this reads v2's foreign schema (`posts(day, message_id,
/// channel_id, user_id, timestamp, …)` + `media_attachments(post_day, url)`),
/// a different shape leaf's schema cache knows nothing about. Opened
/// read-only; the v2 archive is at most a few thousand rows, so loading it
/// whole is fine.
async fn load_sqlite(path: &Path) -> anyhow::Result<Vec<TransferPost>> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .read_only(true)
        .create_if_missing(false);
    let pool = sqlx::SqlitePool::connect_with(opts)
        .await
        .with_context(|| format!("opening v2 database {}", path.display()))?;

    // Media first, grouped by day, so each post can pick up its URLs.
    let mut media_by_day: HashMap<i64, Vec<String>> = HashMap::new();
    let media_rows =
        sqlx::query("SELECT post_day, url FROM media_attachments ORDER BY post_day, id")
            .fetch_all(&pool)
            .await
            .context("reading v2 media_attachments")?;
    for row in media_rows {
        let day: i64 = row
            .try_get("post_day")
            .context("v2 media_attachments.post_day")?;
        let url: String = row.try_get("url").context("v2 media_attachments.url")?;
        media_by_day.entry(day).or_default().push(url);
    }

    let post_rows = sqlx::query(
        "SELECT day, message_id, channel_id, user_id, timestamp FROM posts ORDER BY day",
    )
    .fetch_all(&pool)
    .await
    .context("reading v2 posts")?;

    let mut posts = Vec::with_capacity(post_rows.len());
    for row in post_rows {
        let day: i64 = row.try_get("day").context("v2 posts.day")?;
        posts.push(TransferPost {
            media: media_by_day.remove(&day).unwrap_or_default(),
            day,
            message_id: row.try_get("message_id").context("v2 posts.message_id")?,
            channel_id: row.try_get("channel_id").context("v2 posts.channel_id")?,
            user_id: row.try_get("user_id").context("v2 posts.user_id")?,
            timestamp: row.try_get("timestamp").context("v2 posts.timestamp")?,
        });
    }

    pool.close().await;
    Ok(posts)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    /// Creates a minimal v2-schema database (just the two tables migration
    /// reads) and seeds it.
    async fn seed_v2_db(path: &Path) {
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true);
        let pool = sqlx::SqlitePool::connect_with(opts).await.unwrap();
        sqlx::query(
            "CREATE TABLE posts (
                 day INTEGER PRIMARY KEY NOT NULL,
                 message_id TEXT NOT NULL,
                 channel_id TEXT NOT NULL,
                 user_id TEXT NOT NULL,
                 timestamp INTEGER NOT NULL,
                 confirmed INTEGER NOT NULL DEFAULT 1)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE media_attachments (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 post_day INTEGER NOT NULL,
                 url TEXT NOT NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        // Day 2 with two media (inserted out of order to prove sorting), day 1
        // with one, day 3 with none.
        for (day, msg, chan, user, ts) in [
            (2, "m2", "c1", "u", 1_700_000_200_i64),
            (1, "m1", "c1", "u", 1_700_000_100_i64),
            (3, "m3", "c2", "u", 1_700_000_300_i64),
        ] {
            sqlx::query(
                "INSERT INTO posts (day, message_id, channel_id, user_id, timestamp) VALUES (?,?,?,?,?)",
            )
            .bind(day)
            .bind(msg)
            .bind(chan)
            .bind(user)
            .bind(ts)
            .execute(&pool)
            .await
            .unwrap();
        }
        for (day, url) in [
            (2, "https://cdn.discordapp.com/attachments/c1/201/a.png"),
            (2, "https://cdn.discordapp.com/attachments/c1/202/b.png"),
            (1, "https://cdn.discordapp.com/attachments/c1/101/x.png"),
        ] {
            sqlx::query("INSERT INTO media_attachments (post_day, url) VALUES (?, ?)")
                .bind(day)
                .bind(url)
                .execute(&pool)
                .await
                .unwrap();
        }
        pool.close().await;
    }

    #[tokio::test]
    async fn reads_v2_sqlite_grouping_media_and_sorting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("walpurgis.db");
        seed_v2_db(&path).await;

        let posts = load(&path).await.unwrap();
        assert_eq!(posts.iter().map(|p| p.day).collect::<Vec<_>>(), [1, 2, 3]);

        let day1 = posts.iter().find(|p| p.day == 1).unwrap();
        assert_eq!(day1.message_id, "m1");
        assert_eq!(day1.timestamp, 1_700_000_100);
        assert_eq!(day1.media.len(), 1);

        let day2 = posts.iter().find(|p| p.day == 2).unwrap();
        assert_eq!(day2.media.len(), 2);

        let day3 = posts.iter().find(|p| p.day == 3).unwrap();
        assert!(day3.media.is_empty());
    }

    #[tokio::test]
    async fn reads_v2_json_export() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("export.json");
        let raw = br#"[
          {"day":2,"message_id":"m2","channel_id":"c","user_id":"u","timestamp":2,"media":["u2"]},
          {"day":1,"message_id":"m1","channel_id":"c","user_id":"u","timestamp":1,"media":[]}
        ]"#;
        tokio::fs::write(&path, raw).await.unwrap();

        let posts = load(&path).await.unwrap();
        // Sorted by day regardless of file order.
        assert_eq!(posts.iter().map(|p| p.day).collect::<Vec<_>>(), [1, 2]);
        let day2 = posts.iter().find(|p| p.day == 2).unwrap();
        assert_eq!(day2.media, vec!["u2".to_owned()]);
    }

    #[tokio::test]
    async fn rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("junk.json");
        tokio::fs::write(&path, b"not json, not sqlite")
            .await
            .unwrap();
        assert!(load(&path).await.is_err());
    }
}
