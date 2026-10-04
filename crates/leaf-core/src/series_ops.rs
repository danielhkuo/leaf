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
use crate::localtime;
use crate::parser::MAX_DAY;
use crate::policy::{self, CreationContext, PolicyViolation};

/// Discord's epoch (2015-01-01) in milliseconds, for snowflake → time.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

/// Name length bounds (mirrors the old slash `min_length`/`max_length`).
const NAME_MIN: usize = 2;
const NAME_MAX: usize = 40;
const DESCRIPTION_MAX: usize = 200;
/// Longest emoji sequence Unicode recommends for interchange (a two-person
/// kiss with two skin tones), in scalar values.
const EMOJI_MAX_SCALARS: usize = 10;
/// How much of a rejected value an error message may quote back.
const ECHO_MAX_CHARS: usize = 40;

/// A field-level validation failure. `Display` is the user-facing message.
///
/// The variant names predate some of the rules they now cover (the set is
/// matched exhaustively by the API layer, so it stays fixed); each doc
/// comment and message states the full rule. For the one rule that applies
/// to a given input, see [`name_problem`] and [`start_day_problem`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    /// Name outside the 2–40 character range, or containing something a bot
    /// message would turn into a ping (`@everyone`, `@here`, a user, role or
    /// channel mention) or a line break. See [`validate_name`].
    #[error(
        "A series name needs 2 to 40 characters on one line, \
         and can't contain @everyone, @here or a mention."
    )]
    NameLength,
    /// Description longer than 200 characters.
    #[error("A description can be at most 200 characters.")]
    DescriptionTooLong,
    /// Reaction emoji that is not exactly one emoji (empty, text, several
    /// emoji, or a custom server emoji). See [`validate_emoji`].
    #[error("The reaction needs to be a single standard emoji, such as 🍃.")]
    EmojiTooLong,
    /// Start day outside `1..=999999`, or above the earliest archived day.
    /// See [`validate_start_day`].
    #[error(
        "The first day number must be between 1 and 999999, \
         and no higher than the earliest day already archived."
    )]
    StartDayTooLow,
    /// Role-gated privacy without a role.
    #[error("Role-only visibility needs a role. Choose the role that may view this series.")]
    MissingPrivacyRole,
    /// Reminder time not in 24h `HH:MM` form.
    #[error("\"{0}\" isn't a time leaf can use. Enter it as 24-hour HH:MM, for example 17:30.")]
    InvalidReminderTime(String),
    /// Reminder timezone not a known IANA name.
    #[error(
        "\"{0}\" isn't a timezone leaf knows. Pick one from the list, for example America/Chicago."
    )]
    InvalidTimezone(String),
    /// Enabling reminders without a time to fire at.
    #[error("Choose a reminder time before turning reminders on.")]
    ReminderTimeRequired,
    /// Reminders on a series with no schedule.
    #[error(
        "Freeform series have no schedule to remind against. \
         Choose a daily, weekdays or weekly cadence first."
    )]
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
    #[error("A series with that name already exists in this server. Choose a different name.")]
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
    #[error("This series has been revoked, so it can't be edited. A server admin can restore it.")]
    Revoked,
    /// The name already exists in this guild.
    #[error("A series with that name already exists in this server. Choose a different name.")]
    NameTaken,
    /// A database error.
    #[error(transparent)]
    Db(DbError),
}

/// The caller is not the series creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("Only the creator of this series can do that.")]
pub struct Forbidden;

/// Fields needed to create a series, adapted from the slash options or the
/// REST create request.
#[derive(Debug, Clone)]
pub struct CreateSeriesInput {
    /// Display name (see [`validate_name`]; unique per guild).
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
    /// First day number (see [`validate_start_day`]).
    pub start_day: i64,
}

/// Partial update; every field is optional and omitted ones are unchanged.
/// Mirrors the union of the old `/series edit` and `/series reminder`.
///
/// A rename or a new start day is not part of this struct: validate with
/// [`validate_name`] / [`validate_start_day`], set the field on the
/// [`Series`], then pass it to [`update_series`].
#[derive(Debug, Clone, Default)]
pub struct UpdateSeriesInput {
    /// New description (≤ 200 chars).
    pub description: Option<String>,
    /// New reaction emoji (see [`validate_emoji`]).
    pub emoji: Option<String>,
    /// New cadence. Changing to freeform switches reminders off, unless the
    /// same update asks for them on, which is refused.
    pub cadence: Option<Cadence>,
    /// New visibility.
    pub privacy: Option<Privacy>,
    /// New role for role-gated privacy.
    pub privacy_role_id: Option<String>,
    /// Move to a different watched channel. Sending the current channel is
    /// a no-op, even if that channel is no longer watched.
    pub channel_id: Option<String>,
    /// Capture mode (context menu vs passive).
    pub detection_mode: Option<DetectionMode>,
    /// Enable or disable reminders.
    pub reminder_enabled: Option<bool>,
    /// Reminder time of day, 24-hour `HH:MM`. `Some("")` clears it, which is
    /// refused while reminders are on.
    pub reminder_time: Option<String>,
    /// IANA timezone override for reminders, matched case-insensitively.
    /// `Some("")` clears the override (back to the server's timezone).
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

/// Which rule a rejected series name broke. `Display` is a user-facing
/// sentence about that one rule.
///
/// [`ValidationError::NameLength`] covers all of these with one message
/// that lists every rule; use [`name_problem`] to tell the person the one
/// that applies to what they typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameProblem {
    /// Shorter than 2 or longer than 40 characters once trimmed.
    #[error("A series name needs 2 to 40 characters.")]
    Length,
    /// Contains `@everyone`, `@here`, `<@` (user and role mentions) or `<#`
    /// (channel mentions).
    #[error("A series name can't contain @everyone, @here or a mention.")]
    Mention,
    /// Contains a line break, a tab or another control character.
    #[error("A series name has to fit on one line.")]
    LineBreak,
}

