//! Repository for archived posts and their media attachments.

use sqlx::SqlitePool;

use super::{DbError, DbResult, is_unique_violation};
use crate::domain::{MediaAttachment, NewMediaAttachment, Post};

/// CRUD over `posts` + `media_attachments`.
#[derive(Debug, Clone)]
pub struct PostRepo {
    pool: SqlitePool,
}

impl PostRepo {
    /// Creates a repo over `pool`.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Archives a post and its media in one transaction.
    ///
    /// Returns [`DbError::DuplicateDay`] if the day is already archived.
    pub async fn insert_with_media(
        &self,
        post: &Post,
        media: &[NewMediaAttachment],
    ) -> DbResult<()> {
        let mut tx = self.pool.begin().await?;

        let inserted = sqlx::query!(
            r#"INSERT INTO posts
                   (series_id, day, message_id, channel_id, caption, posted_at, archived_at)
               VALUES (?, ?, ?, ?, ?, ?, ?)"#,
            post.series_id,
            post.day,
            post.message_id,
            post.channel_id,
            post.caption,
            post.posted_at,
            post.archived_at,
        )
        .execute(&mut *tx)
        .await;

        inserted.map_err(|e| {
            if is_unique_violation(&e) {
                DbError::DuplicateDay(post.day)
            } else {
                DbError::Sqlx(e)
            }
        })?;

        for m in media {
            let media_missing = i64::from(m.media_missing);
            sqlx::query!(
                r#"INSERT INTO media_attachments
                       (series_id, day, attachment_id, channel_id, message_id,
                        content_type, original_key, thumb_key, media_missing)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
                post.series_id,
                post.day,
                m.attachment_id,
                m.channel_id,
                m.message_id,
                m.content_type,
                m.original_key,
                m.thumb_key,
                media_missing,
            )
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;
        Ok(())
    }

    /// Fetches one archived day with its media, if present.
    pub async fn get(
        &self,
        series_id: i64,
        day: i64,
    ) -> DbResult<Option<(Post, Vec<MediaAttachment>)>> {
        let row = sqlx::query!(
            r#"SELECT series_id, day, message_id, channel_id, caption, posted_at, archived_at
               FROM posts WHERE series_id = ? AND day = ?"#,
            series_id,
            day
        )
        .fetch_optional(&self.pool)
        .await?;

        let Some(r) = row else { return Ok(None) };
        let post = Post {
            series_id: r.series_id,
            day: r.day,
            message_id: r.message_id,
            channel_id: r.channel_id,
            caption: r.caption,
            posted_at: r.posted_at,
            archived_at: r.archived_at,
        };

        let media = sqlx::query!(
            r#"SELECT id AS "id!: i64", series_id, day, attachment_id, channel_id, message_id,
                      content_type, original_key, thumb_key, media_missing
               FROM media_attachments WHERE series_id = ? AND day = ? ORDER BY id"#,
            series_id,
            day
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|m| MediaAttachment {
            id: m.id,
            series_id: m.series_id,
            day: m.day,
            attachment_id: m.attachment_id,
            channel_id: m.channel_id,
            message_id: m.message_id,
            content_type: m.content_type,
            original_key: m.original_key,
            thumb_key: m.thumb_key,
            media_missing: m.media_missing != 0,
        })
        .collect();

        Ok(Some((post, media)))
    }

    /// True if `day` is already archived in the series.
    pub async fn exists(&self, series_id: i64, day: i64) -> DbResult<bool> {
        let n = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM posts WHERE series_id = ? AND day = ?"#,
            series_id,
            day
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(n > 0)
    }

    /// Highest archived day number, if any.
    pub async fn max_day(&self, series_id: i64) -> DbResult<Option<i64>> {
        let max = sqlx::query_scalar!(
            r#"SELECT MAX(day) AS "max: i64" FROM posts WHERE series_id = ?"#,
            series_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(max)
    }

    /// `(day, posted_at, message_id, channel_id)` for every post, ascending
    /// by day. Input to the `/wrapped` recap (which buckets by timezone in
    /// pure code). A series is at most a few thousand rows, so loading all
    /// is cheaper than a SQL date-bucketing query that would hard-code a tz.
    pub async fn list_for_wrapped(
        &self,
        series_id: i64,
    ) -> DbResult<Vec<(i64, i64, String, String)>> {
        let rows = sqlx::query!(
            r#"SELECT day AS "day!: i64", posted_at AS "posted_at!: i64", message_id, channel_id
               FROM posts WHERE series_id = ? ORDER BY day"#,
            series_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.day, r.posted_at, r.message_id, r.channel_id))
            .collect())
    }

