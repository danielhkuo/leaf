//! Repository for series (the core abstraction).

use sqlx::SqlitePool;

use super::{DbError, DbResult, from_json_ids, is_unique_violation, to_json_ids};
use crate::domain::{NewSeries, Series, SeriesState};
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

    /// Updates the mutable fields of an existing series.
    pub async fn update(&self, s: &Series) -> DbResult<()> {
        let channels = to_json_ids(&s.channels)?;
        let cadence = s.cadence.as_str();
        let detection = s.detection_mode.as_str();
        let privacy = s.privacy.as_str();
        let reminder_enabled = i64::from(s.reminder_enabled);
        let reminder_dm = i64::from(s.reminder_dm);
        let result = sqlx::query!(
            r#"UPDATE series SET
                   name = ?, description = ?, channels = ?, cadence = ?,
                   detection_mode = ?, privacy = ?, privacy_role_id = ?,
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
            reminder_enabled,
            s.reminder_time,
            s.reminder_timezone,
            reminder_dm,
            s.milestone_template,
            s.emoji,
            s.id,
        )
        .execute(&self.pool)
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
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;
    use crate::db::GuildSettingsRepo;
    use crate::db::testutil::test_pool;
    use crate::domain::{Cadence, DetectionMode, Privacy};

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
}
