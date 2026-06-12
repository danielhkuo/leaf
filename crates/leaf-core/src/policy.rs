//! Pure policy logic: who may create a series, and who may view one.
//!
//! Kept free of Discord types so every rule is table-testable; the bot
//! layer adapts interaction data into these inputs.

use crate::domain::{GuildSettings, Privacy, Series, SeriesState};

/// Facts about the would-be creator at `/series create` time.
#[derive(Debug, Clone)]
pub struct CreationContext {
    /// Current unix time.
    pub now_unix: i64,
    /// Creator's Discord account creation time (from the snowflake).
    pub account_created_unix: i64,
    /// When the creator joined the guild, if known.
    pub joined_unix: Option<i64>,
    /// Their live (non-revoked) series count in this guild.
    pub live_series_count: i64,
    /// Whether they hold the configured creator role; `None` when the
    /// guild has no role requirement.
    pub has_creator_role: Option<bool>,
}

/// Why creation was refused. `Display` is the user-facing message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyViolation {
    /// Too many live series already.
    #[error("you already have {0} series here — the limit is {0}. Edit or remove one first")]
    MaxSeries(i64),
    /// Discord account is younger than the policy requires.
    #[error("your Discord account must be at least {0} days old to start a series here")]
    AccountTooNew(i64),
    /// Guild membership is younger than the policy requires.
    #[error("you need to have been a member here for at least {0} days to start a series")]
    MembershipTooNew(i64),
    /// The guild requires a creator role the user lacks.
    #[error("starting a series here requires the creator role")]
    MissingCreatorRole,
    /// The chosen channel is not in the guild's watched list.
    #[error("that channel isn't one this server archives from — an admin can add it with /setup")]
    ChannelNotWatched,
}

const DAY_SECS: i64 = 86_400;

/// Checks every creation policy; first violation wins.
pub fn check_creation(
    settings: &GuildSettings,
    ctx: &CreationContext,
) -> Result<(), PolicyViolation> {
    if ctx.has_creator_role == Some(false) {
        return Err(PolicyViolation::MissingCreatorRole);
    }
    if ctx.live_series_count >= settings.max_series_per_user {
        return Err(PolicyViolation::MaxSeries(settings.max_series_per_user));
    }
    if settings.min_account_age_days > 0 {
        let age_days = (ctx.now_unix - ctx.account_created_unix) / DAY_SECS;
        if age_days < settings.min_account_age_days {
            return Err(PolicyViolation::AccountTooNew(
                settings.min_account_age_days,
            ));
        }
    }
    if settings.min_membership_age_days > 0 {
        let joined = ctx.joined_unix.unwrap_or(ctx.now_unix);
        if (ctx.now_unix - joined) / DAY_SECS < settings.min_membership_age_days {
            return Err(PolicyViolation::MembershipTooNew(
                settings.min_membership_age_days,
            ));
        }
    }
    Ok(())
}

/// True when `channel_id` is one the guild archives from.
#[must_use]
pub fn channel_allowed(settings: &GuildSettings, channel_id: &str) -> bool {
    settings.watched_channels.iter().any(|c| c == channel_id)
}

/// Who is asking to see a series.
#[derive(Debug, Clone, Copy)]
pub struct Viewer<'a> {
    /// Viewer's user snowflake.
    pub user_id: &'a str,
    /// Viewer's role snowflakes.
    pub role_ids: &'a [String],
    /// Holds Manage Guild.
    pub is_admin: bool,
}

