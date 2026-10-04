//! Repository for per-guild settings (Tier-2 configuration).

use sqlx::SqlitePool;

use super::{DbError, DbResult, from_json_ids, to_json_ids};
use crate::domain::GuildSettings;

/// What one save in the admin panel changes: `None` leaves a column as it
/// is. The two ids are `Some(None)` to clear them.
///
/// The panel never writes `setup_complete` or the watched channels (those
/// are `/setup`'s), and it writes only what the admin changed, so a `/setup`
/// saved in chat while the panel was checking a role with Discord survives.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PanelChange {
    /// The guild's default IANA timezone, already validated.
    pub timezone: Option<String>,
    /// The role required to create series (`Some(None)` lets everyone).
    pub creator_role_id: Option<Option<String>>,
    /// The log channel (`Some(None)` switches the log off).
    pub log_channel_id: Option<Option<String>>,
    /// Series each member may own.
    pub max_series_per_user: Option<i64>,
    /// Minimum Discord account age, in days.
    pub min_account_age_days: Option<i64>,
    /// Minimum time in the server, in days.
    pub min_membership_age_days: Option<i64>,
    /// Whether new series start as sprouts.
    pub sprout_enabled: Option<bool>,
    /// Days a sprout needs before it is published.
    pub sprout_threshold: Option<i64>,
}