/// The rule `raw` breaks as a series name, if any. Checked in the order
/// length, line break, mention.
#[must_use]
pub fn name_problem(raw: &str) -> Option<NameProblem> {
    let name = raw.trim();
    if !(NAME_MIN..=NAME_MAX).contains(&name.chars().count()) {
        return Some(NameProblem::Length);
    }
    if name.chars().any(char::is_control) {
        return Some(NameProblem::LineBreak);
    }
    let folded = name.to_lowercase();
    let pings = ["@everyone", "@here", "<@", "<#"]
        .iter()
        .any(|needle| folded.contains(needle));
    pings.then_some(NameProblem::Mention)
}

/// Validates a series name and returns it trimmed.
///
/// Names are echoed into public bot messages (log lines, milestones, channel
/// reminders), so besides the 2–40 character length they must not contain
/// anything Discord would turn into a ping, or a line break that would split
/// those messages. Markdown is allowed in a name; escape it with
/// [`display_name`] when putting the name into a message.
///
/// # Errors
/// [`ValidationError::NameLength`] for a name of the wrong length, or one
/// containing `@everyone`, `@here`, `<@` (user and role mentions), `<#`
/// (channel mentions) or a control character. [`name_problem`] says which.
pub fn validate_name(raw: &str) -> Result<&str, ValidationError> {
    match name_problem(raw) {
        None => Ok(raw.trim()),
        Some(_) => Err(ValidationError::NameLength),
    }
}

/// A series name made safe to put into Discord message content: markdown is
/// backslash-escaped so the name shows exactly as typed.
///
/// Names may contain markdown (`Johan's *art*`, `daily_sketch`), and bot
/// messages wrap them as `**{name}**`. Unescaped, a name with `**` or a
/// backtick breaks that bold, and `[text](https://…)` becomes a masked link.
/// This escapes the characters that start formatting, a mention or a masked
/// link (`\ * _ ~ | [ ] < >` and the backtick), breaks `://` so a bare URL
/// does not turn into a link, escapes a leading `#`, `-` or `1.` (heading
/// and list markers at the start of a line), and turns line breaks into
/// spaces.
///
/// Use it only where Discord renders markdown (message content, embed
/// descriptions and field values). Button labels, select options and
/// autocomplete choices show text literally and would show the backslashes.
#[must_use]
pub fn display_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 8);
    // True while only digits have been seen: the "1" of a "1." list item.
    let mut leading_digits = true;
    for (at, c) in name.char_indices() {
        let first = at == 0;
        let escape = match c {
            '\\' | '*' | '_' | '~' | '`' | '|' | '[' | ']' | '<' | '>' => true,
            '#' | '-' => first,
            '.' => leading_digits && !first,
            _ => false,
        };
        leading_digits = leading_digits && c.is_ascii_digit();
        if escape {
            out.push('\\');
        }
        out.push(if c.is_control() { ' ' } else { c });
        // "https://" → "https:\//", which Discord no longer reads as a URL.
        let rest = name.get(at + c.len_utf8()..).unwrap_or_default();
        if c == ':' && rest.starts_with("//") {
            out.push('\\');
        }
    }
    out
}

/// Which bound a rejected first day number broke. `Display` is a
/// user-facing sentence about that one bound.
///
/// [`ValidationError::StartDayTooLow`] covers both with one message; use
/// [`start_day_problem`] for the one that applies (on create there is no
/// archived day to mention).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StartDayProblem {
    /// Outside `1..=999999`.
    #[error("The first day number must be between 1 and 999999.")]
    OutOfRange,
    /// Above the earliest archived day, which is the payload.
    #[error(
        "The first day number can't be higher than Day {0}, \
         the earliest day already archived."
    )]
    AfterEarliestPost(i64),
}

/// The bound `start_day` breaks, if any. `min_archived_day` is as for
/// [`validate_start_day`].
#[must_use]
pub fn start_day_problem(start_day: i64, min_archived_day: Option<i64>) -> Option<StartDayProblem> {
    if !(1..=MAX_DAY).contains(&start_day) {
        return Some(StartDayProblem::OutOfRange);
    }
    min_archived_day
        .filter(|min| start_day > *min)
        .map(StartDayProblem::AfterEarliestPost)
}

