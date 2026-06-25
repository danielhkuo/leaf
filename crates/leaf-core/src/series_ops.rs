//! Shared series lifecycle operations.
//!
//! Validate-and-create, validate-and-edit, and ownership checks. The bot's
//! slash commands and the gallery's creator REST API both call these, so the
//! rules can't drift between surfaces.
//!
//! Validation is pure and table-testable; the `*_series` wrappers add the one
//! database call. Discord types never appear here — callers adapt interaction
//! or session data into the plain inputs below.

use crate::db::{DbError, SeriesRepo};
use crate::domain::{
    Cadence, DetectionMode, GuildSettings, NewSeries, Privacy, Series, SeriesState,
};
use crate::policy::{self, CreationContext, PolicyViolation};

/// Discord's epoch (2015-01-01) in milliseconds, for snowflake → time.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// Name length bounds (mirrors the old slash `min_length`/`max_length`).
const NAME_MIN: usize = 2;
const NAME_MAX: usize = 40;
const DESCRIPTION_MAX: usize = 200;
const EMOJI_MAX: usize = 8;

/// A field-level validation failure. `Display` is the user-facing message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// Name outside the 2–40 character range.
    #[error("a series name must be between 2 and 40 characters")]
    NameLength,
    /// Description longer than 200 characters.
    #[error("a description can be at most 200 characters")]
    DescriptionTooLong,
    /// Emoji longer than 8 characters.
    #[error("a reaction emoji can be at most 8 characters")]
    EmojiTooLong,
    /// Start day below 1.
    #[error("the start day must be 1 or greater")]
    StartDayTooLow,
    /// Role-gated privacy without a role.
    #[error("role-gated privacy needs a role to gate on")]
    MissingPrivacyRole,
    /// Reminder time not in 24h `HH:MM` form.
    #[error("`{0}` isn't a valid 24h time — use HH:MM, e.g. 17:30")]
    InvalidReminderTime(String),
    /// Reminder timezone not a known IANA name.
    #[error("`{0}` isn't a timezone I know — pick a valid IANA name")]
    InvalidTimezone(String),
    /// Enabling reminders without a time to fire at.
    #[error("to enable reminders, set a time (24h HH:MM)")]
    ReminderTimeRequired,
    /// Reminders on a series with no schedule.
    #[error("freeform series have no schedule to remind against — set a cadence first")]
    ReminderOnFreeform,
}