    /// Every archived day number, ascending (input to streak math).
    pub async fn all_days(&self, series_id: i64) -> DbResult<Vec<i64>> {
        let days = sqlx::query_scalar!(
            r#"SELECT day AS "day: i64" FROM posts WHERE series_id = ? ORDER BY day"#,
            series_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(days)
    }

    /// Archived day numbers within `[start, end]`, ascending.
    pub async fn days_in_range(&self, series_id: i64, start: i64, end: i64) -> DbResult<Vec<i64>> {
        let days = sqlx::query_scalar!(
            r#"SELECT day AS "day: i64" FROM posts
               WHERE series_id = ? AND day >= ? AND day <= ? ORDER BY day"#,
            series_id,
            start,
            end
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(days)
    }

    /// A uniformly random archived day, if the series has any.
    pub async fn random_day(&self, series_id: i64) -> DbResult<Option<i64>> {
        let day = sqlx::query_scalar!(
            r#"SELECT day AS "day: i64" FROM posts
               WHERE series_id = ? ORDER BY RANDOM() LIMIT 1"#,
            series_id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(day)
    }

    /// Total archived days in the series.
    pub async fn count(&self, series_id: i64) -> DbResult<i64> {
        let n = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM posts WHERE series_id = ?"#,
            series_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    /// Finds the `(series_id, day)` archived from a given message, if any
    /// (used by `/delete link:` and the 🗑️ context menu).
    pub async fn find_by_message(&self, message_id: &str) -> DbResult<Option<(i64, i64)>> {
        let row = sqlx::query!(
            r#"SELECT series_id, day FROM posts WHERE message_id = ?"#,
            message_id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.series_id, r.day)))
    }

    /// Deletes an archived day, returning the R2 object keys that should be
    /// removed from storage. Returns [`sqlx::Error::RowNotFound`] as an
    /// error if the day was not archived.
    pub async fn delete(&self, series_id: i64, day: i64) -> DbResult<Vec<String>> {
        let mut tx = self.pool.begin().await?;

        let keys = sqlx::query!(
            r#"SELECT original_key, thumb_key FROM media_attachments
               WHERE series_id = ? AND day = ?"#,
            series_id,
            day
        )
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .flat_map(|r| [r.original_key, r.thumb_key])
        .flatten()
        .collect();

        let result = sqlx::query!(
            "DELETE FROM posts WHERE series_id = ? AND day = ?",
            series_id,
            day
        )
        .execute(&mut *tx)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }

        tx.commit().await?;
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;
    use crate::db::testutil::test_pool;
    use crate::db::{GuildSettingsRepo, SeriesRepo};
    use crate::domain::{Cadence, DetectionMode, NewSeries, Privacy, SeriesState};

    async fn fixture() -> (tempfile::TempDir, PostRepo, i64) {
        let (dir, pool) = test_pool().await;
        GuildSettingsRepo::new(pool.clone())
            .ensure_exists("g")
            .await
            .unwrap();
        let series = SeriesRepo::new(pool.clone())
            .create(
                &NewSeries {
                    guild_id: "g".to_owned(),
                    creator_id: "u".to_owned(),
                    name: "s".to_owned(),
                    description: String::new(),
                    channels: vec![],
                    cadence: Cadence::Daily,
                    detection_mode: DetectionMode::ContextMenu,
                    privacy: Privacy::Public,
                    privacy_role_id: None,
                    start_day: 1,
                    state: SeriesState::Active,
                },
                0,
            )
            .await
            .unwrap();
        (dir, PostRepo::new(pool), series.id)
    }

    fn post(series_id: i64, day: i64) -> Post {
        Post {
            series_id,
            day,
            message_id: format!("m{day}"),
            channel_id: "c1".to_owned(),
            caption: format!("Day {day}"),
            posted_at: 1_700_000_000 + day,
            archived_at: 1_700_000_100 + day,
        }
    }

    fn media(n: u32) -> NewMediaAttachment {
        NewMediaAttachment {
            attachment_id: format!("a{n}"),
            channel_id: "c1".to_owned(),
            message_id: "m".to_owned(),
            content_type: "image/png".to_owned(),
            original_key: Some(format!("orig/{n}")),
            thumb_key: Some(format!("thumb/{n}")),
            media_missing: false,
        }
    }

    #[tokio::test]
    async fn insert_get_round_trip_with_media() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 1), &[media(1), media(2)])
            .await
            .unwrap();