/// Validates a first day number: `1..=999999`.
///
/// Once the series has posts it may also be no higher than the earliest
/// archived day (`min_archived_day`, from `PostRepo::min_day`), so that no
/// existing day ends up before the start.
///
/// # Errors
/// [`ValidationError::StartDayTooLow`] when out of range.
/// [`start_day_problem`] says which bound.
pub fn validate_start_day(
    start_day: i64,
    min_archived_day: Option<i64>,
) -> Result<(), ValidationError> {
    match start_day_problem(start_day, min_archived_day) {
        None => Ok(()),
        Some(_) => Err(ValidationError::StartDayTooLow),
    }
}

/// Validates a reaction emoji and returns it trimmed.
///
/// The value must be exactly one standard emoji, because the bot reacts with
/// the whole string: a plain pictograph, a keycap, a flag, a skin-tone
/// variant or a joined sequence such as "woman astronaut".
///
/// leaf has no copy of the Unicode emoji tables, so this checks the *shape*
/// of an emoji sequence over the code-point blocks emoji live in. A few
/// symbols Discord does not treat as emoji get through, so whoever reacts
/// must still handle Discord's "unknown emoji" error.
///
/// # Errors
/// [`ValidationError::EmojiTooLong`] for an empty value, text, more than one
/// emoji, or a custom server emoji (`<:name:id>`).
pub fn validate_emoji(raw: &str) -> Result<&str, ValidationError> {
    let emoji = raw.trim();
    if emoji.chars().count() <= EMOJI_MAX_SCALARS && is_single_emoji(emoji) {
        Ok(emoji)
    } else {
        Err(ValidationError::EmojiTooLong)
    }
}

const ZERO_WIDTH_JOINER: char = '\u{200D}';
/// Variation selector 16: "render the previous character as an emoji".
const EMOJI_PRESENTATION: char = '\u{FE0F}';
const COMBINING_KEYCAP: char = '\u{20E3}';
/// 🏴, the base of the subdivision flags (England, Scotland, Wales).
const BLACK_FLAG: char = '\u{1F3F4}';
const CANCEL_TAG: char = '\u{E007F}';

const fn is_regional_indicator(c: char) -> bool {
    matches!(c, '\u{1F1E6}'..='\u{1F1FF}')
}

const fn is_skin_tone(c: char) -> bool {
    matches!(c, '\u{1F3FB}'..='\u{1F3FF}')
}

/// Tag letters and digits that spell a subdivision after [`BLACK_FLAG`].
const fn is_tag(c: char) -> bool {
    matches!(c, '\u{E0030}'..='\u{E0039}' | '\u{E0061}'..='\u{E007A}')
}

/// A character that can be an emoji on its own: a superset of Unicode's
/// `Extended_Pictographic`, by block. Regional indicators and skin tones are
/// excluded because they only mean something in sequence.
const fn is_pictograph(c: char) -> bool {
    if is_regional_indicator(c) || is_skin_tone(c) {
        return false;
    }
    matches!(c,
        '\u{00A9}' | '\u{00AE}' | '\u{203C}' | '\u{2049}' | '\u{2122}' | '\u{2139}'
        | '\u{2194}'..='\u{21AA}'
        | '\u{2300}'..='\u{23FF}'
        | '\u{24C2}'
        | '\u{25A0}'..='\u{25FF}'
        | '\u{2600}'..='\u{27BF}'
        | '\u{2934}' | '\u{2935}'
        | '\u{2B00}'..='\u{2B55}'
        | '\u{3030}' | '\u{303D}' | '\u{3297}' | '\u{3299}'
        | '\u{1F000}'..='\u{1FAFF}'
    )
}

/// True when `s` is shaped like exactly one emoji: a keycap, a flag, a
/// subdivision flag, or one or more pictographs (each optionally followed by
/// a skin tone and/or the emoji-presentation selector) joined by ZWJ.
fn is_single_emoji(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let Some(first) = chars.next() else {
        return false;
    };

    if matches!(first, '0'..='9' | '#' | '*') {
        chars.next_if_eq(&EMOJI_PRESENTATION);
        return chars.next() == Some(COMBINING_KEYCAP) && chars.next().is_none();
    }
    if is_regional_indicator(first) {
        return chars.next().is_some_and(is_regional_indicator) && chars.next().is_none();
    }
    if !is_pictograph(first) {
        return false;
    }
    if first == BLACK_FLAG && chars.peek().copied().is_some_and(is_tag) {
        while chars.next_if(|c| is_tag(*c)).is_some() {}
        return chars.next() == Some(CANCEL_TAG) && chars.next().is_none();
    }
    loop {
        chars.next_if(|c| is_skin_tone(*c));
        chars.next_if_eq(&EMOJI_PRESENTATION);
        match chars.next() {
            None => return true,
            Some(ZERO_WIDTH_JOINER) => {
                if !chars.next().is_some_and(is_pictograph) {
                    return false;
                }
            }
            Some(_) => return false,
        }
    }
}