/// Why a series could not be created.
#[derive(Debug, thiserror::Error)]
pub enum CreateError {
    /// A creation-policy rule blocked it (role, limit, age, channel).
    #[error(transparent)]
    Policy(#[from] PolicyViolation),
    /// A field failed validation.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The name already exists in this guild.
    #[error("a series with this name already exists here")]
    NameTaken,
    /// A database error.
    #[error(transparent)]
    Db(DbError),
}

/// Why a series could not be updated.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// The caller does not own the series.
    #[error(transparent)]
    Forbidden(#[from] Forbidden),
    /// The target channel is not one the guild archives from.
    #[error(transparent)]
    Policy(#[from] PolicyViolation),
    /// A field failed validation.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The series is revoked and cannot be edited (admins restore it).
    #[error("this series has been revoked — an admin can restore it")]
    Revoked,
    /// The name already exists in this guild.
    #[error("a series with this name already exists here")]
    NameTaken,
    /// A database error.
    #[error(transparent)]
    Db(DbError),
}

/// The caller is not the series creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("only the series creator can do that")]
pub struct Forbidden;

/// Fields needed to create a series, adapted from the slash options or the
/// REST create request.
#[derive(Debug, Clone)]
pub struct CreateSeriesInput {
    /// Display name (validated 2–40 chars, unique per guild).
    pub name: String,
    /// Free-text description (validated ≤ 200 chars).
    pub description: String,
    /// The single watched channel to post in (v1 is single-channel).
    pub channel_id: String,
    /// Posting cadence.
    pub cadence: Cadence,
    /// Visibility.
    pub privacy: Privacy,
    /// Role for [`Privacy::RoleGated`]; required when role-gated.
    pub privacy_role_id: Option<String>,
    /// First day number (validated ≥ 1).
    pub start_day: i64,
}

/// Partial update; every field is optional and omitted ones are unchanged.
/// Mirrors the union of the old `/series edit` and `/series reminder`.
#[derive(Debug, Clone, Default)]
pub struct UpdateSeriesInput {
    /// New description (≤ 200 chars).
    pub description: Option<String>,
    /// New reaction emoji (≤ 8 chars).
    pub emoji: Option<String>,
    /// New cadence.
    pub cadence: Option<Cadence>,
    /// New visibility.
    pub privacy: Option<Privacy>,
    /// New role for role-gated privacy.
    pub privacy_role_id: Option<String>,
    /// Move to a different watched channel.
    pub channel_id: Option<String>,
    /// Capture mode (context menu vs passive).
    pub detection_mode: Option<DetectionMode>,
    /// Enable or disable reminders.
    pub reminder_enabled: Option<bool>,
    /// Reminder time of day, `HH:MM`.
    pub reminder_time: Option<String>,
    /// IANA timezone override for reminders.
    pub reminder_timezone: Option<String>,
    /// Remind by DM (true) or channel ping (false).
    pub reminder_dm: Option<bool>,
}

/// Builds the policy [`CreationContext`] from raw, Discord-free facts. The
/// `account_created_unix` typically comes from [`account_created_unix`].
#[must_use]
pub fn build_creation_context(
    now_unix: i64,
    account_created_unix: i64,
    joined_unix: Option<i64>,
    live_series_count: i64,
    role_ids: &[String],
    settings: &GuildSettings,
) -> CreationContext {
    CreationContext {
        now_unix,
        account_created_unix,
        joined_unix,
        live_series_count,
        has_creator_role: settings
            .creator_role_id
            .as_ref()
            .map(|role| role_ids.iter().any(|r| r == role)),
    }
}

/// Account creation time (unix seconds) derived from a Discord snowflake.
/// Returns `0` for an unparseable id (treated as a very old account).
#[must_use]
pub fn account_created_unix(user_id: &str) -> i64 {
    user_id.parse::<u64>().map_or(0, |snowflake| {
        let ms = i64::try_from(snowflake >> 22).unwrap_or(i64::MAX) + DISCORD_EPOCH_MS;
        ms / 1000
    })
}

/// Validates ownership for a mutating operation.
///
/// # Errors
/// [`Forbidden`] when `user_id` is not the series creator.
pub fn assert_owner(series: &Series, user_id: &str) -> Result<(), Forbidden> {
    if series.creator_id == user_id {
        Ok(())
    } else {
        Err(Forbidden)
    }
}

/// Validates a create request against policy and field rules, producing the
/// [`NewSeries`] to insert. Pure: no database access.
///
/// # Errors
/// [`CreateError::Validation`] for field problems, or [`CreateError::Policy`]
/// for channel/eligibility violations.
pub fn validate_create(
    settings: &GuildSettings,
    ctx: &CreationContext,
    input: &CreateSeriesInput,
) -> Result<NewSeries, CreateError> {
    let name = input.name.trim();
    if name.chars().count() < NAME_MIN || name.chars().count() > NAME_MAX {
        return Err(ValidationError::NameLength.into());
    }
    if input.description.chars().count() > DESCRIPTION_MAX {
        return Err(ValidationError::DescriptionTooLong.into());
    }
    if input.start_day < 1 {
        return Err(ValidationError::StartDayTooLow.into());
    }
    if input.privacy == Privacy::RoleGated && input.privacy_role_id.is_none() {
        return Err(ValidationError::MissingPrivacyRole.into());
    }
    if !policy::channel_allowed(settings, &input.channel_id) {
        return Err(PolicyViolation::ChannelNotWatched.into());
    }
    policy::check_creation(settings, ctx)?;

    let state = if settings.sprout_enabled {
        SeriesState::Sprout
    } else {
        SeriesState::Active
    };

    Ok(NewSeries {
        guild_id: settings.guild_id.clone(),
        creator_id: String::new(), // filled by the caller below
        name: name.to_owned(),
        description: input.description.clone(),
        channels: vec![input.channel_id.clone()],
        cadence: input.cadence,
        detection_mode: DetectionMode::ContextMenu,
        privacy: input.privacy,
        privacy_role_id: input.privacy_role_id.clone(),
        start_day: input.start_day,
        state,
    })
}

/// Validates and creates a series for `creator_id`.
///
/// # Errors
/// As [`validate_create`], plus [`CreateError::NameTaken`] or
/// [`CreateError::Db`] from the insert.
pub async fn create_series(
    repo: &SeriesRepo,
    settings: &GuildSettings,
    ctx: &CreationContext,
    creator_id: &str,
    input: &CreateSeriesInput,
    now_unix: i64,
) -> Result<Series, CreateError> {
    let mut new = validate_create(settings, ctx, input)?;
    new.creator_id = creator_id.to_owned();
    match repo.create(&new, now_unix).await {
        Ok(series) => Ok(series),
        Err(DbError::SeriesNameTaken) => Err(CreateError::NameTaken),
        Err(e) => Err(CreateError::Db(e)),
    }
}

/// Applies a partial update to `series` in place after validating it. Pure: no
/// database access. The series must already be owner-checked and non-revoked.
///
/// # Errors
/// [`UpdateError::Validation`] or [`UpdateError::Policy`] for bad fields.
pub fn apply_update(
    settings: &GuildSettings,
    series: &mut Series,
    input: &UpdateSeriesInput,
) -> Result<(), UpdateError> {
    if let Some(description) = &input.description {
        if description.chars().count() > DESCRIPTION_MAX {
            return Err(ValidationError::DescriptionTooLong.into());
        }
        series.description.clone_from(description);
    }
    if let Some(emoji) = &input.emoji {
        if emoji.chars().count() > EMOJI_MAX {
            return Err(ValidationError::EmojiTooLong.into());
        }
        series.emoji.clone_from(emoji);
    }
    if let Some(cadence) = input.cadence {
        series.cadence = cadence;
    }
    if let Some(privacy) = input.privacy {
        series.privacy = privacy;
    }
    if let Some(role) = &input.privacy_role_id {
        series.privacy_role_id = Some(role.clone());
    }
    if series.privacy == Privacy::RoleGated && series.privacy_role_id.is_none() {
        return Err(ValidationError::MissingPrivacyRole.into());
    }
    if let Some(channel_id) = &input.channel_id {
        if !policy::channel_allowed(settings, channel_id) {
            return Err(PolicyViolation::ChannelNotWatched.into());
        }
        series.channels = vec![channel_id.clone()];
    }
    if let Some(mode) = input.detection_mode {
        series.detection_mode = mode;
    }

    apply_reminder_update(series, input)?;
    Ok(())
}

/// Applies and validates the reminder portion of an update.
fn apply_reminder_update(
    series: &mut Series,
    input: &UpdateSeriesInput,
) -> Result<(), ValidationError> {
    if let Some(time) = &input.reminder_time {
        if !valid_hh_mm(time) {
            return Err(ValidationError::InvalidReminderTime(time.clone()));
        }
        series.reminder_time = Some(time.clone());
    }
    if let Some(tz) = &input.reminder_timezone {
        if tz.parse::<chrono_tz::Tz>().is_err() {
            return Err(ValidationError::InvalidTimezone(tz.clone()));
        }
        series.reminder_timezone = Some(tz.clone());
    }
    if let Some(dm) = input.reminder_dm {
        series.reminder_dm = dm;
    }
    if let Some(enabled) = input.reminder_enabled {
        if enabled {
            if series.cadence == Cadence::Freeform {
                return Err(ValidationError::ReminderOnFreeform);
            }
            if series.reminder_time.is_none() {
                return Err(ValidationError::ReminderTimeRequired);
            }
        }
        series.reminder_enabled = enabled;
    }
    Ok(())
}

/// Validates ownership and a partial update, then persists it.
///
/// # Errors
/// [`UpdateError::Forbidden`] if `user_id` is not the creator,
/// [`UpdateError::Revoked`] for a revoked series, validation/policy errors, or
/// [`UpdateError::Db`] / [`UpdateError::NameTaken`] from the write.
pub async fn update_series(
    repo: &SeriesRepo,
    settings: &GuildSettings,
    mut series: Series,
    user_id: &str,
    input: &UpdateSeriesInput,
) -> Result<Series, UpdateError> {
    assert_owner(&series, user_id)?;
    if series.state == SeriesState::Revoked {
        return Err(UpdateError::Revoked);
    }
    apply_update(settings, &mut series, input)?;
    match repo.update(&series).await {
        Ok(()) => Ok(series),
        Err(DbError::SeriesNameTaken) => Err(UpdateError::NameTaken),
        Err(e) => Err(UpdateError::Db(e)),
    }
}

/// True when `s` is a 24-hour `HH:MM` time.
#[must_use]
pub fn valid_hh_mm(s: &str) -> bool {
    chrono::NaiveTime::parse_from_str(s, "%H:%M").is_ok()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    fn settings() -> GuildSettings {
        let mut s = GuildSettings::defaults_for("g");
        s.watched_channels = vec!["c1".to_owned(), "c2".to_owned()];
        s.max_series_per_user = 2;
        s
    }

    fn ctx() -> CreationContext {
        CreationContext {
            now_unix: 1_000 * 86_400,
            account_created_unix: 900 * 86_400,
            joined_unix: Some(950 * 86_400),
            live_series_count: 0,
            has_creator_role: None,
        }
    }

    fn create_input() -> CreateSeriesInput {
        CreateSeriesInput {
            name: "Daily Johan".to_owned(),
            description: "one a day".to_owned(),
            channel_id: "c1".to_owned(),
            cadence: Cadence::Daily,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
        }
    }

    fn series() -> Series {
        Series {
            id: 1,
            guild_id: "g".to_owned(),
            creator_id: "creator".to_owned(),
            name: "s".to_owned(),
            description: String::new(),
            channels: vec!["c1".to_owned()],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".to_owned(),
            state: SeriesState::Active,
            created_at: 0,
        }
    }

    #[test]
    fn valid_create_produces_new_series() {
        let new = validate_create(&settings(), &ctx(), &create_input()).unwrap();
        assert_eq!(new.name, "Daily Johan");
        assert_eq!(new.channels, vec!["c1".to_owned()]);
        assert_eq!(new.state, SeriesState::Active);
    }

    #[test]
    fn sprout_guild_starts_series_as_sprout() {
        let mut s = settings();
        s.sprout_enabled = true;
        let new = validate_create(&s, &ctx(), &create_input()).unwrap();
        assert_eq!(new.state, SeriesState::Sprout);
    }

    #[test]
    fn short_name_is_rejected() {
        let mut input = create_input();
        input.name = "x".to_owned();
        assert!(matches!(
            validate_create(&settings(), &ctx(), &input),
            Err(CreateError::Validation(ValidationError::NameLength))
        ));
    }

    #[test]
    fn unwatched_channel_is_rejected() {
        let mut input = create_input();
        input.channel_id = "nope".to_owned();
        assert!(matches!(
            validate_create(&settings(), &ctx(), &input),
            Err(CreateError::Policy(PolicyViolation::ChannelNotWatched))
        ));
    }

    #[test]
    fn role_gated_without_role_is_rejected() {
        let mut input = create_input();
        input.privacy = Privacy::RoleGated;
        assert!(matches!(
            validate_create(&settings(), &ctx(), &input),
            Err(CreateError::Validation(ValidationError::MissingPrivacyRole))
        ));
    }

    #[test]
    fn policy_violation_propagates_from_create() {
        let mut c = ctx();
        c.live_series_count = 2;
        assert!(matches!(
            validate_create(&settings(), &c, &create_input()),
            Err(CreateError::Policy(PolicyViolation::MaxSeries(2)))
        ));
    }

    #[test]
    fn assert_owner_distinguishes_creator() {
        let s = series();
        assert!(assert_owner(&s, "creator").is_ok());
        assert_eq!(assert_owner(&s, "someone"), Err(Forbidden));
    }

    #[test]
    fn update_applies_fields() {
        let mut s = series();
        let input = UpdateSeriesInput {
            description: Some("new".to_owned()),
            emoji: Some("📸".to_owned()),
            privacy: Some(Privacy::CreatorOnly),
            channel_id: Some("c2".to_owned()),
            detection_mode: Some(DetectionMode::Passive),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.description, "new");
        assert_eq!(s.emoji, "📸");
        assert_eq!(s.privacy, Privacy::CreatorOnly);
        assert_eq!(s.channels, vec!["c2".to_owned()]);
        assert_eq!(s.detection_mode, DetectionMode::Passive);
    }

    #[test]
    fn update_to_unwatched_channel_is_rejected() {
        let mut s = series();
        let input = UpdateSeriesInput {
            channel_id: Some("elsewhere".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Policy(PolicyViolation::ChannelNotWatched))
        ));
    }

    #[test]
    fn enabling_reminder_on_freeform_is_rejected() {
        let mut s = series();
        s.cadence = Cadence::Freeform;
        let input = UpdateSeriesInput {
            reminder_enabled: Some(true),
            reminder_time: Some("17:30".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(ValidationError::ReminderOnFreeform))
        ));
    }

