//! Repository for series (the core abstraction).

use sqlx::SqlitePool;

use super::{DbError, DbResult, from_json_ids, is_unique_violation, to_json_ids};
use crate::domain::{NewSeries, ReminderFailure, Series, SeriesState};
use crate::reminder::ReminderCandidate;

/// CRUD over the `series` table.
#[derive(Debug, Clone)]
pub struct SeriesRepo {
    pool: SqlitePool,
}

/// Internal row shape shared by every SELECT in this module.
struct Row {
    id: i64,
    guild_id: String,
    creator_id: String,
    name: String,
    description: String,
    channels: String,
    cadence: String,
    detection_mode: String,
    privacy: String,
    privacy_role_id: Option<String>,
    start_day: i64,
    reminder_enabled: i64,
    reminder_time: Option<String>,
    reminder_timezone: Option<String>,
    reminder_dm: i64,
    milestone_template: Option<String>,
    emoji: String,
    state: String,
    created_at: i64,
}

impl Row {
    fn into_series(self) -> DbResult<Series> {
        Ok(Series {
            id: self.id,
            guild_id: self.guild_id,
            creator_id: self.creator_id,
            name: self.name,
            description: self.description,
            channels: from_json_ids(&self.channels)?,
            cadence: self.cadence.parse()?,
            detection_mode: self.detection_mode.parse()?,
            privacy: self.privacy.parse()?,
            privacy_role_id: self.privacy_role_id,
            start_day: self.start_day,
            reminder_enabled: self.reminder_enabled != 0,
            reminder_time: self.reminder_time,
            reminder_timezone: self.reminder_timezone,
            reminder_dm: self.reminder_dm != 0,
            milestone_template: self.milestone_template,
            emoji: self.emoji,
            state: self.state.parse()?,
            created_at: self.created_at,
        })
    }
}

// sqlx requires the SQL in `query_as!` to be a plain string literal, so the
// column list below is repeated per query; `Row` keeps the mapping single.

impl SeriesRepo {
    /// Creates a repo over `pool`.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// The pool this repo runs on, so a caller that holds only a repo
    /// (leaf-server's API state) can build a sibling repo over the same
    /// database.
    #[must_use]
    pub const fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Inserts a new series, returning it with its assigned id.
    ///
    /// Returns [`DbError::SeriesNameTaken`] when the `(guild, name)` pair
    /// already exists.
    pub async fn create(&self, new: &NewSeries, now_unix: i64) -> DbResult<Series> {
        let channels = to_json_ids(&new.channels)?;
        let cadence = new.cadence.as_str();
        let detection = new.detection_mode.as_str();
        let privacy = new.privacy.as_str();
        let state = new.state.as_str();
        let result = sqlx::query!(
            r#"INSERT INTO series
                   (guild_id, creator_id, name, description, channels, cadence,
                    detection_mode, privacy, privacy_role_id, start_day, state,
                    created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
            new.guild_id,
            new.creator_id,
            new.name,
            new.description,
            channels,
            cadence,
            detection,
            privacy,
            new.privacy_role_id,
            new.start_day,
            state,
            now_unix,
        )
        .execute(&self.pool)
        .await;

        let done = result.map_err(|e| {
            if is_unique_violation(&e) {
                DbError::SeriesNameTaken
            } else {
                DbError::Sqlx(e)
            }
        })?;

        let id = done.last_insert_rowid();
        self.get(id)
            .await?
            .ok_or(DbError::Sqlx(sqlx::Error::RowNotFound))
    }

    /// Fetches a series by id.
    pub async fn get(&self, id: i64) -> DbResult<Option<Series>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!: i64", guild_id, creator_id, name, description, channels,
                      cadence, detection_mode, privacy, privacy_role_id, start_day,
                      reminder_enabled, reminder_time, reminder_timezone, reminder_dm,
                      milestone_template, emoji, state, created_at
               FROM series WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await?
        .map(Row::into_series)
        .transpose()
    }