/// Visibility rules: admins and creators always see their own; revoked is
/// admin-only; sprouts are hidden from everyone else; otherwise privacy
/// applies (public / role-gated / creator-only).
#[must_use]
pub fn can_view(series: &Series, viewer: &Viewer<'_>) -> bool {
    if viewer.is_admin {
        return true;
    }
    let is_creator = series.creator_id == viewer.user_id;
    match series.state {
        SeriesState::Revoked => false,
        SeriesState::Sprout => is_creator,
        SeriesState::Active => match series.privacy {
            Privacy::Public => true,
            Privacy::CreatorOnly => is_creator,
            Privacy::RoleGated => {
                is_creator
                    || series
                        .privacy_role_id
                        .as_ref()
                        .is_some_and(|r| viewer.role_ids.contains(r))
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Cadence, DetectionMode};

    fn settings() -> GuildSettings {
        let mut s = GuildSettings::defaults_for("g");
        s.watched_channels = vec!["c1".to_owned(), "c2".to_owned()];
        s.max_series_per_user = 2;
        s
    }

    fn ctx() -> CreationContext {
        CreationContext {
            now_unix: 1_000 * DAY_SECS,
            account_created_unix: 900 * DAY_SECS, // 100 days old
            joined_unix: Some(950 * DAY_SECS),    // 50 days a member
            live_series_count: 0,
            has_creator_role: None,
        }
    }

    #[test]
    fn defaults_allow_creation() {
        assert_eq!(check_creation(&settings(), &ctx()), Ok(()));
    }

    #[test]
    fn each_policy_blocks_individually() {
        let s = settings();

        let mut c = ctx();
        c.live_series_count = 2;
        assert_eq!(check_creation(&s, &c), Err(PolicyViolation::MaxSeries(2)));

        let mut s2 = s.clone();
        s2.min_account_age_days = 365;
        assert_eq!(
            check_creation(&s2, &ctx()),
            Err(PolicyViolation::AccountTooNew(365))
        );

        let mut s3 = s.clone();
        s3.min_membership_age_days = 60;
        assert_eq!(
            check_creation(&s3, &ctx()),
            Err(PolicyViolation::MembershipTooNew(60))
        );

        let mut c2 = ctx();
        c2.has_creator_role = Some(false);
        assert_eq!(
            check_creation(&s, &c2),
            Err(PolicyViolation::MissingCreatorRole)
        );
        let mut c3 = ctx();
        c3.has_creator_role = Some(true);
        assert_eq!(check_creation(&s, &c3), Ok(()));
    }

    #[test]
    fn unknown_join_date_passes_membership_check() {
        // Benefit of the doubt? No: unknown join counts as "joined now",
        // which FAILS a nonzero membership requirement (conservative).
        let mut s = settings();
        s.min_membership_age_days = 1;
        let mut c = ctx();
        c.joined_unix = None;
        assert_eq!(
            check_creation(&s, &c),
            Err(PolicyViolation::MembershipTooNew(1))
        );
    }

    #[test]
    fn channel_allowlist() {
        let s = settings();
        assert!(channel_allowed(&s, "c1"));
        assert!(!channel_allowed(&s, "elsewhere"));
    }

    fn series(state: SeriesState, privacy: Privacy, role: Option<&str>) -> Series {
        Series {
            id: 1,
            guild_id: "g".to_owned(),
            creator_id: "creator".to_owned(),
            name: "s".to_owned(),
            description: String::new(),
            channels: vec![],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy,
            privacy_role_id: role.map(str::to_owned),
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".to_owned(),
            state,
            created_at: 0,
        }
    }

    #[test]
    fn visibility_matrix() {
        let stranger = Viewer {
            user_id: "u",
            role_ids: &[],
            is_admin: false,
        };
        let creator = Viewer {
            user_id: "creator",
            role_ids: &[],
            is_admin: false,
        };
        let admin = Viewer {
            user_id: "u",
            role_ids: &[],
            is_admin: true,
        };
        let roled_ids = vec!["vip".to_owned()];
        let roled = Viewer {
            user_id: "u",
            role_ids: &roled_ids,
            is_admin: false,
        };

        let public = series(SeriesState::Active, Privacy::Public, None);
        assert!(can_view(&public, &stranger));

        let gated = series(SeriesState::Active, Privacy::RoleGated, Some("vip"));
        assert!(!can_view(&gated, &stranger));
        assert!(can_view(&gated, &roled));
        assert!(can_view(&gated, &creator));

        let private = series(SeriesState::Active, Privacy::CreatorOnly, None);
        assert!(!can_view(&private, &stranger));
        assert!(can_view(&private, &creator));
        assert!(can_view(&private, &admin));

        let sprout = series(SeriesState::Sprout, Privacy::Public, None);
        assert!(!can_view(&sprout, &stranger));
        assert!(can_view(&sprout, &creator));
        assert!(can_view(&sprout, &admin));

        let revoked = series(SeriesState::Revoked, Privacy::Public, None);
        assert!(!can_view(&revoked, &stranger));
        assert!(!can_view(&revoked, &creator));
        assert!(can_view(&revoked, &admin));
    }
}
