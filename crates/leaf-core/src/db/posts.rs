//! Repository for archived posts and their media attachments.

use sqlx::{Sqlite, SqlitePool, Transaction};

use super::{DbError, DbResult, is_unique_violation};
use crate::domain::{DaySummary, ExportRow, MediaAttachment, NewMediaAttachment, Post};

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
        insert_post(&mut tx, post).await?;
        insert_media(&mut tx, post.series_id, post.day, media).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Archives many days in ONE transaction, skipping days that are
    /// already archived (and later repeats of a day within `posts`).
    /// Returns `(imported, skipped)`.
    ///
    /// All-or-nothing: on any error nothing is written, so a failed import
    /// can simply be run again. One commit also means one fsync instead of
    /// one per day, which is what made large imports slow.
    pub async fn insert_many(
        &self,
        posts: &[(Post, Vec<NewMediaAttachment>)],
    ) -> DbResult<(usize, usize)> {
        let mut tx = self.pool.begin().await?;
        let mut imported = 0_usize;
        let mut skipped = 0_usize;

        for (post, media) in posts {
            let result = sqlx::query!(
                r#"INSERT INTO posts
                       (series_id, day, message_id, channel_id, caption, posted_at, archived_at)
                   VALUES (?, ?, ?, ?, ?, ?, ?)
                   ON CONFLICT (series_id, day) DO NOTHING"#,
                post.series_id,
                post.day,
                post.message_id,
                post.channel_id,
                post.caption,
                post.posted_at,
                post.archived_at,
            )
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() == 0 {
                skipped += 1;
                continue;
            }
            insert_media(&mut tx, post.series_id, post.day, media).await?;
            imported += 1;
        }

        tx.commit().await?;
        Ok((imported, skipped))
    }

    /// Archives `post` with `media`, replacing whatever was archived for
    /// that day before (if anything), in one transaction. The day is never
    /// left empty: a failure keeps the old entry intact.
    ///
    /// Returns the storage keys of the replaced entry that no media row
    /// holds any more; the caller removes those objects. Keys embed the
    /// series, day and attachment id, so re-archiving the same attachment
    /// yields the same keys, and deleting every old key would delete the
    /// upload that was just stored (or one a moved day still points at,
    /// see [`Self::move_day`]).
    pub async fn replace_with_media(
        &self,
        post: &Post,
        media: &[NewMediaAttachment],
    ) -> DbResult<Vec<String>> {
        let mut tx = self.pool.begin().await?;
        let stale = replace_day(&mut tx, post, media).await?;
        tx.commit().await?;
        Ok(stale)
    }

    /// [`Self::replace_with_media`] for filling in a day that holds no
    /// stored file, when and only when it still is one.
    ///
    /// The day is replaced only while it is still archived from
    /// `post.message_id` and none of its media rows holds a file. Both are
    /// checked inside the transaction that replaces it, so an entry written
    /// in the meantime (someone archived a real post as that day) is never
    /// overwritten and its files are never handed back for deletion.
    ///
    /// Returns `None`, with nothing changed, when the day is gone, holds
    /// another message, or has a stored file; otherwise the keys to remove,
    /// as [`Self::replace_with_media`] does.
    pub async fn replace_placeholder_with_media(
        &self,
        post: &Post,
        media: &[NewMediaAttachment],
    ) -> DbResult<Option<Vec<String>>> {
        let mut tx = self.pool.begin().await?;

        // A write first: it takes the write lock, so what the check below
        // reads cannot change before the replace. It changes nothing.
        let same_message = sqlx::query!(
            r#"UPDATE posts SET archived_at = archived_at
               WHERE series_id = ? AND day = ? AND message_id = ?"#,
            post.series_id,
            post.day,
            post.message_id,
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if same_message == 0 {
            return Ok(None);
        }
        let stored = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM media_attachments
               WHERE series_id = ? AND day = ?
                 AND (media_missing = 0 OR original_key IS NOT NULL OR thumb_key IS NOT NULL)"#,
            post.series_id,
            post.day,
        )
        .fetch_one(&mut *tx)
        .await?;
        if stored > 0 {
            return Ok(None);
        }

        let stale = replace_day(&mut tx, post, media).await?;
        tx.commit().await?;
        Ok(Some(stale))
    }

    /// Renumbers an archived day from `from` to `to`, keeping its media.
    ///
    /// `media_attachments` references `posts (series_id, day)` without
    /// `ON UPDATE CASCADE`, so the move is: insert the post under the new
    /// day, repoint the media rows, delete the old post, all in one
    /// transaction. Stored object keys are left as they are (they are read
    /// from the rows, never rebuilt from the day number).
    ///
    /// A moved day therefore keeps keys that name its old day number, and
    /// archiving the same message under that old number again stores to the
    /// very same keys: two days can share an object. Never delete a key
    /// from storage because one day stopped using it. [`Self::delete`] and
    /// [`Self::replace_with_media`] return only keys no row holds any more;
    /// for keys from anywhere else, ask [`Self::unreferenced_keys`] first.
    ///
    /// Returns [`DbError::DuplicateDay`] when `to` is already archived and
    /// [`sqlx::Error::RowNotFound`] as an error when `from` is not.
    pub async fn move_day(&self, series_id: i64, from: i64, to: i64) -> DbResult<()> {
        if from == to {
            return if self.exists(series_id, from).await? {
                Ok(())
            } else {
                Err(DbError::Sqlx(sqlx::Error::RowNotFound))
            };
        }

        let mut tx = self.pool.begin().await?;

        let copied = sqlx::query!(
            r#"INSERT INTO posts
                   (series_id, day, message_id, channel_id, caption, posted_at, archived_at)
               SELECT series_id, ?, message_id, channel_id, caption, posted_at, archived_at
               FROM posts WHERE series_id = ? AND day = ?"#,
            to,
            series_id,
            from,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                DbError::DuplicateDay(to)
            } else {
                DbError::Sqlx(e)
            }
        })?;
        if copied.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }

        sqlx::query!(
            "UPDATE media_attachments SET day = ? WHERE series_id = ? AND day = ?",
            to,
            series_id,
            from,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query!(
            "DELETE FROM posts WHERE series_id = ? AND day = ?",
            series_id,
            from
        )
        .execute(&mut *tx)
        .await?;

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

    /// Lowest archived day number, if any.
    pub async fn min_day(&self, series_id: i64) -> DbResult<Option<i64>> {
        let min = sqlx::query_scalar!(
            r#"SELECT MIN(day) AS "min: i64" FROM posts WHERE series_id = ?"#,
            series_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(min)
    }

    /// `posted_at` of the most recently posted day, if any (unix seconds).
    pub async fn latest_posted_at(&self, series_id: i64) -> DbResult<Option<i64>> {
        let latest = sqlx::query_scalar!(
            r#"SELECT MAX(posted_at) AS "latest: i64" FROM posts WHERE series_id = ?"#,
            series_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(latest)
    }

    /// The nearest archived days on either side of `day`, as
    /// `(previous, next)`. `day` itself need not be archived.
    pub async fn neighbor_days(
        &self,
        series_id: i64,
        day: i64,
    ) -> DbResult<(Option<i64>, Option<i64>)> {
        let row = sqlx::query!(
            r#"SELECT
                   (SELECT MAX(day) FROM posts WHERE series_id = ? AND day < ?) AS "prev?: i64",
                   (SELECT MIN(day) FROM posts WHERE series_id = ? AND day > ?) AS "next?: i64""#,
            series_id,
            day,
            series_id,
            day,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok((row.prev, row.next))
    }

    /// The lowest day number `>= from` that is not archived yet: `from`
    /// itself when free, otherwise the first gap after it.
    pub async fn first_free_day_from(&self, series_id: i64, from: i64) -> DbResult<i64> {
        // Candidates are `from` and the day after each archived day at or
        // past it; the answer is the smallest candidate that is free.
        let free = sqlx::query_scalar!(
            r#"SELECT MIN(c.d) AS "d?: i64" FROM (
                   SELECT ? AS d
                   UNION ALL
                   SELECT day + 1 FROM posts WHERE series_id = ? AND day >= ?
               ) c
               WHERE NOT EXISTS (
                   SELECT 1 FROM posts q WHERE q.series_id = ? AND q.day = c.d
               )"#,
            from,
            series_id,
            from,
            series_id,
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(free.unwrap_or(from))
    }

    /// Resolves an attachment id to its storage keys and type, for the
    /// media proxy. `None` if unknown; keys are `None` when the media is a
    /// missing placeholder (imported but never fetched).
    ///
    /// An attachment id is not unique: one post can be archived as two days
    /// or into two series, and an import records a failed download under the
    /// real id. A row that holds the file wins over one that does not, then
    /// the oldest.
    pub async fn media_location(
        &self,
        attachment_id: &str,
    ) -> DbResult<Option<(Option<String>, Option<String>, String)>> {
        let row = sqlx::query!(
            r#"SELECT original_key, thumb_key, content_type
               FROM media_attachments WHERE attachment_id = ?
               ORDER BY (original_key IS NULL), (thumb_key IS NULL), id LIMIT 1"#,
            attachment_id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.original_key, r.thumb_key, r.content_type)))
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

    /// Every archived day's tile summary, ascending by day, in one query
    /// (the gallery index; a series is at most a few thousand rows).
    pub async fn day_summaries(&self, series_id: i64) -> DbResult<Vec<DaySummary>> {
        let rows = sqlx::query!(
            r#"SELECT p.day AS "day!: i64", p.posted_at AS "posted_at!: i64",
                      m.attachment_id AS "first_attachment_id?: String",
                      (m.thumb_key IS NOT NULL) AS "has_thumb!: i64",
                      COALESCE(a.media_count, 0) AS "media_count!: i64",
                      COALESCE(a.stored_count, 0) AS "stored_count!: i64"
               FROM posts p
               LEFT JOIN (
                   SELECT day, COUNT(*) AS media_count,
                          SUM(media_missing = 0) AS stored_count,
                          MIN(id) AS first_id,
                          MIN(CASE WHEN thumb_key IS NOT NULL THEN id END) AS first_thumb_id
                   FROM media_attachments WHERE series_id = ? GROUP BY day
               ) a ON a.day = p.day
               LEFT JOIN media_attachments m ON m.id = COALESCE(a.first_thumb_id, a.first_id)
               WHERE p.series_id = ? ORDER BY p.day"#,
            series_id,
            series_id,
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|r| DaySummary {
                day: r.day,
                posted_at: r.posted_at,
                first_attachment_id: r.first_attachment_id,
                has_thumb: r.has_thumb != 0,
                missing: r.stored_count == 0,
                media_count: r.media_count,
            })
            .collect())
    }

    /// Every archived day with the object keys of its stored originals,
    /// ascending by day, in one query (input to `/export`).
    pub async fn export_rows(&self, series_id: i64) -> DbResult<Vec<ExportRow>> {
        let rows = sqlx::query!(
            r#"SELECT p.day AS "day!: i64", p.message_id AS "message_id!: String",
                      p.channel_id AS "channel_id!: String", p.caption AS "caption!: String",
                      p.posted_at AS "posted_at!: i64",
                      m.original_key AS "original_key?: String"
               FROM posts p
               LEFT JOIN media_attachments m ON m.series_id = p.series_id AND m.day = p.day
               WHERE p.series_id = ? ORDER BY p.day, m.id"#,
            series_id
        )
        .fetch_all(&self.pool)
        .await?;

        // One row per (day, attachment), already grouped by the ORDER BY.
        let mut out: Vec<ExportRow> = Vec::new();
        for r in rows {
            match out.last_mut() {
                Some(last) if last.day == r.day => last.media_keys.extend(r.original_key),
                _ => out.push(ExportRow {
                    day: r.day,
                    message_id: r.message_id,
                    channel_id: r.channel_id,
                    caption: r.caption,
                    posted_at: r.posted_at,
                    media_keys: r.original_key.into_iter().collect(),
                }),
            }
        }
        Ok(out)
    }

    /// Archived days with no stored media file: every media row is a
    /// `media_missing` placeholder (as `/import` writes them), or the day
    /// has no media rows at all. Ascending. These are the days a migration
    /// re-run can still fill in.
    pub async fn placeholder_days(&self, series_id: i64) -> DbResult<Vec<i64>> {
        let days = sqlx::query_scalar!(
            r#"SELECT p.day AS "day!: i64" FROM posts p
               WHERE p.series_id = ?
                 AND NOT EXISTS (
                     SELECT 1 FROM media_attachments m
                     WHERE m.series_id = p.series_id AND m.day = p.day
                       AND m.media_missing = 0
                 )
               ORDER BY p.day"#,
            series_id
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

    /// A random archived day that has stored media, so `/random` shows a
    /// picture whenever the series has one. Falls back to any archived day
    /// when none has media (a freshly imported series); `None` only for an
    /// empty series.
    pub async fn random_day_with_media(&self, series_id: i64) -> DbResult<Option<i64>> {
        let day = sqlx::query_scalar!(
            r#"SELECT p.day AS "day!: i64" FROM posts p
               WHERE p.series_id = ?
               ORDER BY EXISTS (
                            SELECT 1 FROM media_attachments m
                            WHERE m.series_id = p.series_id AND m.day = p.day
                              AND m.media_missing = 0
                        ) DESC,
                        RANDOM()
               LIMIT 1"#,
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

    /// Finds the `(series_id, day)` archived from a given message, if any.
    /// One message can back several entries (two series, or two days); this
    /// returns the first in `(series_id, day)` order, the same one every
    /// time. Use [`Self::find_all_by_message`] to see them all.
    pub async fn find_by_message(&self, message_id: &str) -> DbResult<Option<(i64, i64)>> {
        let row = sqlx::query!(
            r#"SELECT series_id, day FROM posts WHERE message_id = ?
               ORDER BY series_id, day LIMIT 1"#,
            message_id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| (r.series_id, r.day)))
    }

    /// Every `(series_id, day)` archived from a given message, ordered by
    /// series then day. `message_id` is not unique: the same message can be
    /// archived into two series, or as two days of one.
    pub async fn find_all_by_message(&self, message_id: &str) -> DbResult<Vec<(i64, i64)>> {
        let rows = sqlx::query!(
            r#"SELECT series_id, day FROM posts WHERE message_id = ?
               ORDER BY series_id, day"#,
            message_id
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|r| (r.series_id, r.day)).collect())
    }

    /// The R2 object keys (originals and thumbnails) stored for a day,
    /// without deleting anything. Empty when the day is unknown or holds
    /// only placeholders. This is what the day references, not what may be
    /// removed from storage: another day can hold the same key (see
    /// [`Self::move_day`]).
    pub async fn storage_keys(&self, series_id: i64, day: i64) -> DbResult<Vec<String>> {
        let keys = sqlx::query!(
            r#"SELECT original_key, thumb_key FROM media_attachments
               WHERE series_id = ? AND day = ? ORDER BY id"#,
            series_id,
            day
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .flat_map(|r| [r.original_key, r.thumb_key])
        .flatten()
        .collect();
        Ok(keys)
    }

    /// Narrows `keys` to the ones no media row holds (as an original or a
    /// thumbnail), in the order given and without repeats: the keys that
    /// are safe to remove from storage. Use it before cleaning up after a
    /// failed archive, where the uploads may have overwritten objects that
    /// an archived day still points at.
    pub async fn unreferenced_keys(&self, keys: &[String]) -> DbResult<Vec<String>> {
        unreferenced(&self.pool, keys).await
    }

    /// Deletes an archived day, returning the R2 object keys that should be
    /// removed from storage: the day's keys, minus any that another day
    /// still holds (see [`Self::move_day`]). Returns
    /// [`sqlx::Error::RowNotFound`] as an error if the day was not archived.
    pub async fn delete(&self, series_id: i64, day: i64) -> DbResult<Vec<String>> {
        let mut tx = self.pool.begin().await?;

        let keys: Vec<String> = sqlx::query!(
            r#"SELECT original_key, thumb_key FROM media_attachments
               WHERE series_id = ? AND day = ? ORDER BY id"#,
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

        // Checked after the cascade removed this day's media rows.
        let keys = unreferenced(&mut *tx, &keys).await?;
        tx.commit().await?;
        Ok(keys)
    }

    /// Takes back days that [`Self::insert_many`] wrote without media (an
    /// import), in ONE transaction: on any error nothing is removed, so it
    /// can simply be run again.
    ///
    /// `days` pairs each day with the message it was written for, and
    /// `archived_at` is the stamp the whole batch carried. A day is deleted
    /// only while it is still exactly that: the same message, the same
    /// stamp, and no media row that holds a file. A day that was archived
    /// for real since, replaced, or written by someone else is left alone.
    /// The check is part of the delete, so nothing can change in between.
    ///
    /// Returns `(removed, kept)`. `kept` counts the days that are still
    /// archived because they no longer match; days that are already gone
    /// count as neither. A removed day never held a stored file, so there
    /// are no storage keys to clean up.
    pub async fn delete_placeholders(
        &self,
        series_id: i64,
        archived_at: i64,
        days: &[(i64, &str)],
    ) -> DbResult<(usize, usize)> {
        let mut tx = self.pool.begin().await?;
        let mut removed = 0_usize;
        let mut kept = 0_usize;

        for &(day, message_id) in days {
            let result = sqlx::query!(
                r#"DELETE FROM posts
                   WHERE series_id = ? AND day = ? AND message_id = ? AND archived_at = ?
                     AND NOT EXISTS (
                         SELECT 1 FROM media_attachments m
                         WHERE m.series_id = posts.series_id AND m.day = posts.day
                           AND (m.media_missing = 0
                                OR m.original_key IS NOT NULL
                                OR m.thumb_key IS NOT NULL)
                     )"#,
                series_id,
                day,
                message_id,
                archived_at,
            )
            .execute(&mut *tx)
            .await?;

            if result.rows_affected() > 0 {
                removed += 1;
                continue;
            }
            let still_there = sqlx::query_scalar!(
                r#"SELECT COUNT(*) AS "n!: i64" FROM posts WHERE series_id = ? AND day = ?"#,
                series_id,
                day
            )
            .fetch_one(&mut *tx)
            .await?;
            if still_there > 0 {
                kept += 1;
            }
        }

        tx.commit().await?;
        Ok((removed, kept))
    }
}

/// The subset of `keys` that no `media_attachments` row holds, in the order
/// given and without repeats. Runs on `executor` so callers can ask inside
/// the transaction that just removed or replaced rows.
async fn unreferenced<'e, E>(executor: E, keys: &[String]) -> DbResult<Vec<String>>
where
    E: sqlx::SqliteExecutor<'e>,
{
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    // One statement for the whole batch: the keys travel as a JSON array.
    // Neither key column is indexed; one scan of a few thousand rows is
    // cheap next to the storage round trips it guards.
    let json = serde_json::to_string(keys)?;
    let held: Vec<String> = sqlx::query!(
        r#"SELECT m.original_key, m.thumb_key FROM media_attachments m
           WHERE m.original_key IN (SELECT value FROM json_each(?1))
              OR m.thumb_key IN (SELECT value FROM json_each(?1))"#,
        json
    )
    .fetch_all(executor)
    .await?
    .into_iter()
    .flat_map(|r| [r.original_key, r.thumb_key])
    .flatten()
    .collect();

    let mut free: Vec<String> = Vec::with_capacity(keys.len());
    for key in keys {
        if !held.contains(key) && !free.contains(key) {
            free.push(key.clone());
        }
    }
    Ok(free)
}

/// Replaces whatever is archived at `post`'s day with `post` and `media`,
/// and returns the replaced entry's storage keys that no media row holds
/// any more.
async fn replace_day(
    tx: &mut Transaction<'_, Sqlite>,
    post: &Post,
    media: &[NewMediaAttachment],
) -> DbResult<Vec<String>> {
    let replaced = sqlx::query!(
        r#"DELETE FROM media_attachments WHERE series_id = ? AND day = ?
               RETURNING original_key AS "original_key?: String",
                         thumb_key AS "thumb_key?: String""#,
        post.series_id,
        post.day
    )
    .fetch_all(&mut **tx)
    .await?;

    sqlx::query!(
        "DELETE FROM posts WHERE series_id = ? AND day = ?",
        post.series_id,
        post.day
    )
    .execute(&mut **tx)
    .await?;

    insert_post(tx, post).await?;
    insert_media(tx, post.series_id, post.day, media).await?;

    // Checked after the new rows exist, so keys they reuse are kept.
    let replaced: Vec<String> = replaced
        .into_iter()
        .flat_map(|r| [r.original_key, r.thumb_key])
        .flatten()
        .collect();
    unreferenced(&mut **tx, &replaced).await
}

/// Inserts the `posts` row inside `tx`, mapping a taken day to
/// [`DbError::DuplicateDay`].
async fn insert_post(tx: &mut Transaction<'_, Sqlite>, post: &Post) -> DbResult<()> {
    sqlx::query!(
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
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            DbError::DuplicateDay(post.day)
        } else {
            DbError::Sqlx(e)
        }
    })?;
    Ok(())
}

/// Inserts a day's media rows inside `tx`; the post row must already exist.
async fn insert_media(
    tx: &mut Transaction<'_, Sqlite>,
    series_id: i64,
    day: i64,
    media: &[NewMediaAttachment],
) -> DbResult<()> {
    for m in media {
        let media_missing = i64::from(m.media_missing);
        sqlx::query!(
            r#"INSERT INTO media_attachments
                       (series_id, day, attachment_id, channel_id, message_id,
                        content_type, original_key, thumb_key, media_missing)
                   VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            series_id,
            day,
            m.attachment_id,
            m.channel_id,
            m.message_id,
            m.content_type,
            m.original_key,
            m.thumb_key,
            media_missing,
        )
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
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
    async fn media_location_prefers_the_row_that_holds_the_file() {
        let (_d, repo, sid) = fixture().await;
        // An import that could not download the file, then the same post
        // archived again as another day with the file stored.
        let missing = NewMediaAttachment {
            original_key: None,
            thumb_key: None,
            media_missing: true,
            ..media(1)
        };
        repo.insert_with_media(&post(sid, 1), std::slice::from_ref(&missing))
            .await
            .unwrap();
        assert_eq!(
            repo.media_location("a1").await.unwrap(),
            Some((None, None, "image/png".to_owned()))
        );

        // Stored without a thumbnail (a video), then stored in full.
        let no_thumb = NewMediaAttachment {
            thumb_key: None,
            ..media(1)
        };
        repo.insert_with_media(&post(sid, 2), &[no_thumb])
            .await
            .unwrap();
        assert_eq!(
            repo.media_location("a1").await.unwrap(),
            Some((Some("orig/1".to_owned()), None, "image/png".to_owned()))
        );
        let mut full = media(1);
        full.original_key = Some("orig/later".to_owned());
        repo.insert_with_media(&post(sid, 3), &[full, missing])
            .await
            .unwrap();
        assert_eq!(
            repo.media_location("a1").await.unwrap(),
            Some((
                Some("orig/later".to_owned()),
                Some("thumb/1".to_owned()),
                "image/png".to_owned()
            ))
        );
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

    /// A `media_missing` placeholder row, as `/import` writes them.
    fn placeholder(n: u32) -> NewMediaAttachment {
        NewMediaAttachment {
            attachment_id: format!("import-m-{n}"),
            channel_id: "c1".to_owned(),
            message_id: "m".to_owned(),
            content_type: String::new(),
            original_key: None,
            thumb_key: None,
            media_missing: true,
        }
    }

    /// A second series in the fixture guild.
    async fn second_series(repo: &PostRepo) -> i64 {
        SeriesRepo::new(repo.pool.clone())
            .create(
                &NewSeries {
                    guild_id: "g".to_owned(),
                    creator_id: "u2".to_owned(),
                    name: "s2".to_owned(),
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
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn message_lookup_is_deterministic_and_complete() {
        let (_d, repo, sid) = fixture().await;
        let other = second_series(&repo).await;

        // One message archived as two days of one series and into another
        // series, inserted out of order.
        let same = |series_id: i64, day: i64| Post {
            message_id: "shared".to_owned(),
            ..post(series_id, day)
        };
        repo.insert_with_media(&same(other, 3), &[]).await.unwrap();
        repo.insert_with_media(&same(sid, 9), &[]).await.unwrap();
        repo.insert_with_media(&same(sid, 4), &[]).await.unwrap();
        repo.insert_with_media(&post(sid, 5), &[]).await.unwrap();

        assert_eq!(
            repo.find_all_by_message("shared").await.unwrap(),
            vec![(sid, 4), (sid, 9), (other, 3)]
        );
        assert_eq!(
            repo.find_by_message("shared").await.unwrap(),
            Some((sid, 4))
        );
        assert_eq!(
            repo.find_all_by_message("m5").await.unwrap(),
            vec![(sid, 5)]
        );
        assert!(repo.find_all_by_message("nope").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn day_summaries_cover_stored_placeholder_and_bare_days() {
        let (_d, repo, sid) = fixture().await;
        let other = second_series(&repo).await;

        // Day 1: two stored files. Day 2: placeholders only. Day 3: no
        // media rows. Day 4: a placeholder first, then a stored file.
        repo.insert_with_media(&post(sid, 1), &[media(1), media(2)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 2), &[placeholder(1), placeholder(2)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 3), &[]).await.unwrap();
        repo.insert_with_media(&post(sid, 4), &[placeholder(3), media(3)])
            .await
            .unwrap();
        // Another series' media must not leak into the counts.
        repo.insert_with_media(&post(other, 1), &[media(9)])
            .await
            .unwrap();

        let got = repo.day_summaries(sid).await.unwrap();
        let expect = |day: i64, first: Option<&str>, has_thumb, missing, media_count| DaySummary {
            day,
            posted_at: 1_700_000_000 + day,
            first_attachment_id: first.map(str::to_owned),
            has_thumb,
            missing,
            media_count,
        };
        assert_eq!(
            got,
            vec![
                expect(1, Some("a1"), true, false, 2),
                expect(2, Some("import-m-1"), false, true, 2),
                expect(3, None, false, true, 0),
                // The tile uses the first attachment that has a thumbnail.
                expect(4, Some("a3"), true, false, 2),
            ]
        );
        assert!(repo.day_summaries(9999).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn export_rows_group_media_keys_per_day() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 2), &[media(1), placeholder(1), media(2)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 1), &[]).await.unwrap();
        repo.insert_with_media(&post(sid, 3), &[placeholder(2)])
            .await
            .unwrap();

        let got = repo.export_rows(sid).await.unwrap();
        let expect = |day: i64, keys: &[&str]| ExportRow {
            day,
            message_id: format!("m{day}"),
            channel_id: "c1".to_owned(),
            caption: format!("Day {day}"),
            posted_at: 1_700_000_000 + day,
            media_keys: keys.iter().map(|k| (*k).to_owned()).collect(),
        };
        assert_eq!(
            got,
            vec![
                expect(1, &[]),
                expect(2, &["orig/1", "orig/2"]),
                expect(3, &[]),
            ]
        );
        assert!(repo.export_rows(9999).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn insert_many_imports_new_days_and_skips_existing() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 2), &[media(1)])
            .await
            .unwrap();

        let batch = vec![
            (post(sid, 1), vec![placeholder(1)]),
            (post(sid, 2), vec![placeholder(2)]), // already archived
            (post(sid, 3), vec![]),
            (post(sid, 3), vec![placeholder(3)]), // repeated within the batch
        ];
        assert_eq!(repo.insert_many(&batch).await.unwrap(), (2, 2));
        assert_eq!(repo.all_days(sid).await.unwrap(), vec![1, 2, 3]);

        // Skipped days keep their media; no rows are added to them.
        let (_, m) = repo.get(sid, 2).await.unwrap().unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m.first().unwrap().attachment_id, "a1");
        let (_, m) = repo.get(sid, 3).await.unwrap().unwrap();
        assert!(m.is_empty());
        let (_, m) = repo.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(m.len(), 1);

        // Re-running the same batch is a no-op.
        assert_eq!(repo.insert_many(&batch).await.unwrap(), (0, 4));
        assert_eq!(repo.insert_many(&[]).await.unwrap(), (0, 0));
    }

    #[tokio::test]
    async fn insert_many_is_all_or_nothing() {
        let (_d, repo, sid) = fixture().await;
        // The second entry names a series that does not exist, which the
        // foreign key rejects after the first entry was already written.
        let batch = vec![(post(sid, 1), vec![media(1)]), (post(9999, 1), vec![])];
        assert!(repo.insert_many(&batch).await.is_err());
        assert_eq!(repo.count(sid).await.unwrap(), 0);
        assert!(repo.media_location("a1").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn move_day_keeps_post_and_media() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 5), &[media(1), media(2)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 6), &[media(3)])
            .await
            .unwrap();

        repo.move_day(sid, 5, 50).await.unwrap();

        assert!(repo.get(sid, 5).await.unwrap().is_none());
        let (p, m) = repo.get(sid, 50).await.unwrap().unwrap();
        assert_eq!(
            p,
            Post {
                day: 50,
                ..post(sid, 5)
            }
        );
        assert_eq!(
            m.iter()
                .map(|x| (x.day, x.attachment_id.as_str(), x.original_key.as_deref()))
                .collect::<Vec<_>>(),
            [(50, "a1", Some("orig/1")), (50, "a2", Some("orig/2"))]
        );
        // The neighbouring day is untouched, and no media row is orphaned.
        let (_, m) = repo.get(sid, 6).await.unwrap().unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(repo.all_days(sid).await.unwrap(), vec![6, 50]);
        assert!(repo.storage_keys(sid, 5).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn move_day_refuses_a_taken_or_missing_day() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 5), &[media(1)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 6), &[media(2)])
            .await
            .unwrap();

        // Target taken: nothing moves.
        let err = repo.move_day(sid, 5, 6).await.unwrap_err();
        assert!(matches!(err, DbError::DuplicateDay(6)));
        let (_, m) = repo.get(sid, 5).await.unwrap().unwrap();
        assert_eq!(m.first().unwrap().attachment_id, "a1");
        let (_, m) = repo.get(sid, 6).await.unwrap().unwrap();
        assert_eq!(m.first().unwrap().attachment_id, "a2");

        // Source missing.
        assert!(repo.move_day(sid, 7, 8).await.is_err());
        assert!(!repo.exists(sid, 8).await.unwrap());

        // Moving a day onto itself is a no-op when it exists.
        repo.move_day(sid, 5, 5).await.unwrap();
        assert!(repo.move_day(sid, 7, 7).await.is_err());
    }

    #[tokio::test]
    async fn replace_with_media_swaps_a_day_and_reports_stale_keys() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 1), &[media(1), media(2)])
            .await
            .unwrap();

        // The replacement re-uses attachment 2 (same keys) and adds 3.
        let replacement = Post {
            message_id: "new".to_owned(),
            ..post(sid, 1)
        };
        let mut stale = repo
            .replace_with_media(&replacement, &[media(2), media(3)])
            .await
            .unwrap();
        stale.sort();
        assert_eq!(stale, vec!["orig/1", "thumb/1"]);

        let (p, m) = repo.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(p.message_id, "new");
        assert_eq!(
            m.iter()
                .map(|x| x.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["a2", "a3"]
        );

        // Replacing a day that was never archived simply archives it.
        let stale = repo
            .replace_with_media(&post(sid, 2), &[media(4)])
            .await
            .unwrap();
        assert!(stale.is_empty());
        assert!(repo.exists(sid, 2).await.unwrap());
    }

    #[tokio::test]
    async fn replace_with_media_keeps_the_old_day_on_failure() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 1), &[media(1)])
            .await
            .unwrap();

        // Make the second media insert fail, after the old rows were
        // deleted and the new post and first media row were written.
        sqlx::query(
            "CREATE TRIGGER reject_boom BEFORE INSERT ON media_attachments
             WHEN NEW.attachment_id = 'boom'
             BEGIN SELECT RAISE(ABORT, 'boom'); END",
        )
        .execute(&repo.pool)
        .await
        .unwrap();
        let boom = NewMediaAttachment {
            attachment_id: "boom".to_owned(),
            ..media(3)
        };
        let replacement = Post {
            message_id: "new".to_owned(),
            ..post(sid, 1)
        };
        assert!(
            repo.replace_with_media(&replacement, &[media(2), boom])
                .await
                .is_err()
        );

        let (p, m) = repo.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(p, post(sid, 1));
        assert_eq!(
            m.iter()
                .map(|x| x.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["a1"]
        );
    }

    #[tokio::test]
    async fn delete_keeps_keys_a_moved_day_shares() {
        let (_d, repo, sid) = fixture().await;
        // Day 5 is renumbered to 6 (its keys still name day 5), then the
        // same message is archived as Day 5 again, to the very same keys.
        repo.insert_with_media(&post(sid, 5), &[media(1)])
            .await
            .unwrap();
        repo.move_day(sid, 5, 6).await.unwrap();
        repo.insert_with_media(&post(sid, 5), &[media(1), media(2)])
            .await
            .unwrap();

        // Removing Day 6 must not release the objects Day 5 still shows.
        assert!(repo.delete(sid, 6).await.unwrap().is_empty());
        assert_eq!(
            repo.storage_keys(sid, 5).await.unwrap(),
            vec!["orig/1", "thumb/1", "orig/2", "thumb/2"]
        );

        // The last holder releases every key, each one once.
        assert_eq!(
            repo.delete(sid, 5).await.unwrap(),
            vec!["orig/1", "thumb/1", "orig/2", "thumb/2"]
        );
    }

    #[tokio::test]
    async fn replace_keeps_keys_a_moved_day_shares() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 5), &[media(1), media(2)])
            .await
            .unwrap();
        repo.move_day(sid, 5, 6).await.unwrap();
        repo.insert_with_media(&post(sid, 5), &[media(1), media(2)])
            .await
            .unwrap();

        // Day 6 is replaced by something else entirely. Its old keys are
        // still Day 5's, so none of them is stale.
        let stale = repo
            .replace_with_media(&post(sid, 6), &[media(3)])
            .await
            .unwrap();
        assert!(stale.is_empty());

        // Replacing Day 5 now frees attachment 1 (nobody holds it) but not
        // attachment 2, which the replacement reuses.
        let mut stale = repo
            .replace_with_media(&post(sid, 5), &[media(2)])
            .await
            .unwrap();
        stale.sort();
        assert_eq!(stale, vec!["orig/1", "thumb/1"]);
    }

    #[tokio::test]
    async fn unreferenced_keys_filters_held_keys() {
        let (_d, repo, sid) = fixture().await;
        let other = second_series(&repo).await;
        repo.insert_with_media(&post(sid, 1), &[media(1)])
            .await
            .unwrap();
        // A thumbnail-less row in another series: holders are found
        // wherever they are, by either key column.
        let no_thumb = NewMediaAttachment {
            thumb_key: None,
            ..media(2)
        };
        repo.insert_with_media(&post(other, 1), &[no_thumb])
            .await
            .unwrap();

        let keys = |names: &[&str]| names.iter().map(|k| (*k).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            repo.unreferenced_keys(&keys(&[
                "orig/9", "orig/1", "thumb/2", "thumb/1", "orig/2", "orig/9", "it's"
            ]))
            .await
            .unwrap(),
            // Input order, repeats dropped; odd characters survive the trip.
            vec!["orig/9", "thumb/2", "it's"]
        );
        assert!(repo.unreferenced_keys(&[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn range_helpers() {
        let (_d, repo, sid) = fixture().await;
        for day in [3, 4, 5, 9] {
            repo.insert_with_media(&post(sid, day), &[]).await.unwrap();
        }

        assert_eq!(repo.min_day(sid).await.unwrap(), Some(3));
        // post() stamps `posted_at` from the day, so day 9 is the latest.
        assert_eq!(
            repo.latest_posted_at(sid).await.unwrap(),
            Some(1_700_000_009)
        );

        for (day, expected) in [
            (1, (None, Some(3))),
            (3, (None, Some(4))),
            (4, (Some(3), Some(5))),
            (7, (Some(5), Some(9))),
            (9, (Some(5), None)),
            (20, (Some(9), None)),
        ] {
            assert_eq!(
                repo.neighbor_days(sid, day).await.unwrap(),
                expected,
                "day {day}"
            );
        }

        for (from, expected) in [(1, 1), (3, 6), (4, 6), (6, 6), (9, 10), (10, 10), (50, 50)] {
            assert_eq!(
                repo.first_free_day_from(sid, from).await.unwrap(),
                expected,
                "from {from}"
            );
        }
    }

    #[tokio::test]
    async fn range_helpers_on_an_empty_series() {
        let (_d, repo, sid) = fixture().await;
        assert_eq!(repo.min_day(sid).await.unwrap(), None);
        assert_eq!(repo.latest_posted_at(sid).await.unwrap(), None);
        assert_eq!(repo.neighbor_days(sid, 5).await.unwrap(), (None, None));
        assert_eq!(repo.first_free_day_from(sid, 5).await.unwrap(), 5);
        assert_eq!(repo.random_day_with_media(sid).await.unwrap(), None);
        assert!(repo.placeholder_days(sid).await.unwrap().is_empty());
        assert!(repo.storage_keys(sid, 1).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn random_day_prefers_days_with_stored_media() {
        let (_d, repo, sid) = fixture().await;
        for day in 1..=20 {
            repo.insert_with_media(&post(sid, day), &[placeholder(1)])
                .await
                .unwrap();
        }
        // No day has media yet: any day will do.
        let any = repo.random_day_with_media(sid).await.unwrap().unwrap();
        assert!((1..=20).contains(&any));

        // Once one day has a stored file it is always the pick. The
        // preference is an ORDER BY, not a probability, so this holds on
        // every draw.
        repo.insert_with_media(&post(sid, 21), &[placeholder(2), media(1)])
            .await
            .unwrap();
        for _ in 0..10 {
            assert_eq!(repo.random_day_with_media(sid).await.unwrap(), Some(21));
        }
    }

    #[tokio::test]
    async fn a_placeholder_is_replaced_only_while_it_still_is_one() {
        let (_d, repo, sid) = fixture().await;
        // Day 1: a placeholder from m1. Filling it in works.
        repo.insert_with_media(&post(sid, 1), &[placeholder(1)])
            .await
            .unwrap();
        let freed = repo
            .replace_placeholder_with_media(&post(sid, 1), &[media(1)])
            .await
            .unwrap();
        assert_eq!(freed, Some(Vec::new()));
        let (_, rows) = repo.get(sid, 1).await.unwrap().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows.iter().all(|row| !row.media_missing));

        // It now holds a file: a second fill-in (a stale list, a day named
        // twice) changes nothing and frees nothing, even when its own rows
        // carry no file.
        let again = repo
            .replace_placeholder_with_media(&post(sid, 1), &[placeholder(9)])
            .await
            .unwrap();
        assert_eq!(again, None);
        let (_, rows) = repo.get(sid, 1).await.unwrap().unwrap();
        let kept = rows.first().and_then(|row| row.original_key.as_deref());
        assert_eq!(kept, Some("orig/1"));

        // Day 2: a placeholder that another message was archived over.
        repo.insert_with_media(&post(sid, 2), &[placeholder(2)])
            .await
            .unwrap();
        let mut other = post(sid, 2);
        other.message_id = "someone-else".to_owned();
        repo.replace_with_media(&other, &[placeholder(3)])
            .await
            .unwrap();
        let refused = repo
            .replace_placeholder_with_media(&post(sid, 2), &[media(2)])
            .await
            .unwrap();
        assert_eq!(refused, None);
        let (kept, _) = repo.get(sid, 2).await.unwrap().unwrap();
        assert_eq!(kept.message_id, "someone-else");

        // A day that is not archived at all is not created.
        let absent = repo
            .replace_placeholder_with_media(&post(sid, 3), &[media(3)])
            .await
            .unwrap();
        assert_eq!(absent, None);
        assert!(!repo.exists(sid, 3).await.unwrap());
    }

    #[tokio::test]
    async fn placeholder_days_lists_days_without_stored_media() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 1), &[media(1)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 2), &[placeholder(1), placeholder(2)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 3), &[placeholder(3), media(2)])
            .await
            .unwrap();
        repo.insert_with_media(&post(sid, 4), &[]).await.unwrap();

        assert_eq!(repo.placeholder_days(sid).await.unwrap(), vec![2, 4]);
    }

    #[tokio::test]
    async fn storage_keys_lists_without_deleting() {
        let (_d, repo, sid) = fixture().await;
        repo.insert_with_media(&post(sid, 1), &[media(1), placeholder(1), media(2)])
            .await
            .unwrap();

        assert_eq!(
            repo.storage_keys(sid, 1).await.unwrap(),
            vec!["orig/1", "thumb/1", "orig/2", "thumb/2"]
        );
        assert!(repo.exists(sid, 1).await.unwrap());
        assert!(repo.storage_keys(sid, 2).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_placeholders_takes_back_only_what_is_still_as_written() {
        const STAMP: i64 = 1_800_000_000;
        let (_d, repo, sid) = fixture().await;
        // What the import wrote: eight days, one stamp, no stored files.
        let written = |day: i64| Post {
            archived_at: STAMP,
            ..post(sid, day)
        };
        let batch: Vec<(Post, Vec<NewMediaAttachment>)> = (1..=8_u32)
            .map(|day| (written(i64::from(day)), vec![placeholder(day)]))
            .chain([(written(9), vec![])])
            .collect();
        assert_eq!(repo.insert_many(&batch).await.unwrap(), (9, 0));

        // Day 3: archived for real since, from the same message.
        repo.replace_with_media(&post(sid, 3), &[media(1)])
            .await
            .unwrap();
        // Day 4: removed, then written again for the same message, without
        // media, by another import (its own stamp).
        repo.delete(sid, 4).await.unwrap();
        let again = Post {
            archived_at: STAMP + 60,
            ..post(sid, 4)
        };
        repo.insert_with_media(&again, &[placeholder(40)])
            .await
            .unwrap();
        // Day 5: removed by someone else.
        repo.delete(sid, 5).await.unwrap();
        // Day 6: another message holds the day now, same stamp.
        repo.delete(sid, 6).await.unwrap();
        let other = Post {
            message_id: "other".to_owned(),
            ..written(6)
        };
        repo.insert_with_media(&other, &[]).await.unwrap();
        // Day 7: still the same row, but a file was stored for it.
        repo.replace_with_media(&written(7), &[placeholder(7), media(2)])
            .await
            .unwrap();

        // Day 8 is as written, but is not asked for.
        let asked = [
            (1, "m1"),
            (2, "m2"),
            (3, "m3"),
            (4, "m4"),
            (5, "m5"),
            (6, "m6"),
            (7, "m7"),
            (9, "m9"),
        ];
        // Another series' days are never touched, whatever they hold.
        let elsewhere = second_series(&repo).await;
        let twin = Post {
            series_id: elsewhere,
            ..written(1)
        };
        repo.insert_with_media(&twin, &[]).await.unwrap();

        assert_eq!(
            repo.delete_placeholders(sid, STAMP, &asked).await.unwrap(),
            (3, 4)
        );
        assert_eq!(repo.all_days(sid).await.unwrap(), vec![3, 4, 6, 7, 8]);
        assert_eq!(repo.all_days(elsewhere).await.unwrap(), vec![1]);
        // The placeholders went with their days; stored files were kept.
        assert!(repo.media_location("import-m-1").await.unwrap().is_none());
        assert!(repo.media_location("import-m-8").await.unwrap().is_some());
        assert_eq!(repo.storage_keys(sid, 7).await.unwrap().len(), 2);

        // Running it again finds nothing left to take back.
        assert_eq!(
            repo.delete_placeholders(sid, STAMP, &asked).await.unwrap(),
            (0, 4)
        );
        // The stamp is part of the match.
        assert_eq!(
            repo.delete_placeholders(sid, STAMP + 1, &[(8, "m8")])
                .await
                .unwrap(),
            (0, 1)
        );
        assert_eq!(
            repo.delete_placeholders(sid, STAMP, &[]).await.unwrap(),
            (0, 0)
        );
    }
}