    #[test]
    fn enabling_reminder_without_time_is_rejected() {
        let mut s = series();
        let input = UpdateSeriesInput {
            reminder_enabled: Some(true),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(
                ValidationError::ReminderTimeRequired
            ))
        ));
    }

    #[test]
    fn invalid_reminder_time_is_rejected() {
        let mut s = series();
        let input = UpdateSeriesInput {
            reminder_time: Some("25:99".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(
                ValidationError::InvalidReminderTime(_)
            ))
        ));
    }

    #[test]
    fn enabling_reminder_with_time_succeeds() {
        let mut s = series();
        let input = UpdateSeriesInput {
            reminder_enabled: Some(true),
            reminder_time: Some("09:00".to_owned()),
            reminder_dm: Some(false),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert!(s.reminder_enabled);
        assert_eq!(s.reminder_time.as_deref(), Some("09:00"));
        assert!(!s.reminder_dm);
    }

    #[test]
    fn build_context_detects_creator_role() {
        let mut s = settings();
        s.creator_role_id = Some("role-1".to_owned());
        let has = build_creation_context(0, 0, None, 0, &["role-1".to_owned()], &s);
        assert_eq!(has.has_creator_role, Some(true));
        let lacks = build_creation_context(0, 0, None, 0, &["other".to_owned()], &s);
        assert_eq!(lacks.has_creator_role, Some(false));
    }

    #[test]
    fn account_age_from_snowflake() {
        // A known snowflake's embedded timestamp is after the Discord epoch.
        let created = account_created_unix("175928847299117063");
        assert!(created > DISCORD_EPOCH_MS / 1000);
        assert_eq!(account_created_unix("not-a-number"), 0);
    }
}