    /// Fetches a series by its per-guild unique name.
    pub async fn get_by_name(&self, guild_id: &str, name: &str) -> DbResult<Option<Series>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!: i64", guild_id, creator_id, name, description, channels,
                      cadence, detection_mode, privacy, privacy_role_id, start_day,
                      reminder_enabled, reminder_time, reminder_timezone, reminder_dm,
                      milestone_template, emoji, state, created_at
               FROM series WHERE guild_id = ? AND name = ?"#,
            guild_id,
            name
        )
        .fetch_optional(&self.pool)
        .await?
        .map(Row::into_series)
        .transpose()
    }

    /// All series in a guild, oldest first.
    pub async fn list_by_guild(&self, guild_id: &str) -> DbResult<Vec<Series>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!: i64", guild_id, creator_id, name, description, channels,
                      cadence, detection_mode, privacy, privacy_role_id, start_day,
                      reminder_enabled, reminder_time, reminder_timezone, reminder_dm,
                      milestone_template, emoji, state, created_at
               FROM series WHERE guild_id = ? ORDER BY id"#,
            guild_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::into_series)
        .collect()
    }

    /// All series owned by `creator_id` in a guild, oldest first.
    pub async fn list_by_creator(&self, guild_id: &str, creator_id: &str) -> DbResult<Vec<Series>> {
        sqlx::query_as!(
            Row,
            r#"SELECT id AS "id!: i64", guild_id, creator_id, name, description, channels,
                      cadence, detection_mode, privacy, privacy_role_id, start_day,
                      reminder_enabled, reminder_time, reminder_timezone, reminder_dm,
                      milestone_template, emoji, state, created_at
               FROM series WHERE guild_id = ? AND creator_id = ? ORDER BY id"#,
            guild_id,
            creator_id
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(Row::into_series)
        .collect()
    }

    /// Number of non-revoked series `creator_id` has in a guild (for the
    /// max-per-user policy check).
    pub async fn count_live_by_creator(&self, guild_id: &str, creator_id: &str) -> DbResult<i64> {
        let n = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM series
               WHERE guild_id = ? AND creator_id = ? AND state != 'revoked'"#,
            guild_id,
            creator_id
        )
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }

    /// Updates the mutable fields of an existing series, including its name
    /// and `start_day`.
    ///
    /// A recorded reminder failure describes the delivery route that was in
    /// place when it happened, so it is cleared when that route changes:
    /// reminders toggled, DM versus channel ping, or the channels while the
    /// series is pinged in a channel. (Channels play no part in a DM, so a
    /// "DMs closed" warning survives a channel edit.) The creator who
    /// follows the advice to switch routes then stops seeing the stale
    /// warning straight away.
    ///
    /// Returns [`DbError::SeriesNameTaken`] when the new name collides.
    pub async fn update(&self, s: &Series) -> DbResult<()> {
        let channels = to_json_ids(&s.channels)?;
        let cadence = s.cadence.as_str();
        let detection = s.detection_mode.as_str();
        let privacy = s.privacy.as_str();
        let reminder_enabled = i64::from(s.reminder_enabled);
        let reminder_dm = i64::from(s.reminder_dm);
        let mut tx = self.pool.begin().await?;

        // Compares against the stored values, so it must run first.
        sqlx::query!(
            r#"UPDATE series SET reminder_error = NULL, reminder_error_at = NULL
               WHERE id = ? AND reminder_error IS NOT NULL
                 AND (reminder_enabled != ? OR reminder_dm != ?
                      OR (reminder_dm = 0 AND channels != ?))"#,
            s.id,
            reminder_enabled,
            reminder_dm,
            channels,
        )
        .execute(&mut *tx)
        .await?;

        let result = sqlx::query!(
            r#"UPDATE series SET
                   name = ?, description = ?, channels = ?, cadence = ?,
                   detection_mode = ?, privacy = ?, privacy_role_id = ?,
                   start_day = ?,
                   reminder_enabled = ?, reminder_time = ?, reminder_timezone = ?,
                   reminder_dm = ?, milestone_template = ?, emoji = ?
               WHERE id = ?"#,
            s.name,
            s.description,
            channels,
            cadence,
            detection,
            privacy,
            s.privacy_role_id,
            s.start_day,
            reminder_enabled,
            s.reminder_time,
            s.reminder_timezone,
            reminder_dm,
            s.milestone_template,
            s.emoji,
            s.id,
        )
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                DbError::SeriesNameTaken
            } else {
                DbError::Sqlx(e)
            }
        })?;

        if result.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        tx.commit().await?;
        Ok(())
    }

    /// Transitions a series' lifecycle state (sprout promotion, revoke).
    pub async fn set_state(&self, id: i64, state: SeriesState) -> DbResult<()> {
        let state = state.as_str();
        let result = sqlx::query!("UPDATE series SET state = ? WHERE id = ?", state, id)
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// Moves a series from `from` to `to` only if it is still in `from`,
    /// and says whether it moved. For transitions decided from a copy that
    /// may be old (a sprout's promotion after a long upload, and its undo):
    /// a series an admin revoked meanwhile stays revoked.
    pub async fn set_state_if(
        &self,
        id: i64,
        from: SeriesState,
        to: SeriesState,
    ) -> DbResult<bool> {
        let (from, to) = (from.as_str(), to.as_str());
        let result = sqlx::query!(
            "UPDATE series SET state = ? WHERE id = ? AND state = ?",
            to,
            id,
            from
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Deletes a series and (via FK cascade) all its posts and media rows.
    pub async fn delete(&self, id: i64) -> DbResult<()> {
        sqlx::query!("DELETE FROM series WHERE id = ?", id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Updates the reminder configuration for a series.
    pub async fn set_reminder_config(
        &self,
        id: i64,
        enabled: bool,
        time: Option<&str>,
        timezone: Option<&str>,
        dm: bool,
    ) -> DbResult<()> {
        let enabled = i64::from(enabled);
        let dm = i64::from(dm);
        let affected = sqlx::query!(
            r#"UPDATE series SET reminder_enabled = ?, reminder_time = ?,
                   reminder_timezone = ?, reminder_dm = ? WHERE id = ?"#,
            enabled,
            time,
            timezone,
            dm,
            id,
        )
        .execute(&self.pool)
        .await?;
        if affected.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// Every reminder-enabled, non-revoked series with a reminder time set,
    /// joined with post aggregates and a resolved timezone (series override
    /// else guild default). Drives the scheduler tick across all guilds.
    pub async fn reminder_candidates(&self) -> DbResult<Vec<ReminderCandidate>> {
        let rows = sqlx::query!(
            r#"SELECT s.id AS "id!: i64", s.guild_id, s.name, s.creator_id,
                      s.channels, s.cadence, s.start_day,
                      s.reminder_time AS "reminder_time!: String",
                      COALESCE(s.reminder_timezone, g.timezone) AS "timezone!: String",
                      s.reminder_dm, s.last_reminder_day,
                      (s.privacy = 'public' AND s.state = 'active') AS "listed!: i64",
                      (SELECT MAX(day) FROM posts p WHERE p.series_id = s.id) AS "max_day: i64",
                      (SELECT MAX(posted_at) FROM posts p WHERE p.series_id = s.id) AS "last_post_at: i64"
               FROM series s
               JOIN guild_settings g ON g.guild_id = s.guild_id
               WHERE s.reminder_enabled = 1
                 AND s.reminder_time IS NOT NULL
                 AND s.state != 'revoked'"#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|r| {
                Ok(ReminderCandidate {
                    series_id: r.id,
                    guild_id: r.guild_id,
                    name: r.name,
                    creator_id: r.creator_id,
                    channels: from_json_ids(&r.channels)?,
                    cadence: r.cadence.parse()?,
                    reminder_time: r.reminder_time,
                    timezone: r.timezone,
                    reminder_dm: r.reminder_dm != 0,
                    start_day: r.start_day,
                    max_day: r.max_day,
                    last_post_at: r.last_post_at,
                    last_reminder_day: r.last_reminder_day,
                    listed: r.listed != 0,
                })
            })
            .collect()
    }

    /// Records reminder bookkeeping: the day last reminded for (`None` to
    /// roll back after a failed send) and the check timestamp.
    pub async fn set_reminder_state(
        &self,
        id: i64,
        last_reminder_day: Option<i64>,
        now_unix: i64,
    ) -> DbResult<()> {
        sqlx::query!(
            "UPDATE series SET last_reminder_day = ?, last_reminder_check = ? WHERE id = ?",
            last_reminder_day,
            now_unix,
            id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Records why the last reminder could not be delivered (`Some`), or
    /// clears the record after a successful send (`None`). The reason must
    /// be a [`ReminderFailureKind::as_str`] value: those are the only ones
    /// the API and the Activity know how to explain. `now_unix` is stored
    /// as the failure time and ignored when clearing. Like
    /// [`Self::set_reminder_state`], a series deleted mid-tick is not an
    /// error.
    ///
    /// [`ReminderFailureKind::as_str`]: crate::domain::ReminderFailureKind::as_str
    pub async fn set_reminder_error(
        &self,
        id: i64,
        error: Option<&str>,
        now_unix: i64,
    ) -> DbResult<()> {
        let at = error.map(|_| now_unix);
        sqlx::query!(
            "UPDATE series SET reminder_error = ?, reminder_error_at = ? WHERE id = ?",
            error,
            at,
            id,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// The recorded reminder delivery failure for a series, if any. `None`
    /// while delivery works (and for an unknown series).
    pub async fn reminder_error(&self, id: i64) -> DbResult<Option<ReminderFailure>> {
        let row = sqlx::query!(
            r#"SELECT reminder_error AS "reason?: String",
                      reminder_error_at AS "at?: i64"
               FROM series WHERE id = ?"#,
            id
        )
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.and_then(|r| {
            r.reason.map(|reason| ReminderFailure {
                reason,
                at: r.at.unwrap_or(0),
            })
        }))
    }

    /// Publishes sprouts in a guild in one statement and returns how many
    /// became active. With `threshold: Some(n)` only sprouts that already
    /// have at least `n` archived days are promoted (the threshold was
    /// lowered); with `None` every sprout is (probation was switched off).
    /// Revoked and active series are never touched.
    pub async fn promote_sprouts(&self, guild_id: &str, threshold: Option<i64>) -> DbResult<u64> {
        let result = sqlx::query!(
            r#"UPDATE series SET state = 'active'
               WHERE guild_id = ? AND state = 'sprout'
                 AND (? IS NULL
                      OR (SELECT COUNT(*) FROM posts p WHERE p.series_id = series.id) >= ?)"#,
            guild_id,
            threshold,
            threshold,
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;
    use crate::db::testutil::test_pool;
    use crate::db::{GuildSettingsRepo, PostRepo};
    use crate::domain::{Cadence, DetectionMode, Post, Privacy, ReminderFailureKind};

    fn sample_new(guild: &str, creator: &str, name: &str) -> NewSeries {
        NewSeries {
            guild_id: guild.to_owned(),
            creator_id: creator.to_owned(),
            name: name.to_owned(),
            description: "a sketch a day".to_owned(),
            channels: vec!["c1".to_owned()],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
            state: SeriesState::Active,
        }
    }

    async fn repo_with_guild(guild: &str) -> (tempfile::TempDir, SeriesRepo) {
        let (dir, pool) = test_pool().await;
        GuildSettingsRepo::new(pool.clone())
            .ensure_exists(guild)
            .await
            .unwrap();
        (dir, SeriesRepo::new(pool))
    }

    #[tokio::test]
    async fn reminder_candidates_say_whether_everyone_may_see_the_series() {
        let (_dir, repo) = repo_with_guild("g").await;
        let public = sample_new("g", "u1", "public");
        let mut private = sample_new("g", "u1", "private");
        private.privacy = Privacy::CreatorOnly;
        let mut sprout = sample_new("g", "u1", "sprout");
        sprout.state = SeriesState::Sprout;
        for new in [&public, &private, &sprout] {
            let created = repo.create(new, 1).await.unwrap();
            repo.set_reminder_config(created.id, true, Some("17:30"), None, false)
                .await
                .unwrap();
        }
        let listed: Vec<(String, bool)> = repo
            .reminder_candidates()
            .await
            .unwrap()
            .into_iter()
            .map(|c| (c.name, c.listed))
            .collect();
        assert_eq!(
            listed,
            [
                ("public".to_owned(), true),
                ("private".to_owned(), false),
                ("sprout".to_owned(), false),
            ]
        );
    }

    #[tokio::test]
    async fn create_get_round_trip() {
        let (_dir, repo) = repo_with_guild("g").await;
        let created = repo
            .create(&sample_new("g", "u1", "daily-sketch"), 1000)
            .await
            .unwrap();
        assert_eq!(created.created_at, 1000);
        assert_eq!(created.emoji, "🍃"); // schema default
        assert_eq!(repo.get(created.id).await.unwrap().unwrap(), created);
        assert_eq!(
            repo.get_by_name("g", "daily-sketch")
                .await
                .unwrap()
                .unwrap(),
            created
        );
    }

    #[tokio::test]
    async fn duplicate_name_in_guild_is_rejected() {
        let (_dir, repo) = repo_with_guild("g").await;
        repo.create(&sample_new("g", "u1", "dup"), 0).await.unwrap();
        let err = repo
            .create(&sample_new("g", "u2", "dup"), 0)
            .await
            .unwrap_err();
        assert!(matches!(err, DbError::SeriesNameTaken));
    }

    #[tokio::test]
    async fn update_and_state_transitions() {
        let (_dir, repo) = repo_with_guild("g").await;
        let mut s = repo.create(&sample_new("g", "u1", "s"), 0).await.unwrap();
        s.description = "edited".to_owned();
        s.privacy = Privacy::CreatorOnly;
        s.emoji = "🌿".to_owned();
        repo.update(&s).await.unwrap();
        assert_eq!(repo.get(s.id).await.unwrap().unwrap(), s);

        repo.set_state(s.id, SeriesState::Revoked).await.unwrap();
        assert_eq!(
            repo.get(s.id).await.unwrap().unwrap().state,
            SeriesState::Revoked
        );

        // Missing rows surface as errors, not silent no-ops.
        assert!(repo.set_state(9999, SeriesState::Active).await.is_err());
    }

    #[tokio::test]
    async fn counts_exclude_revoked() {
        let (_dir, repo) = repo_with_guild("g").await;
        let a = repo.create(&sample_new("g", "u1", "a"), 0).await.unwrap();
        repo.create(&sample_new("g", "u1", "b"), 0).await.unwrap();
        repo.create(&sample_new("g", "u2", "c"), 0).await.unwrap();
        assert_eq!(repo.count_live_by_creator("g", "u1").await.unwrap(), 2);
        repo.set_state(a.id, SeriesState::Revoked).await.unwrap();
        assert_eq!(repo.count_live_by_creator("g", "u1").await.unwrap(), 1);
        assert_eq!(repo.list_by_creator("g", "u1").await.unwrap().len(), 2);
        assert_eq!(repo.list_by_guild("g").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn update_writes_name_and_start_day() {
        let (_dir, repo) = repo_with_guild("g").await;
        let mut s = repo.create(&sample_new("g", "u1", "old"), 0).await.unwrap();
        repo.create(&sample_new("g", "u2", "taken"), 0)
            .await
            .unwrap();

        s.name = "new".to_owned();
        s.start_day = 250;
        repo.update(&s).await.unwrap();
        assert_eq!(repo.get(s.id).await.unwrap().unwrap(), s);
        assert!(repo.get_by_name("g", "old").await.unwrap().is_none());

        // Renaming onto another series is refused and changes nothing.
        let mut clash = s.clone();
        clash.name = "taken".to_owned();
        clash.start_day = 1;
        let err = repo.update(&clash).await.unwrap_err();
        assert!(matches!(err, DbError::SeriesNameTaken));
        assert_eq!(repo.get(s.id).await.unwrap().unwrap(), s);

        // Missing rows surface as errors, not silent no-ops.
        let mut ghost = s.clone();
        ghost.id = 9999;
        ghost.name = "ghost".to_owned();
        assert!(repo.update(&ghost).await.is_err());
    }

    #[tokio::test]
    async fn reminder_error_round_trip() {
        let (_dir, repo) = repo_with_guild("g").await;
        let s = repo.create(&sample_new("g", "u1", "s"), 0).await.unwrap();
        assert_eq!(repo.reminder_error(s.id).await.unwrap(), None);
        assert_eq!(repo.reminder_error(9999).await.unwrap(), None);

        repo.set_reminder_error(s.id, Some(ReminderFailureKind::DmClosed.as_str()), 500)
            .await
            .unwrap();
        let failure = repo.reminder_error(s.id).await.unwrap().unwrap();
        assert_eq!(
            failure,
            ReminderFailure {
                reason: "dm_closed".to_owned(),
                at: 500
            }
        );
        assert_eq!(failure.kind(), Some(ReminderFailureKind::DmClosed));

        // A later failure replaces the earlier one.
        repo.set_reminder_error(
            s.id,
            Some(ReminderFailureKind::ChannelMissing.as_str()),
            900,
        )
        .await
        .unwrap();
        assert_eq!(
            repo.reminder_error(s.id).await.unwrap(),
            Some(ReminderFailure {
                reason: "channel_missing".to_owned(),
                at: 900
            })
        );

        // Clearing ignores the timestamp; a deleted series is not an error.
        repo.set_reminder_error(s.id, None, 1200).await.unwrap();
        assert_eq!(repo.reminder_error(s.id).await.unwrap(), None);
        repo.set_reminder_error(9999, Some("x"), 0).await.unwrap();
    }

    #[tokio::test]
    async fn update_clears_reminder_error_only_when_the_route_changes() {
        let (_dir, repo) = repo_with_guild("g").await;
        let mut s = repo.create(&sample_new("g", "u1", "s"), 0).await.unwrap();
        s.reminder_enabled = true;
        s.reminder_dm = true;
        s.reminder_time = Some("21:00".to_owned());
        repo.update(&s).await.unwrap();
        let fail = async |kind: ReminderFailureKind, at: i64| {
            repo.set_reminder_error(s.id, Some(kind.as_str()), at)
                .await
                .unwrap();
        };
        let recorded = async || repo.reminder_error(s.id).await.unwrap();

        // Edits that leave the delivery route alone keep the warning. In DM
        // mode that includes the channels, which a DM never touches.
        fail(ReminderFailureKind::DmClosed, 500).await;
        s.description = "edited".to_owned();
        s.reminder_time = Some("09:30".to_owned());
        s.reminder_timezone = Some("America/Chicago".to_owned());
        s.channels = vec!["c2".to_owned()];
        repo.update(&s).await.unwrap();
        assert!(recorded().await.is_some());

        // Switching from DM to a channel ping clears it.
        s.reminder_dm = false;
        repo.update(&s).await.unwrap();
        assert_eq!(recorded().await, None);

        // With channel pings, an unrelated edit still keeps the warning...
        fail(ReminderFailureKind::ChannelMissing, 600).await;
        s.description = "edited again".to_owned();
        repo.update(&s).await.unwrap();
        assert!(recorded().await.is_some());

        // ...pointing the series at another channel clears it...
        s.channels = vec!["c3".to_owned()];
        repo.update(&s).await.unwrap();
        assert_eq!(recorded().await, None);

        // ...and so does switching back to DMs, or turning reminders off.
        fail(ReminderFailureKind::NoPermission, 700).await;
        s.reminder_dm = true;
        repo.update(&s).await.unwrap();
        assert_eq!(recorded().await, None);

        fail(ReminderFailureKind::DmClosed, 800).await;
        s.reminder_enabled = false;
        repo.update(&s).await.unwrap();
        assert_eq!(recorded().await, None);
    }

    #[tokio::test]
    async fn promote_sprouts_by_threshold_or_all() {
        let (_dir, pool) = test_pool().await;
        let guilds = GuildSettingsRepo::new(pool.clone());
        guilds.ensure_exists("g").await.unwrap();
        guilds.ensure_exists("other").await.unwrap();
        let repo = SeriesRepo::new(pool.clone());
        let posts = PostRepo::new(pool);

        let sprout = |guild: &str, name: &str| NewSeries {
            state: SeriesState::Sprout,
            ..sample_new(guild, "u1", name)
        };
        let two = repo.create(&sprout("g", "two"), 0).await.unwrap();
        let three = repo.create(&sprout("g", "three"), 0).await.unwrap();
        let empty = repo.create(&sprout("g", "empty"), 0).await.unwrap();
        let revoked = repo.create(&sprout("g", "revoked"), 0).await.unwrap();
        repo.set_state(revoked.id, SeriesState::Revoked)
            .await
            .unwrap();
        let elsewhere = repo.create(&sprout("other", "elsewhere"), 0).await.unwrap();

        for (series, days) in [(&two, 2), (&three, 3), (&revoked, 5), (&elsewhere, 5)] {
            for day in 1..=days {
                let post = Post {
                    series_id: series.id,
                    day,
                    message_id: format!("m{day}"),
                    channel_id: "c1".to_owned(),
                    caption: String::new(),
                    posted_at: day,
                    archived_at: day,
                };
                posts.insert_with_media(&post, &[]).await.unwrap();
            }
        }
        let state = async |id: i64| repo.get(id).await.unwrap().unwrap().state;

        // A lowered threshold releases only the sprouts that already meet it.
        assert_eq!(repo.promote_sprouts("g", Some(3)).await.unwrap(), 1);
        assert_eq!(state(three.id).await, SeriesState::Active);
        assert_eq!(state(two.id).await, SeriesState::Sprout);
        assert_eq!(state(empty.id).await, SeriesState::Sprout);
        // Nothing left to do at the same threshold.
        assert_eq!(repo.promote_sprouts("g", Some(3)).await.unwrap(), 0);

        // Probation switched off: every remaining sprout is published.
        assert_eq!(repo.promote_sprouts("g", None).await.unwrap(), 2);
        assert_eq!(state(two.id).await, SeriesState::Active);
        assert_eq!(state(empty.id).await, SeriesState::Active);

        // Revoked series and other guilds are never touched.
        assert_eq!(state(revoked.id).await, SeriesState::Revoked);
        assert_eq!(state(elsewhere.id).await, SeriesState::Sprout);
    }

    #[tokio::test]
    async fn set_state_if_moves_only_from_the_expected_state() {
        let (_dir, repo) = repo_with_guild("g1").await;
        let mut new = sample_new("g1", "u1", "Sprouting");
        new.state = SeriesState::Sprout;
        let s = repo.create(&new, 0).await.unwrap();

        // A sprout is promoted, once.
        let (sprout, active) = (SeriesState::Sprout, SeriesState::Active);
        assert!(repo.set_state_if(s.id, sprout, active).await.unwrap());
        assert!(!repo.set_state_if(s.id, sprout, active).await.unwrap());
        assert_eq!(repo.get(s.id).await.unwrap().unwrap().state, active);

        // A series revoked meanwhile is neither promoted nor put back.
        repo.set_state(s.id, SeriesState::Revoked).await.unwrap();
        assert!(!repo.set_state_if(s.id, sprout, active).await.unwrap());
        assert!(!repo.set_state_if(s.id, active, sprout).await.unwrap());
        assert_eq!(
            repo.get(s.id).await.unwrap().unwrap().state,
            SeriesState::Revoked
        );

        // An unknown series did not move.
        assert!(!repo.set_state_if(9999, sprout, active).await.unwrap());
    }
}
