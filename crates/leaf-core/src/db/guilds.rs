//! Repository for per-guild settings (Tier-2 configuration).

use sqlx::SqlitePool;

use super::{DbResult, from_json_ids, to_json_ids};
use crate::domain::GuildSettings;

/// CRUD over the `guild_settings` table.
#[derive(Debug, Clone)]
pub struct GuildSettingsRepo {
    pool: SqlitePool,
}

impl GuildSettingsRepo {
    /// Creates a repo over `pool`.
    #[must_use]
    pub const fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Inserts default settings for `guild_id` if no row exists yet.
    /// Idempotent; called on guild join.
    pub async fn ensure_exists(&self, guild_id: &str) -> DbResult<()> {
        sqlx::query!(
            "INSERT OR IGNORE INTO guild_settings (guild_id) VALUES (?)",
            guild_id
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Fetches settings for `guild_id`, if the guild is known.
    pub async fn get(&self, guild_id: &str) -> DbResult<Option<GuildSettings>> {
        let row = sqlx::query!(
            r#"SELECT guild_id, setup_complete, log_channel_id, watched_channels,
                      creator_role_id, timezone, max_series_per_user,
                      min_account_age_days, min_membership_age_days,
                      sprout_enabled, sprout_threshold, active_persona
               FROM guild_settings WHERE guild_id = ?"#,
            guild_id
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(|r| {
            Ok(GuildSettings {
                guild_id: r.guild_id,
                setup_complete: r.setup_complete != 0,
                log_channel_id: r.log_channel_id,
                watched_channels: from_json_ids(&r.watched_channels)?,
                creator_role_id: r.creator_role_id,
                timezone: r.timezone,
                max_series_per_user: r.max_series_per_user,
                min_account_age_days: r.min_account_age_days,
                min_membership_age_days: r.min_membership_age_days,
                sprout_enabled: r.sprout_enabled != 0,
                sprout_threshold: r.sprout_threshold,
                active_persona: r.active_persona,
            })
        })
        .transpose()
    }

    /// Writes the full settings row (upsert). `setup_complete` is part of
    /// the row: completing `/setup` is an update like any other.
    pub async fn upsert(&self, s: &GuildSettings) -> DbResult<()> {
        let watched = to_json_ids(&s.watched_channels)?;
        let setup_complete = i64::from(s.setup_complete);
        let sprout_enabled = i64::from(s.sprout_enabled);
        sqlx::query!(
            r#"INSERT INTO guild_settings
                   (guild_id, setup_complete, log_channel_id, watched_channels,
                    creator_role_id, timezone, max_series_per_user,
                    min_account_age_days, min_membership_age_days,
                    sprout_enabled, sprout_threshold, active_persona)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
               ON CONFLICT (guild_id) DO UPDATE SET
                    setup_complete = excluded.setup_complete,
                    log_channel_id = excluded.log_channel_id,
                    watched_channels = excluded.watched_channels,
                    creator_role_id = excluded.creator_role_id,
                    timezone = excluded.timezone,
                    max_series_per_user = excluded.max_series_per_user,
                    min_account_age_days = excluded.min_account_age_days,
                    min_membership_age_days = excluded.min_membership_age_days,
                    sprout_enabled = excluded.sprout_enabled,
                    sprout_threshold = excluded.sprout_threshold,
                    active_persona = excluded.active_persona"#,
            s.guild_id,
            setup_complete,
            s.log_channel_id,
            watched,
            s.creator_role_id,
            s.timezone,
            s.max_series_per_user,
            s.min_account_age_days,
            s.min_membership_age_days,
            sprout_enabled,
            s.sprout_threshold,
            s.active_persona,
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
    use crate::db::testutil::test_pool;

    #[tokio::test]
    async fn ensure_then_get_returns_defaults() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);
        repo.ensure_exists("g1").await.unwrap();
        repo.ensure_exists("g1").await.unwrap(); // idempotent

        let s = repo.get("g1").await.unwrap().unwrap();
        assert_eq!(s, GuildSettings::defaults_for("g1"));
        assert!(repo.get("missing").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn upsert_round_trips_every_field() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);

        let mut s = GuildSettings::defaults_for("g2");
        s.setup_complete = true;
        s.log_channel_id = Some("c-log".to_owned());
        s.watched_channels = vec!["c1".to_owned(), "c2".to_owned()];
        s.creator_role_id = Some("r1".to_owned());
        s.timezone = "America/Chicago".to_owned();
        s.max_series_per_user = 5;
        s.min_account_age_days = 30;
        s.min_membership_age_days = 7;
        s.sprout_enabled = true;
        s.sprout_threshold = 4;
        s.active_persona = "weary".to_owned();

        repo.upsert(&s).await.unwrap();
        assert_eq!(repo.get("g2").await.unwrap().unwrap(), s);

        // Upsert over an existing row updates in place.
        s.timezone = "UTC".to_owned();
        repo.upsert(&s).await.unwrap();
        assert_eq!(repo.get("g2").await.unwrap().unwrap(), s);
    }
}
