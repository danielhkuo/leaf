//! Repository for launch intents: the short-lived "open the gallery here"
//! hand-off between a chat button and the Activity.
//!
//! Discord's `LAUNCH_ACTIVITY` response carries no payload, so the bot
//! records where the user was headed just before launching and the Activity
//! collects it once it has a session. The bot and the server share one
//! database, which makes this the only channel that needs no undocumented
//! Discord behaviour.

use sqlx::SqlitePool;

use super::DbResult;
use crate::domain::LaunchIntent;

/// How long a launch intent stays collectable, in seconds. Long enough for
/// a cold Activity start on a phone, short enough that a stale intent never
/// hijacks a later, unrelated launch.
pub const LAUNCH_INTENT_TTL_SECS: i64 = 120;

/// Put/take over the `launch_intents` table.
#[derive(Debug, Clone)]
pub struct LaunchIntentRepo {
    pool: SqlitePool,
}

impl LaunchIntentRepo {
    /// Creates a repo over `pool`.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Records where `user_id` should land in `guild_id`'s gallery,
    /// replacing any earlier intent for the same pair. It expires
    /// [`LAUNCH_INTENT_TTL_SECS`] after `now_unix`.
    pub async fn put(
        &self,
        user_id: &str,
        guild_id: &str,
        series_id: i64,
        day: Option<i64>,
        now_unix: i64,
    ) -> DbResult<()> {
        let expires_at = now_unix.saturating_add(LAUNCH_INTENT_TTL_SECS);
        let mut tx = self.pool.begin().await?;

        // Intents nobody collected (launch cancelled, Activity never
        // opened) would otherwise sit here forever.
        sqlx::query!("DELETE FROM launch_intents WHERE expires_at <= ?", now_unix)
            .execute(&mut *tx)
            .await?;

        sqlx::query!(
            r#"INSERT INTO launch_intents (user_id, guild_id, series_id, day, expires_at)
               VALUES (?, ?, ?, ?, ?)
               ON CONFLICT (user_id, guild_id) DO UPDATE SET
                    series_id = excluded.series_id,
                    day = excluded.day,
                    expires_at = excluded.expires_at"#,
            user_id,
            guild_id,
            series_id,
            day,
            expires_at,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(())
    }

    /// Collects and removes the pending intent for `(user_id, guild_id)`.
    /// `None` when there is none or it has expired; either way the row is
    /// gone afterwards, so an intent is acted on at most once.
    pub async fn take(
        &self,
        user_id: &str,
        guild_id: &str,
        now_unix: i64,
    ) -> DbResult<Option<LaunchIntent>> {
        let row = sqlx::query!(
            r#"DELETE FROM launch_intents WHERE user_id = ? AND guild_id = ?
               RETURNING series_id AS "series_id!: i64", day AS "day?: i64",
                         expires_at AS "expires_at!: i64""#,
            user_id,
            guild_id,
        )
        .fetch_optional(&self.pool)
        .await?;

        Ok(row
            .filter(|r| r.expires_at > now_unix)
            .map(|r| LaunchIntent {
                series_id: r.series_id,
                day: r.day,
            }))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;
    use crate::db::testutil::test_pool;

    async fn repo() -> (tempfile::TempDir, LaunchIntentRepo, SqlitePool) {
        let (dir, pool) = test_pool().await;
        (dir, LaunchIntentRepo::new(pool.clone()), pool)
    }

    async fn rows(pool: &SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM launch_intents")
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn take_returns_the_intent_once() {
        let (_d, repo, _pool) = repo().await;
        repo.put("u", "g", 7, Some(42), 1000).await.unwrap();

        let got = repo.take("u", "g", 1001).await.unwrap();
        assert_eq!(
            got,
            Some(LaunchIntent {
                series_id: 7,
                day: Some(42)
            })
        );
        // Consumed on read.
        assert_eq!(repo.take("u", "g", 1001).await.unwrap(), None);
    }

    #[tokio::test]
    async fn intents_are_scoped_to_user_and_guild() {
        let (_d, repo, _pool) = repo().await;
        repo.put("u", "g", 7, None, 1000).await.unwrap();

        assert_eq!(repo.take("other", "g", 1001).await.unwrap(), None);
        assert_eq!(repo.take("u", "other", 1001).await.unwrap(), None);
        assert_eq!(
            repo.take("u", "g", 1001).await.unwrap(),
            Some(LaunchIntent {
                series_id: 7,
                day: None
            })
        );
    }

    #[tokio::test]
    async fn a_newer_put_replaces_the_older_one() {
        let (_d, repo, pool) = repo().await;
        repo.put("u", "g", 7, Some(1), 1000).await.unwrap();
        repo.put("u", "g", 8, None, 1010).await.unwrap();
        assert_eq!(rows(&pool).await, 1);

        assert_eq!(
            repo.take("u", "g", 1011).await.unwrap(),
            Some(LaunchIntent {
                series_id: 8,
                day: None
            })
        );
    }

    #[tokio::test]
    async fn expired_intents_are_not_returned_and_are_removed() {
        let (_d, repo, pool) = repo().await;
        repo.put("u", "g", 7, Some(3), 1000).await.unwrap();

        // Still collectable one second before expiry, gone at expiry.
        let last = 1000 + LAUNCH_INTENT_TTL_SECS - 1;
        repo.put("u2", "g", 9, None, 1000).await.unwrap();
        assert!(repo.take("u2", "g", last).await.unwrap().is_some());
        assert_eq!(repo.take("u", "g", last + 1).await.unwrap(), None);
        assert_eq!(rows(&pool).await, 0);
    }

    #[tokio::test]
    async fn put_prunes_uncollected_expired_rows() {
        let (_d, repo, pool) = repo().await;
        repo.put("stale", "g", 1, None, 1000).await.unwrap();
        repo.put("fresh", "g", 2, None, 1000 + LAUNCH_INTENT_TTL_SECS)
            .await
            .unwrap();
        assert_eq!(rows(&pool).await, 1);
        assert_eq!(
            repo.take("stale", "g", 1000 + LAUNCH_INTENT_TTL_SECS)
                .await
                .unwrap(),
            None
        );
    }
}