impl PanelChange {
    /// The columns in which `after` differs from `before`, among those the
    /// panel owns.
    #[must_use]
    pub fn between(before: &GuildSettings, after: &GuildSettings) -> Self {
        fn changed<T: PartialEq + Clone>(before: &T, after: &T) -> Option<T> {
            (before != after).then(|| after.clone())
        }
        Self {
            timezone: changed(&before.timezone, &after.timezone),
            creator_role_id: changed(&before.creator_role_id, &after.creator_role_id),
            log_channel_id: changed(&before.log_channel_id, &after.log_channel_id),
            max_series_per_user: changed(&before.max_series_per_user, &after.max_series_per_user),
            min_account_age_days: changed(
                &before.min_account_age_days,
                &after.min_account_age_days,
            ),
            min_membership_age_days: changed(
                &before.min_membership_age_days,
                &after.min_membership_age_days,
            ),
            sprout_enabled: changed(&before.sprout_enabled, &after.sprout_enabled),
            sprout_threshold: changed(&before.sprout_threshold, &after.sprout_threshold),
        }
    }
}

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
    /// Idempotent. Returns `true` when this call created the row, which is
    /// how the bot tells a guild it has never seen (greet it) from one it
    /// already knows, whether or not the gateway was up when it was added.
    pub async fn ensure_exists(&self, guild_id: &str) -> DbResult<bool> {
        let result = sqlx::query!(
            "INSERT OR IGNORE INTO guild_settings (guild_id) VALUES (?)",
            guild_id
        )
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
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

    /// Saves what `/setup` chooses, the watched channels and the log
    /// channel, and marks setup complete. Every other column is left as it
    /// is, so a policy change made in the admin panel while the `/setup`
    /// prompt was open survives the save. Creates the row with defaults if
    /// the guild is not known yet.
    pub async fn set_channels(
        &self,
        guild_id: &str,
        watched: &[String],
        log_channel_id: Option<&str>,
    ) -> DbResult<()> {
        let watched = to_json_ids(watched)?;
        sqlx::query!(
            r#"INSERT INTO guild_settings
                   (guild_id, setup_complete, log_channel_id, watched_channels)
               VALUES (?, 1, ?, ?)
               ON CONFLICT (guild_id) DO UPDATE SET
                    setup_complete = 1,
                    log_channel_id = excluded.log_channel_id,
                    watched_channels = excluded.watched_channels"#,
            guild_id,
            log_channel_id,
            watched,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Saves what the admin panel changed, and nothing else: see
    /// [`PanelChange`]. Returns [`sqlx::Error::RowNotFound`] as an error
    /// for an unknown guild.
    pub async fn apply_panel_change(&self, guild_id: &str, c: &PanelChange) -> DbResult<()> {
        let set_role = i64::from(c.creator_role_id.is_some());
        let role = c.creator_role_id.as_ref().and_then(Option::as_deref);
        let set_log = i64::from(c.log_channel_id.is_some());
        let log = c.log_channel_id.as_ref().and_then(Option::as_deref);
        let sprout_enabled = c.sprout_enabled.map(i64::from);
        let result = sqlx::query!(
            r#"UPDATE guild_settings SET
                    timezone = COALESCE(?, timezone),
                    creator_role_id = CASE WHEN ? THEN ? ELSE creator_role_id END,
                    log_channel_id = CASE WHEN ? THEN ? ELSE log_channel_id END,
                    max_series_per_user = COALESCE(?, max_series_per_user),
                    min_account_age_days = COALESCE(?, min_account_age_days),
                    min_membership_age_days = COALESCE(?, min_membership_age_days),
                    sprout_enabled = COALESCE(?, sprout_enabled),
                    sprout_threshold = COALESCE(?, sprout_threshold)
               WHERE guild_id = ?"#,
            c.timezone,
            set_role,
            role,
            set_log,
            log,
            c.max_series_per_user,
            c.min_account_age_days,
            c.min_membership_age_days,
            sprout_enabled,
            c.sprout_threshold,
            guild_id,
        )
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// Sets only the role required to create series (`None` lets everyone
    /// create). Targeted for the same reason as [`Self::set_channels`].
    /// Returns [`sqlx::Error::RowNotFound`] as an error for an unknown guild.
    pub async fn set_creator_role(&self, guild_id: &str, role_id: Option<&str>) -> DbResult<()> {
        let result = sqlx::query!(
            "UPDATE guild_settings SET creator_role_id = ? WHERE guild_id = ?",
            role_id,
            guild_id,
        )
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
        Ok(())
    }

    /// Sets only the guild's default IANA timezone. The caller validates
    /// the name. Returns [`sqlx::Error::RowNotFound`] as an error for an
    /// unknown guild.
    pub async fn set_timezone(&self, guild_id: &str, timezone: &str) -> DbResult<()> {
        let result = sqlx::query!(
            "UPDATE guild_settings SET timezone = ? WHERE guild_id = ?",
            timezone,
            guild_id,
        )
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::Sqlx(sqlx::Error::RowNotFound));
        }
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
        // Only the call that creates the row reports an insert.
        assert!(repo.ensure_exists("g1").await.unwrap());
        assert!(!repo.ensure_exists("g1").await.unwrap());

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

        // A row made by upsert is not "new" to a later ensure_exists.
        assert!(!repo.ensure_exists("g2").await.unwrap());
    }

    #[tokio::test]
    async fn set_channels_touches_only_setup_fields() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);

        let mut s = GuildSettings::defaults_for("g3");
        s.creator_role_id = Some("r1".to_owned());
        s.timezone = "America/Chicago".to_owned();
        s.max_series_per_user = 9;
        s.sprout_enabled = true;
        repo.upsert(&s).await.unwrap();

        let watched = vec!["c1".to_owned(), "c2".to_owned()];
        repo.set_channels("g3", &watched, Some("c-log"))
            .await
            .unwrap();

        s.setup_complete = true;
        s.watched_channels = watched;
        s.log_channel_id = Some("c-log".to_owned());
        assert_eq!(repo.get("g3").await.unwrap().unwrap(), s);

        // The log channel can be cleared; setup stays complete.
        repo.set_channels("g3", &[], None).await.unwrap();
        s.watched_channels = Vec::new();
        s.log_channel_id = None;
        assert_eq!(repo.get("g3").await.unwrap().unwrap(), s);
    }

    #[tokio::test]
    async fn set_channels_creates_a_missing_row_with_defaults() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);

        repo.set_channels("g4", &["c1".to_owned()], None)
            .await
            .unwrap();

        let mut expected = GuildSettings::defaults_for("g4");
        expected.setup_complete = true;
        expected.watched_channels = vec!["c1".to_owned()];
        assert_eq!(repo.get("g4").await.unwrap().unwrap(), expected);
    }

    #[tokio::test]
    async fn targeted_role_and_timezone_setters() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);
        repo.ensure_exists("g5").await.unwrap();

        repo.set_creator_role("g5", Some("r9")).await.unwrap();
        repo.set_timezone("g5", "Europe/Berlin").await.unwrap();

        let mut expected = GuildSettings::defaults_for("g5");
        expected.creator_role_id = Some("r9".to_owned());
        expected.timezone = "Europe/Berlin".to_owned();
        assert_eq!(repo.get("g5").await.unwrap().unwrap(), expected);

        repo.set_creator_role("g5", None).await.unwrap();
        expected.creator_role_id = None;
        assert_eq!(repo.get("g5").await.unwrap().unwrap(), expected);

        // Unknown guilds surface as errors, not silent no-ops.
        assert!(repo.set_creator_role("nope", None).await.is_err());
        assert!(repo.set_timezone("nope", "UTC").await.is_err());
    }

    #[tokio::test]
    async fn a_panel_change_writes_only_what_changed() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);

        let mut s = GuildSettings::defaults_for("g6");
        s.setup_complete = true;
        s.watched_channels = vec!["c1".to_owned()];
        s.log_channel_id = Some("c-log".to_owned());
        s.creator_role_id = Some("r1".to_owned());
        s.timezone = "America/Chicago".to_owned();
        repo.upsert(&s).await.unwrap();

        // Nothing changed: nothing is written.
        repo.apply_panel_change("g6", &PanelChange::default())
            .await
            .unwrap();
        assert_eq!(repo.get("g6").await.unwrap().unwrap(), s);

        // Every column the panel owns, the ids cleared.
        let mut after = s.clone();
        after.timezone = "Asia/Tokyo".to_owned();
        after.creator_role_id = None;
        after.log_channel_id = None;
        after.max_series_per_user = 7;
        after.min_account_age_days = 30;
        after.min_membership_age_days = 14;
        after.sprout_enabled = !s.sprout_enabled;
        after.sprout_threshold = s.sprout_threshold + 2;
        let change = PanelChange::between(&s, &after);
        assert_eq!(change.creator_role_id, Some(None));
        repo.apply_panel_change("g6", &change).await.unwrap();
        assert_eq!(repo.get("g6").await.unwrap().unwrap(), after);

        // An id can be set again; the rest stays.
        let mut again = after.clone();
        again.creator_role_id = Some("r2".to_owned());
        let change = PanelChange::between(&after, &again);
        assert_eq!(change.timezone, None);
        repo.apply_panel_change("g6", &change).await.unwrap();
        assert_eq!(repo.get("g6").await.unwrap().unwrap(), again);

        assert!(
            repo.apply_panel_change("nope", &PanelChange::default())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn a_panel_change_does_not_undo_a_setup_saved_meanwhile() {
        let (_dir, pool) = test_pool().await;
        let repo = GuildSettingsRepo::new(pool);
        repo.ensure_exists("g7").await.unwrap();

        // The panel reads the row before `/setup` has been run...
        let read = repo.get("g7").await.unwrap().unwrap();
        assert!(!read.setup_complete);

        // ...`/setup` is saved in chat while the panel checks with Discord...
        repo.set_channels("g7", &["c1".to_owned()], Some("c-log"))
            .await
            .unwrap();
        repo.set_timezone("g7", "Europe/Paris").await.unwrap();

        // ...and the panel then saves a change to one number.
        let mut edited = read.clone();
        edited.max_series_per_user = 9;
        repo.apply_panel_change("g7", &PanelChange::between(&read, &edited))
            .await
            .unwrap();

        let stored = repo.get("g7").await.unwrap().unwrap();
        assert!(stored.setup_complete);
        assert_eq!(stored.watched_channels, vec!["c1".to_owned()]);
        assert_eq!(stored.log_channel_id.as_deref(), Some("c-log"));
        assert_eq!(stored.timezone, "Europe/Paris");
        assert_eq!(stored.max_series_per_user, 9);
    }
}
