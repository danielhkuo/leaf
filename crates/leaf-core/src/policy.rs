//! Pure policy logic: who may create a series, and who may view one.
//!
//! Kept free of Discord types so every rule is table-testable; the bot
//! layer adapts interaction data into these inputs.

use crate::domain::{GuildSettings, Privacy, Series, SeriesState};

/// Facts about the would-be creator at series-creation time.
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

/// Why creation was refused. `Display` is the user-facing message: a full
/// sentence that says what is in the way and what to do about it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyViolation {
    /// Too many live series already. Carries the guild's limit.
    #[error(
        "You've reached this server's limit of {0} series per member. \
         Ask a server admin if you need another."
    )]
    MaxSeries(i64),
    /// Discord account is younger than the policy requires (days).
    #[error("Your Discord account needs to be at least {} old to start a series here.", days(*.0))]
    AccountTooNew(i64),
    /// Guild membership is younger than the policy requires (days).
    #[error(
        "You need to have been a member of this server for at least {} to start a series.",
        days(*.0)
    )]
    MembershipTooNew(i64),
    /// The guild requires a creator role the user lacks.
    #[error("Starting a series here needs the creator role. Ask a server admin for it.")]
    MissingCreatorRole,
    /// The chosen channel is not in the guild's watched list.
    #[error(
        "That channel isn't one this server allows for series. \
         Pick another channel, or ask a server admin to add it with /setup."
    )]
    ChannelNotWatched,
}

impl PolicyViolation {
    /// Unix time at which this rule stops blocking `ctx`, for the two age
    /// rules ("you can start a series on 14 Oct"). `None` for rules that
    /// time alone will not lift, and for an unknown join date.
    #[must_use]
    pub fn eligible_at(&self, ctx: &CreationContext) -> Option<i64> {
        match *self {
            Self::AccountTooNew(required) => Some(
                ctx.account_created_unix
                    .saturating_add(required.saturating_mul(DAY_SECS)),
            ),
            Self::MembershipTooNew(required) => ctx
                .joined_unix
                .map(|joined| joined.saturating_add(required.saturating_mul(DAY_SECS))),
            Self::MaxSeries(_) | Self::MissingCreatorRole | Self::ChannelNotWatched => None,
        }
    }
}

const DAY_SECS: i64 = 86_400;

/// "1 day" / "30 days", for the age-rule messages.
fn days(n: i64) -> String {
    if n == 1 {
        "1 day".to_owned()
    } else {
        format!("{n} days")
    }
}

/// Checks every creation policy; first violation wins.
pub fn check_creation(
    settings: &GuildSettings,
    ctx: &CreationContext,
) -> Result<(), PolicyViolation> {
    creation_violations(settings, ctx)
        .into_iter()
        .next()
        .map_or(Ok(()), Err)
}

/// Every creation policy `ctx` currently fails; empty when creation is
/// allowed.
///
/// In the order [`check_creation`] reports them (role, limit, account age,
/// membership age), so a surface can explain all blockers at once instead of
/// revealing them one refusal at a time.
#[must_use]
pub fn creation_violations(
    settings: &GuildSettings,
    ctx: &CreationContext,
) -> Vec<PolicyViolation> {
    let mut violations = Vec::new();
    if ctx.has_creator_role == Some(false) {
        violations.push(PolicyViolation::MissingCreatorRole);
    }
    if ctx.live_series_count >= settings.max_series_per_user {
        violations.push(PolicyViolation::MaxSeries(settings.max_series_per_user));
    }
    if settings.min_account_age_days > 0 {
        let age_days = (ctx.now_unix - ctx.account_created_unix) / DAY_SECS;
        if age_days < settings.min_account_age_days {
            violations.push(PolicyViolation::AccountTooNew(
                settings.min_account_age_days,
            ));
        }
    }
    if settings.min_membership_age_days > 0 {
        let joined = ctx.joined_unix.unwrap_or(ctx.now_unix);
        if (ctx.now_unix - joined) / DAY_SECS < settings.min_membership_age_days {
            violations.push(PolicyViolation::MembershipTooNew(
                settings.min_membership_age_days,
            ));
        }
    }
    violations
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
    fn all_failing_policies_are_listed_in_check_order() {
        let mut s = settings();
        s.min_account_age_days = 365;
        s.min_membership_age_days = 60;
        let mut c = ctx();
        c.has_creator_role = Some(false);
        c.live_series_count = 5;
        let all = creation_violations(&s, &c);
        assert_eq!(
            all,
            vec![
                PolicyViolation::MissingCreatorRole,
                PolicyViolation::MaxSeries(2),
                PolicyViolation::AccountTooNew(365),
                PolicyViolation::MembershipTooNew(60),
            ]
        );
        // `check_creation` is the head of the same list.
        assert_eq!(check_creation(&s, &c).err(), all.first().cloned());
        assert!(creation_violations(&settings(), &ctx()).is_empty());
    }

    #[test]
    fn age_rules_report_when_they_lift() {
        let c = ctx();
        // Account created on day 900; a 365-day rule lifts on day 1265.
        assert_eq!(
            PolicyViolation::AccountTooNew(365).eligible_at(&c),
            Some(1_265 * DAY_SECS)
        );
        // Joined on day 950; a 60-day rule lifts on day 1010.
        assert_eq!(
            PolicyViolation::MembershipTooNew(60).eligible_at(&c),
            Some(1_010 * DAY_SECS)
        );
        // The date is exactly when the check starts passing.
        let mut s = settings();
        s.min_membership_age_days = 60;
        let mut later = ctx();
        later.now_unix = 1_010 * DAY_SECS - 1;
        assert!(check_creation(&s, &later).is_err());
        later.now_unix = 1_010 * DAY_SECS;
        assert_eq!(check_creation(&s, &later), Ok(()));

        let mut unknown_join = ctx();
        unknown_join.joined_unix = None;
        assert_eq!(
            PolicyViolation::MembershipTooNew(60).eligible_at(&unknown_join),
            None
        );
        for v in [
            PolicyViolation::MaxSeries(2),
            PolicyViolation::MissingCreatorRole,
            PolicyViolation::ChannelNotWatched,
        ] {
            assert_eq!(v.eligible_at(&c), None, "{v:?}");
        }
    }

    #[test]
    fn messages_are_actionable_sentences() {
        let all = [
            PolicyViolation::MaxSeries(3),
            PolicyViolation::AccountTooNew(30),
            PolicyViolation::MembershipTooNew(1),
            PolicyViolation::MissingCreatorRole,
            PolicyViolation::ChannelNotWatched,
        ];
        for v in &all {
            let text = v.to_string();
            assert!(text.starts_with(char::is_uppercase), "{text}");
            assert!(text.ends_with('.'), "{text}");
            // Creators cannot delete a series, so never tell them to.
            assert!(!text.to_lowercase().contains("remove"), "{text}");
        }
        assert_eq!(
            PolicyViolation::MaxSeries(3).to_string(),
            "You've reached this server's limit of 3 series per member. \
             Ask a server admin if you need another."
        );
        assert!(
            PolicyViolation::AccountTooNew(30)
                .to_string()
                .contains("at least 30 days old")
        );
        assert!(
            PolicyViolation::MembershipTooNew(1)
                .to_string()
                .contains("at least 1 day to")
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