        let (p, m) = repo.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(p, post(sid, 1));
        assert_eq!(m.len(), 2);
        assert_eq!(
            m.iter()
                .map(|x| x.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["a1", "a2"]
        );
        assert!(repo.get(sid, 2).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn duplicate_day_rejected_and_rolls_back() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 5), &[media(1)])
            .await
            .unwrap();
        let err = repo
            .insert_with_media(&post(sid, 5), &[media(2)])
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::DuplicateDay(5)));

        // The failed insert left no orphan media rows.
        let (_, m) = repo.get(sid, 5).await.unwrap().unwrap();
        assert_eq!(m.len(), 1);
    }

    #[tokio::test]
    async fn day_queries() {
        let (_d, repo, sid) = fixture().await;
        for day in [1, 2, 3, 7, 9] {
            repo.insert_with_media(&post(sid, day), &[]).await.unwrap();
        }
        assert!(repo.exists(sid, 3).await.unwrap());
        assert!(!repo.exists(sid, 4).await.unwrap());
        assert_eq!(repo.max_day(sid).await.unwrap(), Some(9));
        assert_eq!(repo.all_days(sid).await.unwrap(), vec![1, 2, 3, 7, 9]);
        assert_eq!(repo.days_in_range(sid, 2, 7).await.unwrap(), vec![2, 3, 7]);
        assert_eq!(repo.count(sid).await.unwrap(), 5);
        let r = repo.random_day(sid).await.unwrap().unwrap();
        assert!([1, 2, 3, 7, 9].contains(&r));
        assert_eq!(repo.find_by_message("m7").await.unwrap(), Some((sid, 7)));
        assert_eq!(repo.find_by_message("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn empty_series_queries() {
        let (_d, repo, sid) = fixture().await;
        assert_eq!(repo.max_day(sid).await.unwrap(), None);
        assert_eq!(repo.all_days(sid).await.unwrap(), Vec::<i64>::new());
        assert_eq!(repo.random_day(sid).await.unwrap(), None);
        assert_eq!(repo.count(sid).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn delete_returns_storage_keys_and_cascades() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 1), &[media(1), media(2)])
            .await
            .unwrap();

        let mut keys = repo.delete(sid, 1).await.unwrap();
        keys.sort();
        assert_eq!(keys, vec!["orig/1", "orig/2", "thumb/1", "thumb/2"]);
        assert!(repo.get(sid, 1).await.unwrap().is_none());

        // Deleting a missing day is an error, not a no-op.
        assert!(repo.delete(sid, 1).await.is_err());

        // Day can be re-archived after deletion.
        repo.insert_with_media(&post(sid, 1), &[]).await.unwrap();
        assert!(repo.exists(sid, 1).await.unwrap());
    }
}