/// The canonical IANA name for `name`, or `None` when it is not a timezone
/// leaf knows.
///
/// Matched ignoring ASCII case and surrounding whitespace
/// (`america/chicago` → `America/Chicago`). Use this to validate and
/// normalise a timezone before storing it.
#[must_use]
pub fn canonical_timezone(name: &str) -> Option<&'static str> {
    localtime::parse_tz(name).map(localtime::Tz::name)
}

/// Result of [`match_series_name`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameMatch<'a> {
    /// Exactly one series fits.
    Found(&'a Series),
    /// Several series fit equally well; these are the candidates, in the
    /// order they were given.
    Ambiguous(Vec<&'a Series>),
    /// Nothing fits.
    NotFound,
}

impl<'a> NameMatch<'a> {
    /// The series, when the match was unique.
    #[must_use]
    pub const fn found(&self) -> Option<&'a Series> {
        match self {
            Self::Found(series) => Some(series),
            Self::Ambiguous(_) | Self::NotFound => None,
        }
    }
}

/// How series names compare when matching typed input: trimmed and
/// lower-cased with full Unicode rules, so `ÉTÉ` finds `Été` and `ЕЖЕ` finds
/// `Еже`. Also the key to filter autocomplete suggestions by.
#[must_use]
pub fn fold_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// Resolves a typed series name against `series`, the forgiving way a phone
/// keyboard needs.
///
/// Tries the exact name, then a case-insensitive match, then a prefix, then
/// a substring. The first of those steps with any hit decides: one hit is
/// the answer, several are ambiguous. So a name that merely contains the
/// input never shadows one that equals it.
///
/// Pass only the series the caller may see (or owns): candidates are
/// returned for [`NameMatch::Ambiguous`] and would otherwise leak names.
#[must_use]
pub fn match_series_name<'a>(
    series: impl IntoIterator<Item = &'a Series>,
    input: &str,
) -> NameMatch<'a> {
    let wanted = input.trim();
    if wanted.is_empty() {
        return NameMatch::NotFound;
    }
    let folded: Vec<(&Series, String)> = series
        .into_iter()
        .map(|s| (s, fold_name(&s.name)))
        .collect();
    if let Some((exact, _)) = folded.iter().find(|(s, _)| s.name == wanted) {
        return NameMatch::Found(exact);
    }

    let key = fold_name(wanted);
    let steps: [fn(&str, &str) -> bool; 3] = [
        |name, key| name == key,
        |name, key| name.starts_with(key),
        |name, key| name.contains(key),
    ];
    for fits in steps {
        let hits: Vec<&Series> = folded
            .iter()
            .filter(|(_, name)| fits(name, &key))
            .map(|(s, _)| *s)
            .collect();
        match hits.as_slice() {
            [] => {}
            [one] => return NameMatch::Found(one),
            _ => return NameMatch::Ambiguous(hits),
        }
    }
    NameMatch::NotFound
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
    let name = validate_name(&input.name)?;
    if input.description.chars().count() > DESCRIPTION_MAX {
        return Err(ValidationError::DescriptionTooLong.into());
    }
    validate_start_day(input.start_day, None)?;
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
/// A field sent with the value the series already has is left alone and not
/// re-validated, so a form that re-sends everything cannot be blocked by a
/// stored value that would no longer pass (a channel since dropped from the
/// watched list, an emoji saved before validation existed).
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
    if let Some(emoji) = &input.emoji
        && *emoji != series.emoji
    {
        validate_emoji(emoji)?.clone_into(&mut series.emoji);
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
    if let Some(channel_id) = &input.channel_id
        && series.channels.first() != Some(channel_id)
    {
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
        // A blank time field means "no time", not a malformed one: it is
        // only a problem if reminders end up switched on (checked below).
        series.reminder_time = if time.trim().is_empty() {
            None
        } else {
            let parsed = chrono::NaiveTime::parse_from_str(time.trim(), "%H:%M")
                .map_err(|_| ValidationError::InvalidReminderTime(echo(time)))?;
            // Stored zero-padded ("9:05" → "09:05") so a time picker can
            // show it again.
            Some(parsed.format("%H:%M").to_string())
        };
    }
    if let Some(tz) = &input.reminder_timezone {
        // An explicit empty value means "no override": use the server's zone.
        series.reminder_timezone = if tz.trim().is_empty() {
            None
        } else {
            let canonical =
                canonical_timezone(tz).ok_or_else(|| ValidationError::InvalidTimezone(echo(tz)))?;
            Some(canonical.to_owned())
        };
    }
    if let Some(dm) = input.reminder_dm {
        series.reminder_dm = dm;
    }
    if let Some(enabled) = input.reminder_enabled {
        series.reminder_enabled = enabled;
    }
    // Freeform has no schedule, so its reminders can never be on. Asking
    // for them is an error; a series that merely still had them on (its
    // cadence just changed to freeform, or an older row) has them switched
    // off, so settings never show a reminder that will not fire.
    if series.cadence == Cadence::Freeform && series.reminder_enabled {
        if input.reminder_enabled == Some(true) {
            return Err(ValidationError::ReminderOnFreeform);
        }
        series.reminder_enabled = false;
    }
    let touched = input.reminder_enabled.is_some() || input.reminder_time.is_some();
    if touched && series.reminder_enabled && series.reminder_time.is_none() {
        return Err(ValidationError::ReminderTimeRequired);
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

/// A rejected value as an error message may quote it: trimmed and cut to
/// [`ECHO_MAX_CHARS`], so a long paste cannot bloat the reply.
fn echo(value: &str) -> String {
    let value = value.trim();
    if value.chars().count() <= ECHO_MAX_CHARS {
        value.to_owned()
    } else {
        let head: String = value.chars().take(ECHO_MAX_CHARS).collect();
        format!("{head}…")
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
    fn names_are_trimmed_and_length_checked_in_characters() {
        assert_eq!(validate_name("  Daily Johan  "), Ok("Daily Johan"));
        assert_eq!(validate_name("ab"), Ok("ab"));
        assert_eq!(validate_name("日記"), Ok("日記"));
        assert_eq!(validate_name("Ежедневник"), Ok("Ежедневник"));
        let forty = "é".repeat(40);
        assert_eq!(validate_name(&forty), Ok(forty.as_str()));
        for bad in ["", " x ", &"é".repeat(41)] {
            assert_eq!(
                validate_name(bad),
                Err(ValidationError::NameLength),
                "{bad}"
            );
        }
    }

    #[test]
    fn names_that_would_ping_or_break_lines_are_rejected() {
        for bad in [
            "@everyone",
            "hi @here friends",
            "@EVERYONE look",
            "<@123456789>",
            "<@!123456789> art",
            "art by <@&987654321>",
            "see <#555>",
            "two\nlines",
            "tab\there",
        ] {
            assert_eq!(
                validate_name(bad),
                Err(ValidationError::NameLength),
                "{bad:?}"
            );
        }
        // Ordinary punctuation, including a lone @ or <, is fine.
        for ok in [
            "cats @ home",
            "I <3 ink",
            "#inktober",
            "daily_sketch",
            "Johan's *art*",
        ] {
            assert_eq!(validate_name(ok), Ok(ok), "{ok}");
        }
        let mut input = create_input();
        input.name = "@everyone daily".to_owned();
        assert!(matches!(
            validate_create(&settings(), &ctx(), &input),
            Err(CreateError::Validation(ValidationError::NameLength))
        ));
    }

    #[test]
    fn name_problem_names_the_one_rule_that_failed() {
        for (raw, problem) in [
            ("x", NameProblem::Length),
            ("   ", NameProblem::Length),
            (&*"é".repeat(41), NameProblem::Length),
            ("@everyone art", NameProblem::Mention),
            ("art by <@&987654321>", NameProblem::Mention),
            ("see <#555>", NameProblem::Mention),
            ("two\nlines", NameProblem::LineBreak),
            ("tab\there", NameProblem::LineBreak),
            // Length is reported first: it is the rule people hit most.
            (&*"@everyone ".repeat(5), NameProblem::Length),
        ] {
            assert_eq!(name_problem(raw), Some(problem), "{raw:?}");
            assert_eq!(validate_name(raw), Err(ValidationError::NameLength));
        }
        for ok in ["Daily Johan", "  ab  ", "[link](https://x.co)"] {
            assert_eq!(name_problem(ok), None, "{ok}");
        }
        // Each sentence is about its own rule only.
        assert!(!NameProblem::Length.to_string().contains('@'));
        assert!(!NameProblem::Mention.to_string().contains("40"));
    }

    #[test]
    fn display_name_escapes_markdown_and_links() {
        for (name, shown) in [
            ("Daily Johan", "Daily Johan"),
            ("Ежедневник (2026)", "Ежедневник (2026)"),
            ("Johan's *art*", r"Johan's \*art\*"),
            ("daily_sketch", r"daily\_sketch"),
            ("**bold** `code`", r"\*\*bold\*\* \`code\`"),
            ("~~gone~~ ||spoiler||", r"\~\~gone\~\~ \|\|spoiler\|\|"),
            ("back\\slash", r"back\\slash"),
            // A masked link shows as the text that was typed.
            (
                "[free nitro](https://x.co)",
                r"\[free nitro\](https:\//x.co)",
            ),
            ("see http://x.co", r"see http:\//x.co"),
            ("Day 1: sketches", "Day 1: sketches"),
            // Older names may still carry a mention or a line break.
            ("<@123> art", r"\<@123\> art"),
            ("two\nlines", "two lines"),
            // Markers that only mean something at the start of a line.
            ("#inktober", r"\#inktober"),
            ("ink #2", "ink #2"),
            ("- sketches", r"\- sketches"),
            ("day-by-day", "day-by-day"),
            ("> quotes", r"\> quotes"),
            ("1. Sketches", r"1\. Sketches"),
            ("12.5 minutes", r"12\.5 minutes"),
            ("Vol. 2", "Vol. 2"),
            (".hidden", ".hidden"),
            ("", ""),
        ] {
            assert_eq!(display_name(name), shown, "{name:?}");
        }
        // The bold wrapper survives anything a name can end with.
        assert_eq!(format!("**{}**", display_name(r"art\")), r"**art\\**");
    }

    #[test]
    fn start_day_problem_names_the_bound_that_failed() {
        for bad in [0, -3, MAX_DAY + 1, i64::MAX] {
            assert_eq!(
                start_day_problem(bad, None),
                Some(StartDayProblem::OutOfRange),
                "{bad}"
            );
            // Out of range wins over the archived-day bound.
            assert_eq!(
                start_day_problem(bad, Some(40)),
                Some(StartDayProblem::OutOfRange),
                "{bad}"
            );
        }
        assert_eq!(start_day_problem(1, None), None);
        assert_eq!(start_day_problem(40, Some(40)), None);
        assert_eq!(
            start_day_problem(41, Some(40)),
            Some(StartDayProblem::AfterEarliestPost(40))
        );
        // On create there is nothing archived, and the copy does not say so.
        assert!(!StartDayProblem::OutOfRange.to_string().contains("archived"));
        assert!(
            StartDayProblem::AfterEarliestPost(40)
                .to_string()
                .contains("Day 40")
        );
        // Agrees with validate_start_day on every case above.
        for (day, min) in [(0, None), (1, None), (41, Some(40)), (40, Some(40))] {
            assert_eq!(
                validate_start_day(day, min).is_ok(),
                start_day_problem(day, min).is_none(),
                "{day} {min:?}"
            );
        }
    }

    #[test]
    fn start_day_is_bounded_by_range_and_earliest_archived_day() {
        for ok in [1, 200, MAX_DAY] {
            assert_eq!(validate_start_day(ok, None), Ok(()), "{ok}");
        }
        for bad in [0, -3, MAX_DAY + 1, i64::MAX] {
            assert_eq!(
                validate_start_day(bad, None),
                Err(ValidationError::StartDayTooLow),
                "{bad}"
            );
        }
        // Earliest archived day is 40: the start may be 40 or lower.
        assert_eq!(validate_start_day(40, Some(40)), Ok(()));
        assert_eq!(validate_start_day(1, Some(40)), Ok(()));
        assert_eq!(
            validate_start_day(41, Some(40)),
            Err(ValidationError::StartDayTooLow)
        );

        let mut input = create_input();
        input.start_day = 0;
        assert!(matches!(
            validate_create(&settings(), &ctx(), &input),
            Err(CreateError::Validation(ValidationError::StartDayTooLow))
        ));
    }

    #[test]
    fn single_emoji_of_every_shape_is_accepted() {
        for ok in [
            "🍃",
            " 📸 ",
            "\u{2764}",           // heart, text form
            "\u{2764}\u{FE0F}",   // heart, emoji form
            "\u{A9}\u{FE0F}",     // copyright
            "1\u{FE0F}\u{20E3}",  // keycap 1
            "#\u{20E3}",          // keycap #, no selector
            "*\u{FE0F}\u{20E3}",  // keycap *
            "\u{1F1EF}\u{1F1F5}", // flag: Japan
            "\u{1F3F4}\u{E0067}\u{E0062}\u{E0073}\u{E0063}\u{E0074}\u{E007F}", // flag: Scotland
            "\u{1F44D}\u{1F3FD}", // thumbs up, skin tone
            "\u{261D}\u{1F3FB}",  // index up, skin tone
            "\u{1F469}\u{200D}\u{1F680}", // woman astronaut
            "\u{1F9D1}\u{1F3FD}\u{200D}\u{1F3A8}", // artist, skin tone
            "\u{1F3F3}\u{FE0F}\u{200D}\u{1F308}", // rainbow flag
            "\u{1F3F4}\u{200D}\u{2620}\u{FE0F}", // pirate flag
            "\u{2764}\u{FE0F}\u{200D}\u{1F525}", // heart on fire
            "\u{1F575}\u{FE0F}\u{200D}\u{2642}\u{FE0F}", // man detective
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}", // family
            // woman running right, skin tone (8 scalars)
            "\u{1F3C3}\u{1F3FD}\u{200D}\u{2640}\u{FE0F}\u{200D}\u{27A1}\u{FE0F}",
            // kiss with two skin tones: the longest standard emoji (10 scalars)
            "\u{1F469}\u{1F3FD}\u{200D}\u{2764}\u{FE0F}\u{200D}\u{1F48B}\u{200D}\u{1F468}\u{1F3FE}",
        ] {
            assert_eq!(validate_emoji(ok), Ok(ok.trim()), "{ok:?}");
        }
    }

    #[test]
    fn text_and_multiple_emoji_are_rejected() {
        for bad in [
            "",
            "   ",
            "hello",
            "a",
            "é",
            "7",
            "#",
            "1\u{FE0F}", // keycap base without the keycap
            "\u{FE0F}",  // selector alone
            "🍃🍃",
            "🍃 📸",
            "🍃x",
            "x🍃",
            "\u{1F1EF}",                            // half a flag
            "\u{1F1EF}\u{1F1F5}\u{1F1EB}\u{1F1F7}", // two flags
            "\u{1F3FD}",                            // skin tone alone
            "🍃\u{200D}",                           // dangling joiner
            "\u{200D}🍃",
            "\u{1F3F4}\u{E0067}\u{E0062}", // unterminated subdivision flag
            "<:leaf:123456789012345678>",  // custom server emoji
            // two families joined: valid shape, but no such emoji
            "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}\u{200D}\u{1F466}\u{200D}\u{1F468}\u{200D}\u{1F469}",
        ] {
            assert_eq!(
                validate_emoji(bad),
                Err(ValidationError::EmojiTooLong),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn timezones_resolve_to_canonical_names() {
        assert_eq!(
            canonical_timezone("America/Chicago"),
            Some("America/Chicago")
        );
        assert_eq!(
            canonical_timezone(" america/chicago "),
            Some("America/Chicago")
        );
        assert_eq!(canonical_timezone("UTC"), Some("UTC"));
        assert_eq!(canonical_timezone("europe/LONDON"), Some("Europe/London"));
        for bad in ["", "CST", "Chicago", "America/Nowhere"] {
            assert_eq!(canonical_timezone(bad), None, "{bad}");
        }
    }

    fn named(id: i64, name: &str) -> Series {
        let mut s = series();
        s.id = id;
        name.clone_into(&mut s.name);
        s
    }

    #[test]
    fn series_names_match_forgivingly() {
        let all = [
            named(1, "Daily Sketch"),
            named(2, "Daily Ink"),
            named(3, "Photos"),
            named(4, "Ежедневник"),
            named(5, "Été"),
            named(6, "Ink"),
        ];
        let id = |input: &str| match_series_name(&all, input).found().map(|s| s.id);

        assert_eq!(id("Daily Sketch"), Some(1)); // exact
        assert_eq!(id("  Photos  "), Some(3)); // trimmed
        assert_eq!(id("DAILY INK"), Some(2)); // case-insensitive
        assert_eq!(id("pho"), Some(3)); // unique prefix
        assert_eq!(id("sketch"), Some(1)); // unique substring
        assert_eq!(id("ежедневник"), Some(4)); // Cyrillic lower-case
        assert_eq!(id("ЕЖЕДНЕВНИК"), Some(4)); // Cyrillic upper-case
        assert_eq!(id("été"), Some(5)); // accented
        assert_eq!(id("ÉTÉ"), Some(5));
        // A whole-name match beats names that merely contain it.
        assert_eq!(id("ink"), Some(6));

        assert_eq!(match_series_name(&all, "nope"), NameMatch::NotFound);
        assert_eq!(match_series_name(&all, "   "), NameMatch::NotFound);
        assert_eq!(match_series_name(&[], "Photos"), NameMatch::NotFound);
    }

    #[test]
    fn ambiguous_names_list_their_candidates() {
        let all = [
            named(1, "Daily Sketch"),
            named(2, "Daily Ink"),
            named(3, "Photos"),
            named(4, "art"),
            named(5, "Art"),
        ];
        let ids = |input: &str| match match_series_name(&all, input) {
            NameMatch::Ambiguous(hits) => hits.iter().map(|s| s.id).collect(),
            NameMatch::Found(_) | NameMatch::NotFound => Vec::new(),
        };
        assert_eq!(ids("daily"), vec![1, 2]); // shared prefix
        assert_eq!(ids("ai"), vec![1, 2]); // shared substring
        assert_eq!(ids("ART"), vec![4, 5]); // differ only by case
        // An exact, case-sensitive name is never ambiguous.
        assert_eq!(
            match_series_name(&all, "art").found().map(|s| s.id),
            Some(4)
        );
        assert_eq!(
            match_series_name(&all, "Art").found().map(|s| s.id),
            Some(5)
        );
        // Works over a filtered view, the way callers pass viewable series.
        let visible = all.iter().filter(|s| s.id != 2);
        assert_eq!(
            match_series_name(visible, "daily").found().map(|s| s.id),
            Some(1)
        );
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
    fn resending_the_current_channel_skips_the_watched_check() {
        // The series lives in a channel an admin has since dropped from the
        // watched list; a form that re-sends every field must still save.
        let mut s = series();
        s.channels = vec!["dropped".to_owned()];
        let input = UpdateSeriesInput {
            description: Some("new".to_owned()),
            channel_id: Some("dropped".to_owned()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.description, "new");
        assert_eq!(s.channels, vec!["dropped".to_owned()]);
        // Moving to another unwatched channel is still refused...
        let input = UpdateSeriesInput {
            channel_id: Some("elsewhere".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Policy(PolicyViolation::ChannelNotWatched))
        ));
        // ...and moving to a watched one works.
        let input = UpdateSeriesInput {
            channel_id: Some("c2".to_owned()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.channels, vec!["c2".to_owned()]);
    }

    #[test]
    fn emoji_is_validated_only_when_it_changes() {
        let mut s = series();
        let input = UpdateSeriesInput {
            emoji: Some("hello".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(ValidationError::EmojiTooLong))
        ));
        assert_eq!(s.emoji, "🍃");

        // A value saved before validation existed does not block other edits.
        s.emoji = "hello".to_owned();
        let input = UpdateSeriesInput {
            emoji: Some("hello".to_owned()),
            description: Some("new".to_owned()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.description, "new");

        let input = UpdateSeriesInput {
            emoji: Some(" 🇯🇵 ".to_owned()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.emoji, "🇯🇵");
    }

    #[test]
    fn reminder_timezone_is_canonicalised_and_can_be_cleared() {
        let mut s = series();
        let input = UpdateSeriesInput {
            reminder_timezone: Some("america/chicago".to_owned()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.reminder_timezone.as_deref(), Some("America/Chicago"));

        // Omitted: unchanged.
        apply_update(&settings(), &mut s, &UpdateSeriesInput::default()).unwrap();
        assert_eq!(s.reminder_timezone.as_deref(), Some("America/Chicago"));

        // Explicitly empty: back to the server's timezone.
        for blank in ["", "  "] {
            s.reminder_timezone = Some("America/Chicago".to_owned());
            let input = UpdateSeriesInput {
                reminder_timezone: Some(blank.to_owned()),
                ..Default::default()
            };
            apply_update(&settings(), &mut s, &input).unwrap();
            assert_eq!(s.reminder_timezone, None, "{blank:?}");
        }
    }

    #[test]
    fn unknown_timezone_is_rejected_with_a_short_echo() {
        let mut s = series();
        s.reminder_timezone = Some("Europe/Paris".to_owned());
        let input = UpdateSeriesInput {
            reminder_timezone: Some("CST".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(ValidationError::InvalidTimezone(tz))) if tz == "CST"
        ));
        assert_eq!(s.reminder_timezone.as_deref(), Some("Europe/Paris"));

        let input = UpdateSeriesInput {
            reminder_timezone: Some("z".repeat(5000)),
            ..Default::default()
        };
        let message = apply_update(&settings(), &mut s, &input)
            .unwrap_err()
            .to_string();
        assert!(message.chars().count() < 200, "{}", message.len());
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
    fn changing_to_freeform_switches_reminders_off() {
        // A dirty-only save sends the cadence alone.
        let mut s = series();
        s.reminder_enabled = true;
        s.reminder_time = Some("17:30".to_owned());
        let input = UpdateSeriesInput {
            cadence: Some(Cadence::Freeform),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.cadence, Cadence::Freeform);
        assert!(!s.reminder_enabled);
        // The time is kept for when a schedule comes back.
        assert_eq!(s.reminder_time.as_deref(), Some("17:30"));

        // Asking for both in one update is still refused.
        let mut s = series();
        s.reminder_enabled = true;
        s.reminder_time = Some("17:30".to_owned());
        let input = UpdateSeriesInput {
            cadence: Some(Cadence::Freeform),
            reminder_enabled: Some(true),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(ValidationError::ReminderOnFreeform))
        ));

        // A freeform row stored with reminders on is corrected by any save.
        let mut s = series();
        s.cadence = Cadence::Freeform;
        s.reminder_enabled = true;
        let input = UpdateSeriesInput {
            description: Some("new".to_owned()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert!(!s.reminder_enabled);

        // Leaving freeform does not switch them back on by itself.
        let input = UpdateSeriesInput {
            cadence: Some(Cadence::Daily),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert!(!s.reminder_enabled);
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
    fn blank_reminder_time_asks_for_a_time_instead_of_a_format() {
        // Enabling with the time field left empty.
        let mut s = series();
        let input = UpdateSeriesInput {
            reminder_enabled: Some(true),
            reminder_time: Some(String::new()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(
                ValidationError::ReminderTimeRequired
            ))
        ));

        // Clearing the time of a series whose reminders are already on.
        let mut s = series();
        s.reminder_enabled = true;
        s.reminder_time = Some("09:00".to_owned());
        let input = UpdateSeriesInput {
            reminder_time: Some("  ".to_owned()),
            ..Default::default()
        };
        assert!(matches!(
            apply_update(&settings(), &mut s, &input),
            Err(UpdateError::Validation(
                ValidationError::ReminderTimeRequired
            ))
        ));

        // With reminders off, a blank time simply clears it.
        let mut s = series();
        s.reminder_time = Some("09:00".to_owned());
        let input = UpdateSeriesInput {
            reminder_time: Some(String::new()),
            ..Default::default()
        };
        apply_update(&settings(), &mut s, &input).unwrap();
        assert_eq!(s.reminder_time, None);
    }

    #[test]
    fn reminder_time_is_stored_zero_padded() {
        let mut s = series();
        for (typed, stored) in [("9:05", "09:05"), (" 17:30 ", "17:30"), ("00:00", "00:00")] {
            let input = UpdateSeriesInput {
                reminder_time: Some(typed.to_owned()),
                ..Default::default()
            };
            apply_update(&settings(), &mut s, &input).unwrap();
            assert_eq!(s.reminder_time.as_deref(), Some(stored), "{typed}");
        }
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
