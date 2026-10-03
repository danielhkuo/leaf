//! `/setup`: the server settings an admin picks in chat.
//!
//! One private message holds the whole form: the series channels, the log
//! channel, the creator role and the timezone, each a native menu that
//! opens on what is saved, plus Save and Cancel. A classic message cannot
//! put text between menus, so the message text is a numbered list in the
//! order of the menus, rewritten after every pick.
//!
//! Nothing is written until Save. A Save that cannot go through (no series
//! channel, a log channel leaf cannot post in) says why and leaves the form
//! up. Save writes only what this form owns, and the role and the timezone
//! only when the admin changed them here, so a change made in the admin
//! panel while the form was open survives. Save and the public how-to check
//! Manage Server again at the press, since the form outlives the command's
//! own check.
//!
//! Presses and the timezone modal are collected for this one run of the
//! command: the ids carry a per-process nonce and the command's interaction
//! id. The form closes after ten idle minutes, inside the fifteen an
//! interaction token can edit its message.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use anyhow::Context as _;
use chrono::Offset as _;
use leaf_core::db::GuildSettingsRepo;
use leaf_core::domain::{GuildSettings, Series, SeriesState};
use leaf_core::localtime::{self, Tz};
use leaf_core::series_ops;
use poise::serenity_prelude as serenity;
use serenity::Builder as _;
use serenity::futures::{Stream, StreamExt as _};
use tokio::time::Instant;

use super::series_lookup::{and_list, bold, clip, open_gallery_button, scoped_id};
use crate::components::{self, OPEN_GALLERY_FALLBACK};
use crate::{Context, Data, Error, checks};

/// How long the form waits after the last press before it closes.
const IDLE: Duration = Duration::from_mins(10);

/// How long an interaction token is trusted to edit its message. Discord
/// allows 15 minutes; this stays inside.
const TOKEN_LIFE: Duration = Duration::from_mins(14);

/// Longest wait for a message leaf posts on Save or for the how-to.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// Discord's limit for values (and options) in one select menu.
const SELECT_MAX_VALUES: u8 = 25;

/// Discord's 2000-character message limit, less a margin.
const MESSAGE_MAX_CHARS: usize = 1900;

/// Discord's limit for a button label.
const BUTTON_LABEL_MAX_CHARS: usize = 80;

/// Channels named in one warning, or in the how-to, before "N more".
const CHANNELS_SHOWN_MAX: usize = 8;

/// Series named in one warning before "N more".
const SERIES_SHOWN_MAX: usize = 3;

/// Timezones named when a typed city fits several.
const ZONES_SHOWN_MAX: usize = 4;

/// How much of a typed timezone is echoed back.
const ECHO_MAX_CHARS: usize = 40;

/// Custom id of the timezone modal's only field.
const ZONE_INPUT_ID: &str = "zone";

/// Longest IANA name is 32 characters; this leaves room for a slip.
const ZONE_INPUT_MAX_CHARS: u16 = 64;

/// Title of the timezone modal.
const ZONE_MODAL_TITLE: &str = "Server timezone";

/// Label of the timezone modal's field.
const ZONE_INPUT_LABEL: &str = "City, or a timezone name";

/// What the empty timezone field shows.
const ZONE_INPUT_PLACEHOLDER: &str = "Paris, New York, Europe/Paris, Asia/Tokyo";

/// The name Discord gives a channel the bot may not view (gateway
/// obfuscation, mandatory from 2026-11-16).
const HIDDEN_CHANNEL_NAME: &str = "___hidden___";

/// Discord error: the channel does not exist.
const UNKNOWN_CHANNEL: isize = 10003;
/// Discord error: the bot cannot see the channel.
const MISSING_ACCESS: isize = 50001;
/// Discord error: the bot lacks a permission.
const MISSING_PERMISSIONS: isize = 50013;

/// What leaf needs in a series channel: to see it, to react to archived
/// posts, and to post milestones and channel reminders.
const SERIES_CHANNEL_NEEDS: serenity::Permissions = serenity::Permissions::VIEW_CHANNEL
    .union(serenity::Permissions::SEND_MESSAGES)
    .union(serenity::Permissions::ADD_REACTIONS)
    .union(serenity::Permissions::READ_MESSAGE_HISTORY);

/// What leaf needs to post anything in a channel.
const POSTING_NEEDS: serenity::Permissions =
    serenity::Permissions::VIEW_CHANNEL.union(serenity::Permissions::SEND_MESSAGES);

/// The timezone menu: the zones most servers are in, west to east, each
/// with the places it covers. Twenty-four, so a saved zone that is not
/// among them still fits in the menu's 25 options.
const COMMON_ZONES: [(&str, &str); 24] = [
    ("Pacific/Honolulu", "Hawaii"),
    ("America/Anchorage", "Alaska"),
    (
        "America/Los_Angeles",
        "Pacific Time: Los Angeles, Vancouver",
    ),
    ("America/Denver", "Mountain Time: Denver, Edmonton"),
    ("America/Chicago", "Central Time: Chicago, Winnipeg"),
    ("America/New_York", "Eastern Time: New York, Toronto"),
    ("America/Mexico_City", "Mexico City"),
    ("America/Bogota", "Bogotá, Lima"),
    ("America/Sao_Paulo", "São Paulo, Rio de Janeiro"),
    ("America/Argentina/Buenos_Aires", "Buenos Aires"),
    ("UTC", "UTC (no daylight saving)"),
    ("Europe/London", "London, Dublin, Lisbon"),
    (
        "Europe/Berlin",
        "Central Europe: Berlin, Paris, Madrid, Rome",
    ),
    ("Europe/Athens", "Eastern Europe: Athens, Helsinki, Kyiv"),
    ("Africa/Johannesburg", "Southern Africa: Johannesburg"),
    ("Europe/Istanbul", "Istanbul, Moscow, Riyadh"),
    ("Asia/Dubai", "Dubai, Abu Dhabi"),
    ("Asia/Karachi", "Pakistan: Karachi"),
    ("Asia/Kolkata", "India: Mumbai, Delhi, Kolkata"),
    ("Asia/Bangkok", "Bangkok, Jakarta, Ho Chi Minh City"),
    ("Asia/Shanghai", "Beijing, Singapore, Manila, Perth"),
    ("Asia/Tokyo", "Tokyo, Seoul"),
    ("Australia/Sydney", "Sydney, Melbourne"),
    ("Pacific/Auckland", "New Zealand: Auckland"),
];

/// The gesture that archives a post, worded as in reminders and refusals.
const ARCHIVE_GESTURE: &str =
    "long-press the post (right-click on desktop), then Apps, then Archive to Series";

/// Shown while Save checks the log channel and writes.
const SAVING: &str = "⏳ Saving…";

/// Save pressed with no series channel (the button is disabled then, so
/// this answers a press that was already on its way).
const NO_CHANNEL_NOTICE: &str = "⚠️ Pick at least one series channel in the 1st menu, then press \
     **Save**.";

/// A Save that failed on leaf's side.
const SAVE_FAILED_NOTICE: &str = "⚠️ Something went wrong on my end while saving, and it's been \
     logged. Your picks are still here: press **Save** to try again.";

/// A press that failed on leaf's side.
const STEP_FAILURE: &str =
    "🍂 Something went wrong on my end. It's been logged. Try that again in a moment.";

// ---------------------------------------------------------------------------
// What leaf knows about the server
// ---------------------------------------------------------------------------

/// One channel, as the gateway cache has it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ChannelFacts {
    name: String,
    /// A text or announcement channel: one the channel menus offer.
    selectable: bool,
    /// leaf's permissions there. `None` when they cannot be worked out:
    /// leaf's own member is not cached, or the channel is a thread (its
    /// permissions come from its parent).
    permissions: Option<serenity::Permissions>,
}

impl ChannelFacts {
    fn new(
        name: &str,
        kind: serenity::ChannelType,
        permissions: Option<serenity::Permissions>,
    ) -> Self {
        // A hidden channel may arrive without its overwrites, so what the
        // cache computes for it cannot be trusted: leaf has no access.
        let hidden = name == HIDDEN_CHANNEL_NAME;
        Self {
            name: name.to_owned(),
            selectable: matches!(
                kind,
                serenity::ChannelType::Text | serenity::ChannelType::News
            ),
            permissions: if hidden {
                Some(serenity::Permissions::empty())
            } else {
                permissions
            },
        }
    }
}

/// The parts of the cached guild the form reads, keyed by id as the
/// settings store them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct GuildFacts {
    channels: HashMap<String, ChannelFacts>,
    /// Role id to "a bot's own role", which no member can hold.
    roles: HashMap<String, bool>,
}

/// Whether `role` is the role Discord made for a bot. Only that bot has it.
/// Other managed roles (server boosters, a streamer's subscribers) are held
/// by members, so they can gate who starts a series.
const fn is_bot_role(role: &serenity::Role) -> bool {
    role.tags.bot_id.is_some()
}

/// Reads the guild from the gateway cache. `None` when it is not cached
/// (an outage): the form then trusts what is saved and checks nothing.
fn guild_facts(cache: &serenity::Cache, guild_id: serenity::GuildId) -> Option<GuildFacts> {
    let me = cache.current_user().id;
    let guild = cache.guild(guild_id)?;
    let member = guild.members.get(&me);
    let mut channels: HashMap<String, ChannelFacts> = guild
        .channels
        .values()
        .map(|channel| {
            let permissions = member.map(|member| guild.user_permissions_in(channel, member));
            let facts = ChannelFacts::new(&channel.name, channel.kind, permissions);
            (channel.id.to_string(), facts)
        })
        .collect();
    for thread in &guild.threads {
        let facts = ChannelFacts::new(&thread.name, thread.kind, None);
        channels.insert(thread.id.to_string(), facts);
    }
    let roles = guild
        .roles
        .values()
        .map(|role| (role.id.to_string(), is_bot_role(role)))
        .collect();
    drop(guild);
    Some(GuildFacts { channels, roles })
}

/// Where leaf stands in a channel, measured against what it needs there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Footing {
    /// Everything needed is granted, or leaf cannot tell.
    Fine,
    /// leaf cannot view the channel (or it is not in the server).
    Unseen,
    /// leaf can view the channel and lacks these.
    Missing(serenity::Permissions),
}

fn footing(facts: Option<&GuildFacts>, channel: &str, needed: serenity::Permissions) -> Footing {
    let Some(facts) = facts else {
        return Footing::Fine;
    };
    let Some(channel) = facts.channels.get(channel) else {
        return Footing::Unseen;
    };
    let Some(granted) = channel.permissions else {
        return Footing::Fine;
    };
    if !granted.contains(serenity::Permissions::VIEW_CHANNEL) {
        return Footing::Unseen;
    }
    let missing = needed.difference(granted);
    if missing.is_empty() {
        Footing::Fine
    } else {
        Footing::Missing(missing)
    }
}

/// The permissions leaf checks, named as Discord's channel settings do.
fn permission_names(permissions: serenity::Permissions) -> Vec<String> {
    [
        (serenity::Permissions::VIEW_CHANNEL, "View Channel"),
        (serenity::Permissions::SEND_MESSAGES, "Send Messages"),
        (serenity::Permissions::ADD_REACTIONS, "Add Reactions"),
        (
            serenity::Permissions::READ_MESSAGE_HISTORY,
            "Read Message History",
        ),
    ]
    .into_iter()
    .filter(|(permission, _)| permissions.contains(*permission))
    .map(|(_, name)| name.to_owned())
    .collect()
}

// ---------------------------------------------------------------------------
// The draft: what the form holds until Save
// ---------------------------------------------------------------------------

/// The four things `/setup` chooses.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Draft {
    watched: Vec<String>,
    log_channel: Option<String>,
    creator_role: Option<String>,
    timezone: String,
}

/// Saved values the form dropped on opening because they point at nothing
/// any more. Each is said in the form, so Save removes nothing unannounced.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Stale {
    /// Series channels that no longer exist.
    channels: usize,
    log_channel: bool,
    creator_role: bool,
    /// The saved timezone, when it is not one leaf knows.
    timezone: Option<String>,
}

impl Draft {
    /// What the form opens with: the saved settings, less what no longer
    /// exists. Without `facts` nothing can be checked, so everything is
    /// kept.
    fn open(settings: &GuildSettings, facts: Option<&GuildFacts>) -> (Self, Stale) {
        let mut stale = Stale::default();
        let channel_exists = |id: &String| facts.is_none_or(|f| f.channels.contains_key(id));

        let watched: Vec<String> = settings
            .watched_channels
            .iter()
            .filter(|id| channel_exists(id))
            .cloned()
            .collect();
        stale.channels = settings.watched_channels.len() - watched.len();

        let log_channel = settings.log_channel_id.clone().filter(channel_exists);
        stale.log_channel = settings.log_channel_id.is_some() && log_channel.is_none();

        let creator_role = settings
            .creator_role_id
            .clone()
            .filter(|id| facts.is_none_or(|f| f.roles.contains_key(id)));
        stale.creator_role = settings.creator_role_id.is_some() && creator_role.is_none();

        let timezone = series_ops::canonical_timezone(&settings.timezone).map_or_else(
            || {
                stale.timezone = Some(settings.timezone.clone());
                "UTC".to_owned()
            },
            str::to_owned,
        );

        let draft = Self {
            watched,
            log_channel,
            creator_role,
            timezone,
        };
        (draft, stale)
    }
}

/// A snowflake stored as text, when it is one.
fn snowflake(id: &str) -> Option<u64> {
    id.parse::<u64>().ok().filter(|id| *id != 0)
}

/// The channels a menu opens with: those of `ids` the menu can show. A
/// default that names a deleted channel, or one of a kind the menu does not
/// offer, makes Discord refuse the whole message.
fn channel_defaults(ids: &[String], facts: Option<&GuildFacts>) -> Vec<serenity::ChannelId> {
    let mut seen = HashSet::new();
    ids.iter()
        .filter(|id| facts.is_none_or(|f| f.channels.get(*id).is_some_and(|c| c.selectable)))
        .filter_map(|id| snowflake(id))
        .filter(|id| seen.insert(*id))
        .take(usize::from(SELECT_MAX_VALUES))
        .map(serenity::ChannelId::new)
        .collect()
}

/// The role the role menu opens with, if it still exists.
fn role_default(role: Option<&str>, facts: Option<&GuildFacts>) -> Option<serenity::RoleId> {
    role.filter(|id| facts.is_none_or(|f| f.roles.contains_key(*id)))
        .and_then(snowflake)
        .map(serenity::RoleId::new)
}

// ---------------------------------------------------------------------------
// Timezones
// ---------------------------------------------------------------------------

/// `now_unix` on the clock of `tz`.
fn local_time(tz: Tz, now_unix: i64) -> chrono::DateTime<Tz> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(now_unix, 0)
        .unwrap_or_default()
        .with_timezone(&tz)
}

/// The zone's offset right now, written as the gallery writes it: `UTC`,
/// `UTC-5`, `UTC+5:30`.
fn offset_label(tz: Tz, now_unix: i64) -> String {
    let seconds = local_time(tz, now_unix).offset().fix().local_minus_utc();
    if seconds == 0 {
        return "UTC".to_owned();
    }
    let sign = if seconds < 0 { '-' } else { '+' };
    let minutes = seconds.unsigned_abs() / 60;
    let (hours, rest) = (minutes / 60, minutes % 60);
    if rest == 0 {
        format!("UTC{sign}{hours}")
    } else {
        format!("UTC{sign}{hours}:{rest:02}")
    }
}

/// A timezone with its offset: `America/Chicago (UTC-5)`. A name that is
/// its own offset (`UTC`) stands alone.
fn zone_text(name: &str, now_unix: i64) -> String {
    match localtime::parse_tz(name).map(|tz| offset_label(tz, now_unix)) {
        Some(offset) if offset != name => format!("{name} ({offset})"),
        _ => name.to_owned(),
    }
}

/// As [`zone_text`], with the time there, so the admin can check the pick
/// against a clock: `America/Chicago (UTC-5, 15:42 there now)`.
fn zone_text_with_clock(name: &str, now_unix: i64) -> String {
    let Some(tz) = localtime::parse_tz(name) else {
        return name.to_owned();
    };
    let clock = local_time(tz, now_unix).format("%H:%M");
    let offset = offset_label(tz, now_unix);
    if offset == name {
        format!("{name} ({clock} there now)")
    } else {
        format!("{name} ({offset}, {clock} there now)")
    }
}

/// One entry of the timezone menu.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ZoneOption {
    value: String,
    label: String,
    description: String,
    selected: bool,
}

/// The timezone menu's options, with `current` selected. A `current` that
/// is not one of the common zones leads the list, so the menu always shows
/// what is set.
fn zone_options(current: &str, now_unix: i64) -> Vec<ZoneOption> {
    let offset = |name: &str| {
        localtime::parse_tz(name).map_or_else(String::new, |tz| offset_label(tz, now_unix))
    };
    let mut options = Vec::with_capacity(COMMON_ZONES.len() + 1);
    if !COMMON_ZONES.iter().any(|(name, _)| *name == current) {
        options.push(ZoneOption {
            value: current.to_owned(),
            label: current.to_owned(),
            description: format!("Current choice · {}", offset(current)),
            selected: true,
        });
    }
    options.extend(COMMON_ZONES.iter().map(|(name, places)| ZoneOption {
        value: (*name).to_owned(),
        label: (*places).to_owned(),
        description: format!("{name} · {}", offset(name)),
        selected: *name == current,
    }));
    options
}

/// What a typed timezone turned out to be.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ZoneInput {
    Found(&'static str),
    /// A city that several different zones are named after.
    Several(Vec<&'static str>),
    /// An abbreviation that is a zone of its own with no daylight saving
    /// (`EST`), which is rarely the clock the admin means.
    NoDaylightSaving,
    Unknown,
}

/// Abbreviations the timezone database also lists as zones, fixed at their
/// standard offset: `EST` is an hour behind New York all summer. They are
/// turned away like `PST` or `CST`, which are no zones at all, so nobody
/// gets a clock without daylight saving by accident. (`HST` is fixed too,
/// but so is Hawaii.)
const NO_DAYLIGHT_SAVING: [&str; 2] = ["EST", "MST"];

/// Whether `a` and `b` are one timezone under two names: the same offset
/// in every week from 1970 to 2038, which is the timezone database's own
/// test for places that share a zone.
fn same_zone(a: Tz, b: Tz) -> bool {
    const WEEK_SECS: i64 = 7 * 24 * 60 * 60;
    const WEEKS: i64 = 3550;
    let offset = |tz: Tz, at: i64| local_time(tz, at).offset().fix().local_minus_utc();
    (0..WEEKS).all(|week| offset(a, week * WEEK_SECS) == offset(b, week * WEEK_SECS))
}

/// The name to use when all of `names` are one timezone (`Asia/Istanbul`
/// and `Europe/Istanbul`): the one the menu lists, else the fullest
/// spelling (`America/Argentina/Buenos_Aires` over `America/Buenos_Aires`).
/// `None` when they are different timezones.
fn one_zone(names: &[&'static str]) -> Option<&'static str> {
    let (first, rest) = names.split_first()?;
    let first_tz = localtime::parse_tz(first)?;
    let same = rest
        .iter()
        .all(|name| localtime::parse_tz(name).is_some_and(|tz| same_zone(first_tz, tz)));
    if !same {
        return None;
    }
    names
        .iter()
        .find(|name| COMMON_ZONES.iter().any(|(common, _)| common == *name))
        // `max_by_key` keeps the last of equals: reversed, the first listed.
        .or_else(|| names.iter().rev().max_by_key(|name| name.len()))
        .copied()
}

/// Reads a timezone typed into the modal: a full name in any case
/// (`europe/paris`), or just the city (`Paris`, `new york`), which is what
/// a phone keyboard makes easy.
fn resolve_zone(input: &str) -> ZoneInput {
    // The names use underscores where people type spaces.
    let typed = input
        .split('/')
        .map(|part| part.split_whitespace().collect::<Vec<_>>().join("_"))
        .collect::<Vec<_>>()
        .join("/");
    if typed.is_empty() {
        return ZoneInput::Unknown;
    }
    if NO_DAYLIGHT_SAVING
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&typed))
    {
        return ZoneInput::NoDaylightSaving;
    }
    if !typed.contains('/') {
        let mut cities: Vec<&'static str> = chrono_tz::TZ_VARIANTS
            .iter()
            .map(|tz| tz.name())
            // `Etc/UTC` and its kin are not cities: "utc" means `UTC`.
            .filter(|name| !name.starts_with("Etc/"))
            .filter(|name| {
                name.rsplit_once('/')
                    .is_some_and(|(_, city)| city.eq_ignore_ascii_case(&typed))
            })
            .collect();
        cities.sort_unstable();
        match cities.as_slice() {
            [] => {}
            [only] => return ZoneInput::Found(only),
            // Often one zone under an old and a new name.
            several => {
                return one_zone(several).map_or(ZoneInput::Several(cities), ZoneInput::Found);
            }
        }
    }
    series_ops::canonical_timezone(&typed).map_or(ZoneInput::Unknown, ZoneInput::Found)
}

/// What was typed, echoed safely.
fn echo(input: &str) -> String {
    bold(&clip(input.trim(), ECHO_MAX_CHARS))
}

/// The notice for a typed timezone leaf could not use.
fn zone_problem_text(input: &str, outcome: &ZoneInput, current: &str) -> String {
    let problem = match outcome {
        ZoneInput::Several(names) => {
            let mut shown: Vec<String> = names
                .iter()
                .take(ZONES_SHOWN_MAX)
                .map(|name| format!("`{name}`"))
                .collect();
            if names.len() > ZONES_SHOWN_MAX {
                shown.push(format!("{} more", names.len() - ZONES_SHOWN_MAX));
            }
            format!(
                "⚠️ {} fits more than one timezone: {}. Press **Other timezone** and type one \
                 of them in full.",
                echo(input),
                and_list(&shown)
            )
        }
        ZoneInput::NoDaylightSaving => format!(
            "⚠️ {} is a fixed offset that never switches to daylight saving. Press **Other \
             timezone** and type a city instead (New York, Denver, Phoenix).",
            echo(input)
        ),
        ZoneInput::Found(_) | ZoneInput::Unknown if input.trim().is_empty() => {
            "⚠️ The timezone field was empty. Press **Other timezone** and type a city \
             (Paris, New York) or a full name such as `Europe/Paris`."
                .to_owned()
        }
        ZoneInput::Found(_) | ZoneInput::Unknown => format!(
            "⚠️ leaf doesn't know a timezone called {}. Press **Other timezone** and type a \
             city (Paris, New York) or a full name such as `Europe/Paris`.",
            echo(input)
        ),
    };
    format!("{problem} The timezone is still **{current}**.")
}

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

/// "1 day" / "3 days".
fn days(n: i64) -> String {
    if n == 1 {
        "1 day".to_owned()
    } else {
        format!("{n} days")
    }
}

/// Every channel of `ids` as a mention, in order.
fn channel_mentions(ids: &[String]) -> String {
    ids.iter()
        .map(|id| format!("<#{id}>"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// "#a", "#a and #b", "#a, #b and 3 more": channels inside a sentence.
fn channel_list<S: AsRef<str>>(ids: &[S]) -> String {
    let mut shown: Vec<String> = ids
        .iter()
        .take(CHANNELS_SHOWN_MAX)
        .map(|id| format!("<#{}>", id.as_ref()))
        .collect();
    if ids.len() > CHANNELS_SHOWN_MAX {
        shown.push(format!("{} more", ids.len() - CHANNELS_SHOWN_MAX));
    }
    and_list(&shown)
}

/// Where to post, for the how-to: "#a", "#a or #b", "one of the series
/// channels (#a, #b, #c)". A creator posts in one of them, so the channels
/// are never joined with "and".
fn post_in_text<S: AsRef<str>>(ids: &[S]) -> String {
    let mention = |id: &S| format!("<#{}>", id.as_ref());
    match ids {
        [] => "a series channel".to_owned(),
        [only] => mention(only),
        [a, b] => format!("{} or {}", mention(a), mention(b)),
        many => {
            let mut shown: Vec<String> =
                many.iter().take(CHANNELS_SHOWN_MAX).map(mention).collect();
            if many.len() > CHANNELS_SHOWN_MAX {
                shown.push(format!("{} more", many.len() - CHANNELS_SHOWN_MAX));
            }
            format!("one of the series channels ({})", shown.join(", "))
        }
    }
}

/// The series channels on the old list, still in the server, that the new
/// list leaves out although live series post there: each with the names of
/// those series.
fn dropped_in_use(
    saved: &[String],
    watched: &[String],
    series: &[Series],
    facts: Option<&GuildFacts>,
) -> Vec<(String, Vec<String>)> {
    saved
        .iter()
        .filter(|id| !watched.contains(id))
        // A deleted channel takes no posts either way.
        .filter(|id| facts.is_none_or(|f| f.channels.contains_key(*id)))
        .filter_map(|id| {
            let users: Vec<String> = series
                .iter()
                .filter(|s| s.state != SeriesState::Revoked && s.channels.contains(id))
                .map(|s| s.name.clone())
                .collect();
            (!users.is_empty()).then(|| (id.clone(), users))
        })
        .collect()
}

/// What the admin should know before (and after) saving `draft`, the most
/// consequential first: a role nobody holds, series left without their
/// channel, a log leaf cannot write, channels leaf cannot fully use.
fn warnings(
    draft: &Draft,
    saved_watched: &[String],
    series: &[Series],
    facts: Option<&GuildFacts>,
) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(role) = draft.creator_role.as_deref()
        && facts.is_some_and(|f| f.roles.get(role).copied().unwrap_or(false))
    {
        lines.push(format!(
            "⚠️ <@&{role}> is a bot's own role, so no member can hold it: nobody can start a \
             series while it is the creator role. Pick a role members have."
        ));
    }

    for (channel, users) in dropped_in_use(saved_watched, &draft.watched, series, facts) {
        let mut names: Vec<String> = users
            .iter()
            .take(SERIES_SHOWN_MAX)
            .map(|name| bold(name))
            .collect();
        if users.len() > SERIES_SHOWN_MAX {
            names.push(format!("{} more", users.len() - SERIES_SHOWN_MAX));
        }
        let (verb, their) = if users.len() == 1 {
            ("posts", "Its")
        } else {
            ("post", "Their")
        };
        lines.push(format!(
            "⚠️ {} {verb} in <#{channel}>, which is not on the list. {their} posts there can't \
             be archived until it is added back or the creator picks another channel in Series \
             settings.",
            and_list(&names)
        ));
    }

    if let Some(log) = draft.log_channel.as_deref() {
        match footing(facts, log, POSTING_NEEDS) {
            Footing::Fine => {}
            Footing::Unseen => lines.push(format!(
                "⚠️ leaf can't see <#{log}>, so it can't write the log there."
            )),
            Footing::Missing(lacking) => lines.push(format!(
                "⚠️ leaf is missing {} in <#{log}>, so it can't write the log there.",
                and_list(&permission_names(lacking))
            )),
        }
    }

    lines.extend(series_channel_warnings(&draft.watched, facts));
    lines
}

/// The series channels leaf cannot see, then those where it lacks a
/// permission, grouped by what is missing so one line covers each case.
fn series_channel_warnings(watched: &[String], facts: Option<&GuildFacts>) -> Vec<String> {
    let mut unseen: Vec<&str> = Vec::new();
    let mut missing: BTreeMap<u64, Vec<&str>> = BTreeMap::new();
    for id in watched {
        match footing(facts, id, SERIES_CHANNEL_NEEDS) {
            Footing::Fine => {}
            Footing::Unseen => unseen.push(id),
            Footing::Missing(lacking) => missing.entry(lacking.bits()).or_default().push(id),
        }
    }

    let mut lines = Vec::new();
    if !unseen.is_empty() {
        lines.push(format!(
            "⚠️ leaf can't see {}. Posts there can still be archived, but leaf can't react, \
             post milestones or send channel reminders there until its role is allowed to \
             View Channel.",
            channel_list(&unseen)
        ));
    }
    for (bits, channels) in missing {
        let lacking = serenity::Permissions::from_bits_truncate(bits);
        lines.push(format!(
            "⚠️ leaf is missing {} in {}, so {} won't work there.",
            and_list(&permission_names(lacking)),
            channel_list(&channels),
            and_list(&effects_of(lacking))
        ));
    }
    lines
}

/// Builds a message from the lines that must be in it (`head`, `tail`) and
/// as many of `notes`, in order, as Discord's limit leaves room for. Notes
/// left out are counted, so nothing is ever cut mid-sentence.
fn assemble(head: Vec<String>, notes: &[String], tail: Vec<String>) -> String {
    /// Room kept for the line that counts what was left out, and for the
    /// blank line above the notes.
    const OVERFLOW_ROOM: usize = 60;
    let size = |lines: &[String]| -> usize { lines.iter().map(|l| l.chars().count() + 1).sum() };

    let mut room = MESSAGE_MAX_CHARS.saturating_sub(size(&head) + size(&tail) + OVERFLOW_ROOM);
    let mut shown = Vec::new();
    for note in notes {
        let cost = note.chars().count() + 1;
        if cost > room {
            break;
        }
        room -= cost;
        shown.push(note.clone());
    }
    match notes.len() - shown.len() {
        0 => {}
        1 => shown.push("⚠️ One more warning doesn't fit in this message.".to_owned()),
        left => shown.push(format!(
            "⚠️ {left} more warnings don't fit in this message."
        )),
    }

    let mut lines = head;
    if !shown.is_empty() {
        lines.push(String::new());
        lines.extend(shown);
    }
    lines.extend(tail);
    lines.join("\n")
}

/// What stops working in a series channel when `lacking` is not granted.
fn effects_of(lacking: serenity::Permissions) -> Vec<String> {
    let mut effects = Vec::new();
    if lacking.intersects(
        serenity::Permissions::ADD_REACTIONS | serenity::Permissions::READ_MESSAGE_HISTORY,
    ) {
        effects.push("the reaction on archived posts".to_owned());
    }
    if lacking.contains(serenity::Permissions::SEND_MESSAGES) {
        effects.push("milestone posts".to_owned());
        effects.push("channel reminders".to_owned());
    }
    effects
}

/// Who may start a series, with the limits that apply to them.
fn policy_text(settings: &GuildSettings) -> String {
    if settings.max_series_per_user < 1 {
        return "nobody: the limit is 0 series per member. Raise it in the admin panel.".to_owned();
    }
    let who = settings.creator_role_id.as_deref().map_or_else(
        || "anyone in the server".to_owned(),
        |role| format!("members with <@&{role}>"),
    );
    let mut rules = vec![format!(
        "up to {} series each",
        settings.max_series_per_user
    )];
    if settings.min_account_age_days > 0 {
        rules.push(format!(
            "Discord account at least {} old",
            days(settings.min_account_age_days)
        ));
    }
    if settings.min_membership_age_days > 0 {
        rules.push(format!(
            "a member here for at least {}",
            days(settings.min_membership_age_days)
        ));
    }
    format!("{who} ({})", rules.join("; "))
}

/// Where the rest of the settings are.
const fn panel_note(has_link: bool) -> &'static str {
    if has_link {
        "-# Series limits, account-age rules and sprouts are in the **Admin panel**."
    } else {
        "-# Series limits, account-age rules and sprouts are in the admin panel: your leaf \
         address, then /admin."
    }
}

/// Everything the form's text depends on.
struct FormView<'a> {
    first_run: bool,
    draft: &'a Draft,
    stale: &'a Stale,
    warnings: &'a [String],
    notice: Option<&'a str>,
    /// A menu looks empty although it has a value: Discord refused the
    /// pre-filled form.
    plain: bool,
    has_link: bool,
    now_unix: i64,
}

/// The form's message: the numbered list the menus follow, then anything
/// the admin should know before saving.
fn form_text(view: &FormView<'_>) -> String {
    let draft = view.draft;
    let heading = if view.first_run {
        "🍃 **Set up leaf**"
    } else {
        "🍃 **leaf setup**: this server's saved settings are filled in"
    };
    let watched = if draft.watched.is_empty() {
        "none yet. Pick at least one: posts can only be archived from these channels and \
         their threads."
            .to_owned()
    } else {
        channel_mentions(&draft.watched)
    };
    let log = draft.log_channel.as_deref().map_or_else(
        || "none. Optional: one quiet line per archived or removed day.".to_owned(),
        |id| format!("<#{id}>"),
    );
    let role = draft.creator_role.as_deref().map_or_else(
        || "none, so anyone can start a series.".to_owned(),
        |id| format!("<@&{id}>, so only members with it can start a series."),
    );
    let head = vec![
        heading.to_owned(),
        "The four menus below follow this list. Pick, then press **Save**. Nothing changes \
         before that."
            .to_owned(),
        String::new(),
        format!("1. **Series channels**: {watched}"),
        format!("2. **Log channel**: {log}"),
        format!("3. **Creator role**: {role}"),
        format!(
            "4. **Timezone**: {}. {}",
            zone_text_with_clock(&draft.timezone, view.now_unix),
            if view.first_run && draft.timezone == "UTC" {
                "That is the default: pick your own so calendar dates and reminder times match \
                 your clock."
            } else {
                "Calendar dates and reminder times follow it."
            }
        ),
    ];

    // The notice answers the last press, so it comes first: if the message
    // runs out of room, it is the warnings at the end that are counted
    // instead of shown.
    let mut notes: Vec<String> = view
        .notice
        .iter()
        .map(|&notice| notice.to_owned())
        .collect();
    notes.extend(stale_notes(view.stale, draft));
    if view.plain {
        notes.push(
            "The menus couldn't be pre-filled, so they look empty. The list above is what \
             will be saved: a menu you leave alone keeps its value."
                .to_owned(),
        );
    }
    notes.extend(view.warnings.iter().cloned());
    assemble(head, &notes, vec![panel_note(view.has_link).to_owned()])
}

/// What the form says about saved values it dropped on opening. A note
/// goes away once the admin has picked a replacement.
fn stale_notes(stale: &Stale, draft: &Draft) -> Vec<String> {
    let mut notes = Vec::new();
    match stale.channels {
        0 => {}
        1 => notes.push(
            "One saved series channel no longer exists, so it comes off the list when you save."
                .to_owned(),
        ),
        n => notes.push(format!(
            "{n} saved series channels no longer exist, so they come off the list when you save."
        )),
    }
    if stale.log_channel && draft.log_channel.is_none() {
        notes.push(
            "The saved log channel no longer exists, so the log is switched off when you save. \
             Pick another in the 2nd menu to keep it."
                .to_owned(),
        );
    }
    if stale.creator_role && draft.creator_role.is_none() {
        notes.push(
            "The saved creator role was deleted, so nobody can start a series right now. \
             Saving lets anyone start one, unless you pick a role in the 3rd menu."
                .to_owned(),
        );
    }
    if let Some(saved) = stale.timezone.as_deref() {
        notes.push(format!(
            "The saved timezone {} isn't one leaf knows, so the 4th menu starts on UTC.",
            echo(saved)
        ));
    }
    notes
}

/// The channel the how-to would go to.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    id: String,
    /// Its name, for the button label, when the cache has it.
    name: Option<String>,
}

/// Where the offer to post a public how-to stands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HowTo {
    Offered(Target),
    Posting(Target),
    /// Link to the posted message.
    Posted(String),
    /// The post was refused, with the reason; the button is back.
    Failed(Target, &'static str),
    /// leaf can post in none of the series channels.
    NoChannel,
}

/// The first series channel leaf can post in.
fn how_to_target(watched: &[String], facts: Option<&GuildFacts>) -> Option<Target> {
    watched
        .iter()
        .find(|id| footing(facts, id, POSTING_NEEDS) == Footing::Fine)
        .map(|id| Target {
            id: id.clone(),
            name: facts
                .and_then(|f| f.channels.get(id))
                .map(|channel| channel.name.clone()),
        })
}

/// The line under the saved summary about telling the members. With `live`
/// false the form has stopped listening and its buttons are gone, so the
/// line names the way back to them instead.
fn how_to_line(how_to: &HowTo, live: bool) -> String {
    match how_to {
        HowTo::Offered(target) if live => format!(
            "Want members to know how leaf works? **Post a how-to** puts a short public guide \
             in <#{}>: how to start a series and how to archive a post.",
            target.id
        ),
        HowTo::Posting(target) if live => format!("⏳ Posting the how-to in <#{}>…", target.id),
        HowTo::Offered(_) | HowTo::Posting(_) => {
            "To post a public how-to for members, run `/setup` and press **Save** again.".to_owned()
        }
        HowTo::Posted(link) => format!("✅ The how-to is posted: {link}"),
        HowTo::Failed(target, reason) => format!(
            "⚠️ leaf couldn't post the how-to in <#{}>. {reason} {}",
            target.id,
            if live {
                "Press the button to try again."
            } else {
                "To try again, run `/setup` and press **Save**."
            }
        ),
        HowTo::NoChannel => format!(
            "leaf can't post in any series channel, so telling members is up to you: they \
             start a series from the gallery, and to archive a post they {ARCHIVE_GESTURE}."
        ),
    }
}

/// The summary after Save, from the settings as stored. `live` is whether
/// its buttons still answer.
fn saved_text(
    settings: &GuildSettings,
    first_run: bool,
    warnings: &[String],
    how_to: &HowTo,
    live: bool,
    has_link: bool,
    now_unix: i64,
) -> String {
    let heading = if first_run {
        "🍃 **leaf is set up.**"
    } else {
        "🍃 **Setup saved.**"
    };
    let log = settings
        .log_channel_id
        .as_deref()
        .map_or_else(|| "none".to_owned(), |id| format!("<#{id}>"));
    let mut head = vec![
        heading.to_owned(),
        format!(
            "**Series channels**: {}",
            channel_mentions(&settings.watched_channels)
        ),
        format!("**Log channel**: {log}"),
        format!("**Who can start a series**: {}", policy_text(settings)),
        format!("**Timezone**: {}", zone_text(&settings.timezone, now_unix)),
    ];
    if settings.sprout_enabled {
        head.push(format!(
            "**New series** start as sprouts: only their creator sees them until {} are \
             archived.",
            days(settings.sprout_threshold)
        ));
    }
    let tail = vec![
        String::new(),
        how_to_line(how_to, live),
        panel_note(has_link).to_owned(),
    ];
    assemble(head, warnings, tail)
}

/// Who can start a series, as one sentence for the members: the stored
/// role and age rules, as [`policy_text`] gives them to the admin. `None`
/// when the limit is 0 series, so nobody can.
fn starters_text(settings: &GuildSettings) -> Option<String> {
    if settings.max_series_per_user < 1 {
        return None;
    }
    let who = settings.creator_role_id.as_deref().map_or_else(
        || "Anyone here".to_owned(),
        |role| format!("Members with <@&{role}>"),
    );
    let mut rules = Vec::new();
    if settings.min_account_age_days > 0 {
        rules.push(format!(
            "their Discord account is at least {} old",
            days(settings.min_account_age_days)
        ));
    }
    if settings.min_membership_age_days > 0 {
        rules.push(format!(
            "they have been a member of this server for at least {}",
            days(settings.min_membership_age_days)
        ));
    }
    Some(if rules.is_empty() {
        format!("{who} can start one.")
    } else {
        format!("{who} can start one once {}.", and_list(&rules))
    })
}

/// The public how-to: what leaf is, how to start a series, how to archive.
pub(crate) fn how_to_text(settings: &GuildSettings) -> String {
    let start = starters_text(settings).map_or_else(
        || "closed for now. This server's limit is 0 series per member.".to_owned(),
        // On a narrow screen the gallery may open on a series page, where
        // the same control is only a "+" at the top.
        |who| {
            format!(
                "press **Open gallery** below, then **Start a series** (on a series page it \
                 is the **+** at the top). {who}"
            )
        },
    );
    format!(
        "🍃 **leaf is ready in this server.**\n\
         leaf keeps a gallery of ongoing series: one numbered post at a time (Day 1, Day 2, and \
         so on).\n\n\
         **Start a series**: {start}\n\
         **Archive a post**: post your photo or video in {}, then {ARCHIVE_GESTURE}.\n\
         **Browse**: the gallery shows every public series as a calendar.\n\
         -# If the button doesn't open the gallery: on mobile, tap + next to the message box, \
         then Apps, then leaf. On desktop, open the app launcher in the message box and choose \
         leaf.",
        post_in_text(&settings.watched_channels)
    )
}

/// The first line leaf writes in a newly chosen log channel. Sending it is
/// also the proof that leaf can post there.
fn log_test_text(admin: serenity::UserId) -> String {
    format!(
        "🍃 This is now leaf's log channel: one quiet line for each day that is archived, \
         moved or removed. Set with `/setup` by <@{admin}>."
    )
}

/// Why a message did not go out, with the fix where there is one. Without
/// a code leaf knows, Discord refused for another reason or did not answer
/// in time.
const fn send_refusal(code: Option<isize>) -> &'static str {
    match code {
        Some(MISSING_ACCESS) => {
            "leaf can't see that channel: allow its role to View Channel and Send Messages there."
        }
        Some(MISSING_PERMISSIONS) => {
            "leaf isn't allowed to post there: allow its role to Send Messages in that channel."
        }
        Some(UNKNOWN_CHANNEL) => "That channel no longer exists.",
        _ => {
            "Discord didn't accept the message, or took too long to answer. That may be temporary."
        }
    }
}

/// The notice when the new log channel fails its test line.
fn log_refusal_text(channel: &str, code: Option<isize>) -> String {
    format!(
        "⚠️ leaf couldn't post a test line in <#{channel}>, so nothing was saved. {} Press \
         **Save** to try again, or pick another log channel in the 2nd menu.",
        send_refusal(code)
    )
}

/// What replaces the form when it closes unsaved.
fn closed_text(first_run: bool, cancelled: bool) -> String {
    let what = if cancelled {
        "Setup cancelled."
    } else {
        "⏳ Setup closed after 10 minutes without a change."
    };
    let kept = if first_run {
        "Nothing was saved, so leaf still isn't set up here. Run `/setup` again when you're \
         ready."
    } else {
        "Nothing was saved, so the settings are as they were. Run `/setup` again to change \
         them."
    };
    format!("{what} {kept}")
}

/// Whether a press comes from someone who may manage the server, by the
/// permissions Discord sends with it. The command checks this once, when
/// it is run; the form stays open long enough for it to change.
fn may_manage(permissions: Option<serenity::Permissions>) -> bool {
    permissions.is_some_and(|granted| granted.administrator() || granted.manage_guild())
}

/// What replaces the form, or the summary once `saved`, when the admin
/// lost Manage Server while it was open.
const fn demoted_text(saved: bool) -> &'static str {
    if saved {
        "🍂 You no longer have Manage Server in this server, so the how-to wasn't posted. The \
         settings you saved are unchanged."
    } else {
        "🍂 You no longer have Manage Server in this server, so nothing was saved. The settings \
         are as they were."
    }
}

/// Discord's JSON error code, when `error` is a refused request.
const fn discord_code(error: &serenity::Error) -> Option<isize> {
    match error {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)) => {
            Some(response.error.code)
        }
        _ => None,
    }
}

/// Whether Discord refused the request as malformed (a 400), which is what
/// a menu default it does not accept looks like.
fn is_bad_request(error: &serenity::Error) -> bool {
    matches!(
        error,
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response))
            if response.status_code == serenity::StatusCode::BAD_REQUEST
    )
}

// ---------------------------------------------------------------------------
// Saving
// ---------------------------------------------------------------------------

/// The log channel to try a line in before saving: the draft's, unless it
/// is the one already stored. Re-saving must not post a line every time.
fn log_to_test<'a>(draft: &'a Draft, stored: Option<&str>) -> Option<&'a str> {
    draft
        .log_channel
        .as_deref()
        .filter(|log| stored != Some(*log))
}

/// Writes `draft` and returns the settings as they are stored afterwards.
///
/// Only what the form owns is written. The role and the timezone are
/// written only when they differ from `opened`, what the form showed when
/// it opened, so a change made in the admin panel meanwhile is not undone
/// by a form that never touched them. The channels go last: they are what
/// marks setup complete.
async fn store(
    guilds: &GuildSettingsRepo,
    guild_id: &str,
    draft: &Draft,
    opened: &GuildSettings,
) -> Result<GuildSettings, Error> {
    if draft.creator_role != opened.creator_role_id {
        guilds
            .set_creator_role(guild_id, draft.creator_role.as_deref())
            .await?;
    }
    if draft.timezone != opened.timezone {
        guilds.set_timezone(guild_id, &draft.timezone).await?;
    }
    guilds
        .set_channels(guild_id, &draft.watched, draft.log_channel.as_deref())
        .await?;
    guilds
        .get(guild_id)
        .await?
        .context("guild settings are gone right after saving them")
}

/// When a quiet form next has to act, and whether it keeps listening after
/// that.
///
/// Normally that is `deadline`, the end of the idle time. But the token
/// that edits the message (`surface_until`) can die first: opening the
/// timezone modal restarts the idle clock without bringing a new token.
/// The message then has to be closed while it still can be, and if a modal
/// may be open the form listens on, because the modal's submit brings a
/// token of its own and puts the form back.
fn wake(deadline: Instant, surface_until: Option<Instant>, modal_open: bool) -> (Instant, bool) {
    match surface_until {
        Some(until) if until < deadline => (until, modal_open),
        _ => (deadline, false),
    }
}

// ---------------------------------------------------------------------------
// The admin panel link
// ---------------------------------------------------------------------------

/// The admin panel, opened on this server.
fn admin_url(public_url: &str, guild_id: &str) -> String {
    format!(
        "{}/admin?guild={guild_id}",
        public_url.trim_end_matches('/')
    )
}

// ---------------------------------------------------------------------------
// Component ids and rows
// ---------------------------------------------------------------------------

/// What a menu or button of the form does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Watched,
    Log,
    Role,
    Zone,
    OtherZone,
    Save,
    Cancel,
    OpenGallery,
    HowTo,
}

impl Action {
    const ALL: [Self; 9] = [
        Self::Watched,
        Self::Log,
        Self::Role,
        Self::Zone,
        Self::OtherZone,
        Self::Save,
        Self::Cancel,
        Self::OpenGallery,
        Self::HowTo,
    ];

    const fn as_str(self) -> &'static str {
        match self {
            Self::Watched => "watched",
            Self::Log => "log",
            Self::Role => "role",
            Self::Zone => "zone",
            Self::OtherZone => "other-zone",
            Self::Save => "save",
            Self::Cancel => "cancel",
            Self::OpenGallery => "open",
            Self::HowTo => "how-to",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|action| action.as_str() == text)
    }
}

/// The ids of one run of the command. They start with the process nonce
/// and the command's interaction id, so a press can only belong to this
/// form and a form from before a restart is never mistaken for a live one.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Ids {
    prefix: String,
}

impl Ids {
    fn new(interaction: u64) -> Self {
        let mut prefix = scoped_id("setup", &[&interaction.to_string()]);
        prefix.push(':');
        Self { prefix }
    }

    fn of(&self, action: Action) -> String {
        format!("{}{}", self.prefix, action.as_str())
    }

    /// Id of the timezone modal. `opener` is the press that opened it, so
    /// each open gets a fresh id (iOS caches a modal's contents by id).
    fn modal(&self, opener: u64) -> String {
        format!("{}m:{opener}", self.prefix)
    }

    fn action(&self, custom_id: &str) -> Option<Action> {
        Action::parse(custom_id.strip_prefix(&self.prefix)?)
    }
}

/// What is on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Stage {
    Form,
    Saved(HowTo),
    Closed,
}

impl Stage {
    /// Whether `action` belongs to what is on screen. A press that does
    /// not (a double tap, a button from before Save) is ignored.
    const fn allows(&self, action: Action) -> bool {
        match self {
            Self::Form => !matches!(action, Action::OpenGallery | Action::HowTo),
            Self::Saved(how_to) => match action {
                Action::OpenGallery => true,
                Action::HowTo => matches!(how_to, HowTo::Offered(_) | HowTo::Failed(..)),
                _ => false,
            },
            Self::Closed => false,
        }
    }
}

/// The link button to the admin panel.
fn panel_button(url: &str) -> serenity::CreateButton {
    serenity::CreateButton::new_link(url).label("Admin panel")
}

/// Which menus may open on a selection leaf supplies. All of them, until
/// Discord refuses the form as malformed; then none, except a menu the
/// admin has since picked in, whose values come from Discord's own picker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Prefill {
    watched: bool,
    log_channel: bool,
    creator_role: bool,
}

impl Prefill {
    const ALL: Self = Self {
        watched: true,
        log_channel: true,
        creator_role: true,
    };
    const NONE: Self = Self {
        watched: false,
        log_channel: false,
        creator_role: false,
    };

    /// Whether a menu looks empty although the draft has a value for it.
    const fn hides(self, draft: &Draft) -> bool {
        (!self.watched && !draft.watched.is_empty())
            || (!self.log_channel && draft.log_channel.is_some())
            || (!self.creator_role && draft.creator_role.is_some())
    }
}

/// The form: four menus and the button row.
fn form_rows(
    ids: &Ids,
    draft: &Draft,
    facts: Option<&GuildFacts>,
    panel: Option<&str>,
    prefill: Prefill,
    now_unix: i64,
) -> Vec<serenity::CreateActionRow> {
    let kinds = || {
        Some(vec![
            serenity::ChannelType::Text,
            serenity::ChannelType::News,
        ])
    };
    let filled = |allowed: bool, ids: &[String]| {
        Some(channel_defaults(ids, facts)).filter(|defaults| allowed && !defaults.is_empty())
    };
    let log_channel: Vec<String> = draft.log_channel.iter().cloned().collect();

    let watched = serenity::CreateSelectMenu::new(
        ids.of(Action::Watched),
        serenity::CreateSelectMenuKind::Channel {
            channel_types: kinds(),
            default_channels: filled(prefill.watched, &draft.watched),
        },
    )
    .placeholder("Series channels: where posts can be archived from")
    .min_values(1)
    .max_values(SELECT_MAX_VALUES);
    let log = serenity::CreateSelectMenu::new(
        ids.of(Action::Log),
        serenity::CreateSelectMenuKind::Channel {
            channel_types: kinds(),
            default_channels: filled(prefill.log_channel, &log_channel),
        },
    )
    .placeholder("Log channel (optional): one quiet line per archive")
    .min_values(0)
    .max_values(1);
    let role = serenity::CreateSelectMenu::new(
        ids.of(Action::Role),
        serenity::CreateSelectMenuKind::Role {
            default_roles: role_default(draft.creator_role.as_deref(), facts)
                .filter(|_| prefill.creator_role)
                .map(|role| vec![role]),
        },
    )
    .placeholder("Creator role (optional): leave empty and anyone can start a series")
    .min_values(0)
    .max_values(1);
    let zones = zone_options(&draft.timezone, now_unix)
        .into_iter()
        .map(|zone| {
            serenity::CreateSelectMenuOption::new(zone.label, zone.value)
                .description(zone.description)
                .default_selection(zone.selected)
        })
        .collect();
    let zone = serenity::CreateSelectMenu::new(
        ids.of(Action::Zone),
        serenity::CreateSelectMenuKind::String { options: zones },
    )
    .placeholder("Timezone: for calendar dates and reminder times");

    let mut buttons = vec![
        serenity::CreateButton::new(ids.of(Action::Save))
            .style(serenity::ButtonStyle::Success)
            .label("Save")
            .disabled(draft.watched.is_empty()),
        serenity::CreateButton::new(ids.of(Action::OtherZone))
            .style(serenity::ButtonStyle::Secondary)
            .label("Other timezone"),
        serenity::CreateButton::new(ids.of(Action::Cancel))
            .style(serenity::ButtonStyle::Secondary)
            .label("Cancel"),
    ];
    if let Some(url) = panel {
        buttons.push(panel_button(url));
    }
    vec![
        serenity::CreateActionRow::SelectMenu(watched),
        serenity::CreateActionRow::SelectMenu(log),
        serenity::CreateActionRow::SelectMenu(role),
        serenity::CreateActionRow::SelectMenu(zone),
        serenity::CreateActionRow::Buttons(buttons),
    ]
}

/// The label of the how-to button: it names the channel, since the post
/// is public.
fn how_to_label(target: &Target) -> String {
    target.name.as_deref().map_or_else(
        || "Post a how-to".to_owned(),
        |name| clip(&format!("Post a how-to in #{name}"), BUTTON_LABEL_MAX_CHARS),
    )
}

/// The buttons under the saved summary. With `live` false only the link
/// is left: what stays once the form stops listening.
fn saved_rows(
    ids: &Ids,
    how_to: &HowTo,
    panel: Option<&str>,
    live: bool,
) -> Vec<serenity::CreateActionRow> {
    let mut buttons = Vec::new();
    if live {
        buttons.push(
            serenity::CreateButton::new(ids.of(Action::OpenGallery))
                .style(serenity::ButtonStyle::Primary)
                .label("Open gallery"),
        );
        if let HowTo::Offered(target) | HowTo::Failed(target, _) = how_to {
            buttons.push(
                serenity::CreateButton::new(ids.of(Action::HowTo))
                    .style(serenity::ButtonStyle::Secondary)
                    .label(how_to_label(target)),
            );
        }
    }
    if let Some(url) = panel {
        buttons.push(panel_button(url));
    }
    if buttons.is_empty() {
        Vec::new()
    } else {
        vec![serenity::CreateActionRow::Buttons(buttons)]
    }
}

/// The modal behind "Other timezone".
fn zone_modal(custom_id: String) -> serenity::CreateModal {
    serenity::CreateModal::new(custom_id, ZONE_MODAL_TITLE).components(vec![
        serenity::CreateActionRow::InputText(
            serenity::CreateInputText::new(
                serenity::InputTextStyle::Short,
                ZONE_INPUT_LABEL,
                ZONE_INPUT_ID,
            )
            .placeholder(ZONE_INPUT_PLACEHOLDER)
            .min_length(2)
            .max_length(ZONE_INPUT_MAX_CHARS)
            .required(true),
        ),
    ])
}

/// What was typed into the timezone field.
fn submitted_zone(data: &serenity::ModalInteractionData) -> &str {
    data.components
        .iter()
        .flat_map(|row| row.components.iter())
        .find_map(|component| match component {
            serenity::ActionRowComponent::InputText(input) if input.custom_id == ZONE_INPUT_ID => {
                input.value.as_deref()
            }
            _ => None,
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// The command
// ---------------------------------------------------------------------------

/// Set up leaf here: series channels, log channel, creator role and timezone.
#[poise::command(
    slash_command,
    guild_only,
    install_context = "Guild",
    required_permissions = "MANAGE_GUILD",
    default_member_permissions = "MANAGE_GUILD"
)]
pub async fn setup(ctx: Context<'_>) -> Result<(), Error> {
    let Some(guild_id) = checks::guild_id(&ctx).await? else {
        return Ok(());
    };
    let (Some(guild), poise::Context::Application(app)) = (ctx.guild_id(), ctx) else {
        return Ok(());
    };
    let data = ctx.data();
    let serenity_ctx = ctx.serenity_context();

    // Everything before the first reply is local: Discord allows it three
    // seconds.
    data.guilds.ensure_exists(&guild_id).await?;
    let opened = data
        .guilds
        .get(&guild_id)
        .await?
        .unwrap_or_else(|| GuildSettings::defaults_for(&guild_id));
    let series = data.series.list_by_guild(&guild_id).await?;
    let facts = guild_facts(&serenity_ctx.cache, guild);
    let (draft, stale) = Draft::open(&opened, facts.as_ref());
    let panel = ctx
        .data()
        .public_url
        .as_deref()
        .map(|url| admin_url(url, &guild_id));

    let now = Instant::now();
    let mut session = Session {
        http: &serenity_ctx.http,
        cache: &serenity_ctx.cache,
        data,
        guild,
        guild_id,
        admin: ctx.author().id,
        ids: Ids::new(app.interaction.id.get()),
        first_run: !opened.setup_complete,
        opened,
        draft,
        stale,
        facts,
        series,
        panel,
        prefill: Prefill::ALL,
        notice: None,
        saved_warnings: Vec::new(),
        stage: Stage::Form,
        surface: Some(app.interaction.token.clone()),
        surface_until: now + TOKEN_LIFE,
        deadline: now + IDLE,
        modal: None,
    };
    // Listening starts before the form is shown, so no press is missed.
    // From then until this command returns, the form's ids are its own;
    // after that the component router answers them as expired.
    let _listening = components::SESSIONS.listen(&session.ids.prefix);
    let events = incoming(
        &serenity_ctx.shard,
        session.admin,
        session.ids.prefix.clone(),
    );
    session.open(&ctx).await?;
    session.run(events).await;
    Ok(())
}

/// A press or a modal submit that belongs to one form.
enum Incoming {
    Press(Box<serenity::ComponentInteraction>),
    Submit(Box<serenity::ModalInteraction>),
}

/// Every press and modal submit of one form: the admin's, with the form's
/// id prefix.
fn incoming(
    shard: &serenity::ShardMessenger,
    user: serenity::UserId,
    prefix: String,
) -> impl Stream<Item = Incoming> + Send + use<> {
    serenity::collect(shard, move |event| {
        let serenity::Event::InteractionCreate(created) = event else {
            return None;
        };
        match &created.interaction {
            serenity::Interaction::Component(press)
                if press.user.id == user && press.data.custom_id.starts_with(&prefix) =>
            {
                Some(Incoming::Press(Box::new(press.clone())))
            }
            serenity::Interaction::Modal(submit)
                if submit.user.id == user && submit.data.custom_id.starts_with(&prefix) =>
            {
                Some(Incoming::Submit(Box::new(submit.clone())))
            }
            _ => None,
        }
    })
}

/// The interaction an update goes out on.
#[derive(Clone, Copy)]
enum Via<'i> {
    /// A menu or button: its message is updated in place.
    Press(&'i serenity::ComponentInteraction),
    /// The timezone modal: updates the message it was opened from.
    Submit(&'i serenity::ModalInteraction),
    /// No new interaction: edit the message already on screen.
    Edit,
}

/// One run of `/setup`, from the form to the last button.
struct Session<'a> {
    http: &'a serenity::Http,
    cache: &'a serenity::Cache,
    data: &'a Data,
    guild: serenity::GuildId,
    guild_id: String,
    admin: serenity::UserId,
    ids: Ids,
    /// Whether the server had never completed setup when the form opened.
    first_run: bool,
    /// The settings as stored when the form opened, then as last saved.
    opened: GuildSettings,
    draft: Draft,
    stale: Stale,
    /// The cached guild, read again on every press: the admin may be
    /// fixing a permission between two of them.
    facts: Option<GuildFacts>,
    /// Every series of the server, for the "still posts there" warning.
    series: Vec<Series>,
    /// The admin panel's address for this server.
    panel: Option<String>,
    /// What the form may pre-fill: everything, unless Discord refused it.
    prefill: Prefill,
    /// A line about the last press, shown until the next change.
    notice: Option<String>,
    /// The warnings as they stood at Save, kept for the summary.
    saved_warnings: Vec<String>,
    stage: Stage,
    /// Token that can edit the message on screen.
    surface: Option<String>,
    /// When `surface` stops working.
    surface_until: Instant,
    /// When the form stops listening: ten minutes after the last press.
    deadline: Instant,
    /// The timezone modal last opened. Discord sends nothing when a modal
    /// is dismissed, so this stays set until its submit or the next open.
    modal: Option<String>,
}

impl Session<'_> {
    /// Restarts the idle clock.
    fn touch(&mut self) {
        self.deadline = Instant::now() + IDLE;
    }

    /// Makes `token`, from an interaction just answered, the one that
    /// edits the message.
    fn set_surface(&mut self, token: String) {
        self.surface = Some(token);
        self.surface_until = Instant::now() + TOKEN_LIFE;
        self.touch();
    }

    // -- drawing ----------------------------------------------------------

    fn form(&self) -> (String, Vec<serenity::CreateActionRow>) {
        let now_unix = checks::now_unix();
        let facts = self.facts.as_ref();
        let warnings = warnings(
            &self.draft,
            &self.opened.watched_channels,
            &self.series,
            facts,
        );
        let text = form_text(&FormView {
            first_run: self.first_run,
            draft: &self.draft,
            stale: &self.stale,
            warnings: &warnings,
            notice: self.notice.as_deref(),
            plain: self.prefill.hides(&self.draft),
            has_link: self.panel.is_some(),
            now_unix,
        });
        let rows = form_rows(
            &self.ids,
            &self.draft,
            facts,
            self.panel.as_deref(),
            self.prefill,
            now_unix,
        );
        (clip(&text, MESSAGE_MAX_CHARS), rows)
    }

    /// Sends the form as the command's reply. If Discord refuses it as
    /// malformed (a saved channel or role it no longer accepts as a menu
    /// default), the form goes out once more with empty menus and no link:
    /// the list in the text still shows what is saved.
    async fn open(&mut self, ctx: &Context<'_>) -> Result<(), Error> {
        let reply = |(text, rows): (String, Vec<serenity::CreateActionRow>)| {
            poise::CreateReply::default()
                .content(text)
                .components(rows)
                .allowed_mentions(serenity::CreateAllowedMentions::new())
                .ephemeral(true)
        };
        match ctx.send(reply(self.form())).await {
            Ok(_) => Ok(()),
            Err(e) if is_bad_request(&e) => {
                tracing::warn!(
                    guild = %self.guild_id,
                    error = %e,
                    "setup form refused; sending it without pre-filled menus"
                );
                // The link goes too: its address is the other thing leaf
                // supplied.
                self.prefill = Prefill::NONE;
                self.panel = None;
                ctx.send(reply(self.form())).await?;
                Ok(())
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Puts `content` and `rows` on screen through `via`.
    async fn deliver(
        &mut self,
        via: Via<'_>,
        content: String,
        rows: Vec<serenity::CreateActionRow>,
    ) -> Result<(), serenity::Error> {
        // Private replies ping nobody, but they carry role mentions.
        let quiet = serenity::CreateAllowedMentions::new();
        let update = || {
            serenity::CreateInteractionResponse::UpdateMessage(
                serenity::CreateInteractionResponseMessage::new()
                    .content(content.clone())
                    .components(rows.clone())
                    .allowed_mentions(quiet.clone()),
            )
        };
        match via {
            Via::Press(press) => {
                press.create_response(self.http, update()).await?;
                self.set_surface(press.token.clone());
            }
            Via::Submit(submit) => {
                submit.create_response(self.http, update()).await?;
                self.set_surface(submit.token.clone());
            }
            Via::Edit => {
                let Some(token) = self.surface.clone() else {
                    return Ok(());
                };
                serenity::EditInteractionResponse::new()
                    .content(content)
                    .components(rows)
                    .allowed_mentions(quiet)
                    .execute(self.http, &token)
                    .await?;
            }
        }
        Ok(())
    }

    async fn show_form(&mut self, via: Via<'_>) -> Result<(), serenity::Error> {
        let (text, rows) = self.form();
        self.deliver(via, text, rows).await?;
        self.stage = Stage::Form;
        Ok(())
    }

    /// The saved summary and its buttons. With `live` false it is what
    /// stays once the form stops listening: the link, and no line that
    /// points at a button.
    fn summary(&self, how_to: &HowTo, live: bool) -> (String, Vec<serenity::CreateActionRow>) {
        let text = saved_text(
            &self.opened,
            self.first_run,
            &self.saved_warnings,
            how_to,
            live,
            self.panel.is_some(),
            checks::now_unix(),
        );
        let rows = saved_rows(&self.ids, how_to, self.panel.as_deref(), live);
        (clip(&text, MESSAGE_MAX_CHARS), rows)
    }

    /// Shows the saved summary as the stage has it.
    async fn show_saved(&mut self, via: Via<'_>) -> Result<(), serenity::Error> {
        let Stage::Saved(how_to) = &self.stage else {
            return Ok(());
        };
        let (text, rows) = self.summary(how_to, true);
        self.deliver(via, text, rows).await
    }

    /// Acknowledges a press that changes nothing.
    async fn dismiss(&mut self, via: Via<'_>) -> Result<(), Error> {
        let quiet = serenity::CreateInteractionResponse::Acknowledge;
        match via {
            Via::Press(press) => {
                press.create_response(self.http, quiet).await?;
                self.set_surface(press.token.clone());
            }
            Via::Submit(submit) => {
                submit.create_response(self.http, quiet).await?;
                self.set_surface(submit.token.clone());
            }
            Via::Edit => {}
        }
        Ok(())
    }

    // -- the loop ----------------------------------------------------------

    /// Handles presses and submits until the form is done or goes quiet.
    async fn run(mut self, events: impl Stream<Item = Incoming> + Send) {
        let mut events = std::pin::pin!(events);
        while self.stage != Stage::Closed {
            let surface_until = self.surface.is_some().then_some(self.surface_until);
            let (at, listen_on) = wake(self.deadline, surface_until, self.modal.is_some());
            let event = match tokio::time::timeout_at(at, events.next()).await {
                Ok(Some(event)) => event,
                Err(_) if listen_on => {
                    self.expire().await;
                    continue;
                }
                Ok(None) | Err(_) => break,
            };
            self.touch();
            self.facts = guild_facts(self.cache, self.guild);
            match event {
                Incoming::Press(press) => {
                    if let Err(e) = self.on_press(&press).await {
                        tracing::error!(error = format!("{e:#}"), "setup press failed");
                        self.report_failure(Via::Press(&press)).await;
                    }
                }
                Incoming::Submit(submit) => {
                    if let Err(e) = self.on_submit(&submit).await {
                        tracing::error!(error = format!("{e:#}"), "setup submit failed");
                        self.report_failure(Via::Submit(&submit)).await;
                    }
                }
            }
        }
        self.expire().await;
    }

    /// Closes what is on screen while its token still works: an unsaved
    /// form says nothing was saved, a saved summary loses the buttons that
    /// would no longer answer and the lines that point at them. The
    /// summary's text is sent again too, in case it never reached the
    /// screen after the write.
    async fn expire(&mut self) {
        let Some(token) = self.surface.take() else {
            return;
        };
        let (text, rows) = match &self.stage {
            Stage::Form => (closed_text(self.first_run, false), Vec::new()),
            Stage::Saved(how_to) => self.summary(how_to, false),
            Stage::Closed => return,
        };
        let edit = serenity::EditInteractionResponse::new()
            .content(text)
            .components(rows)
            .allowed_mentions(serenity::CreateAllowedMentions::new());
        if let Err(e) = edit.execute(self.http, &token).await {
            tracing::debug!(error = %e, "could not close the setup form");
        }
    }

    /// Tells the admin a press failed, if the press is still unanswered.
    async fn report_failure(&self, via: Via<'_>) {
        let reply = serenity::CreateInteractionResponse::Message(
            serenity::CreateInteractionResponseMessage::new()
                .content(STEP_FAILURE)
                .ephemeral(true),
        );
        let replied = match via {
            Via::Press(press) => press.create_response(self.http, reply).await,
            Via::Submit(submit) => submit.create_response(self.http, reply).await,
            Via::Edit => return,
        };
        if let Err(e) = replied {
            tracing::debug!(error = %e, "could not report a failed setup step");
        }
    }

    // -- events -----------------------------------------------------------

    async fn on_press(&mut self, press: &serenity::ComponentInteraction) -> Result<(), Error> {
        use serenity::ComponentInteractionDataKind as Kind;

        let via = Via::Press(press);
        let action = self
            .ids
            .action(&press.data.custom_id)
            .filter(|action| self.stage.allows(*action));
        match (action, &press.data.kind) {
            // A menu the admin picked in shows its picks from then on, even
            // on a form that went out without pre-filled menus: an empty
            // menu would make the next pick replace the list unseen.
            (Some(Action::Watched), Kind::ChannelSelect { values }) => {
                self.draft.watched = values.iter().map(ToString::to_string).collect();
                self.prefill.watched = true;
            }
            (Some(Action::Log), Kind::ChannelSelect { values }) => {
                self.draft.log_channel = values.first().map(ToString::to_string);
                self.prefill.log_channel = true;
            }
            (Some(Action::Role), Kind::RoleSelect { values }) => {
                // The @everyone role shares the server's id: no restriction.
                self.draft.creator_role = values
                    .first()
                    .filter(|role| role.get() != self.guild.get())
                    .map(ToString::to_string);
                self.prefill.creator_role = true;
            }
            (Some(Action::Zone), Kind::StringSelect { values }) => {
                let picked = values
                    .first()
                    .and_then(|name| series_ops::canonical_timezone(name));
                let Some(zone) = picked else {
                    return self.dismiss(via).await;
                };
                zone.clone_into(&mut self.draft.timezone);
            }
            (Some(Action::OtherZone), Kind::Button) => return self.open_zone_modal(press).await,
            (Some(Action::Save), Kind::Button) => return self.save(press).await,
            (Some(Action::Cancel), Kind::Button) => return self.cancel(press).await,
            (Some(Action::OpenGallery), Kind::Button) => return self.open_gallery(press).await,
            (Some(Action::HowTo), Kind::Button) => return self.post_how_to(press).await,
            _ => return self.dismiss(via).await,
        }
        // A menu changed the draft: redraw the list and its warnings.
        self.notice = None;
        self.show_form(via).await?;
        Ok(())
    }

    async fn on_submit(&mut self, submit: &serenity::ModalInteraction) -> Result<(), Error> {
        let via = Via::Submit(submit);
        let ours = self
            .modal
            .take_if(|open| *open == submit.data.custom_id)
            .is_some();
        if !ours || !matches!(self.stage, Stage::Form) {
            return self.dismiss(via).await;
        }
        let typed = submitted_zone(&submit.data);
        match resolve_zone(typed) {
            ZoneInput::Found(zone) => {
                zone.clone_into(&mut self.draft.timezone);
                self.notice = None;
            }
            outcome => {
                self.notice = Some(zone_problem_text(typed, &outcome, &self.draft.timezone));
            }
        }
        self.show_form(via).await?;
        Ok(())
    }

    async fn open_zone_modal(
        &mut self,
        press: &serenity::ComponentInteraction,
    ) -> Result<(), Error> {
        let custom_id = self.ids.modal(press.id.get());
        let modal = serenity::CreateInteractionResponse::Modal(zone_modal(custom_id.clone()));
        press.create_response(self.http, modal).await?;
        self.modal = Some(custom_id);
        Ok(())
    }

    async fn cancel(&mut self, press: &serenity::ComponentInteraction) -> Result<(), Error> {
        let text = closed_text(self.first_run, true);
        self.deliver(Via::Press(press), text, Vec::new()).await?;
        self.stage = Stage::Closed;
        Ok(())
    }

    /// Whether the admin may still manage the server, as this press reports
    /// it. If not, the press is answered by closing what is on screen.
    async fn still_admin(&mut self, press: &serenity::ComponentInteraction) -> Result<bool, Error> {
        let permissions = press.member.as_ref().and_then(|member| member.permissions);
        if may_manage(permissions) {
            return Ok(true);
        }
        tracing::info!(
            guild = %self.guild_id,
            admin = %self.admin,
            "setup closed: the admin no longer has Manage Server"
        );
        let text = demoted_text(matches!(self.stage, Stage::Saved(_)));
        self.deliver(Via::Press(press), text.to_owned(), Vec::new())
            .await?;
        self.stage = Stage::Closed;
        Ok(false)
    }

    // -- saving ------------------------------------------------------------

    async fn save(&mut self, press: &serenity::ComponentInteraction) -> Result<(), Error> {
        if !self.still_admin(press).await? {
            return Ok(());
        }
        if self.draft.watched.is_empty() {
            self.notice = Some(NO_CHANNEL_NOTICE.to_owned());
            self.show_form(Via::Press(press)).await?;
            return Ok(());
        }
        // Answered first: the log-channel test is a round trip to Discord.
        // With the menus gone a second tap has nothing to land on.
        self.deliver(Via::Press(press), SAVING.to_owned(), Vec::new())
            .await?;
        match self.commit().await {
            Ok(Ok(stored)) => {
                tracing::info!(
                    guild = %self.guild_id,
                    admin = %self.admin,
                    channels = stored.watched_channels.len(),
                    "setup saved"
                );
                // Measured against the list as it was, before it is replaced.
                self.saved_warnings = warnings(
                    &self.draft,
                    &self.opened.watched_channels,
                    &self.series,
                    self.facts.as_ref(),
                );
                let how_to = how_to_target(&stored.watched_channels, self.facts.as_ref())
                    .map_or(HowTo::NoChannel, HowTo::Offered);
                self.opened = stored;
                self.notice = None;
                self.modal = None;
                // The stage follows what is stored, whether or not the
                // summary reaches the screen: a form that closes later must
                // not say nothing was saved.
                self.stage = Stage::Saved(how_to);
                self.show_saved(Via::Edit).await?;
            }
            Ok(Err(refusal)) => {
                self.notice = Some(refusal);
                self.show_form(Via::Edit).await?;
            }
            Err(e) => {
                tracing::error!(
                    guild = %self.guild_id,
                    error = format!("{e:#}"),
                    "setup could not be saved"
                );
                self.notice = Some(SAVE_FAILED_NOTICE.to_owned());
                self.show_form(Via::Edit).await?;
            }
        }
        Ok(())
    }

    /// Checks a newly chosen log channel, then writes the draft. The inner
    /// `Err` is a notice for the form: the log channel refused its test
    /// line, and nothing was written.
    async fn commit(&mut self) -> Result<Result<GuildSettings, String>, Error> {
        let data = self.data;
        self.series = data.series.list_by_guild(&self.guild_id).await?;

        let stored_log = data
            .guilds
            .get(&self.guild_id)
            .await?
            .and_then(|stored| stored.log_channel_id);
        if let Some(log) = log_to_test(&self.draft, stored_log.as_deref()) {
            let Some(channel) = snowflake(log).map(serenity::ChannelId::new) else {
                return Ok(Err(log_refusal_text(log, Some(UNKNOWN_CHANNEL))));
            };
            let line = serenity::CreateMessage::new()
                .content(log_test_text(self.admin))
                .allowed_mentions(serenity::CreateAllowedMentions::new())
                .flags(serenity::MessageFlags::SUPPRESS_NOTIFICATIONS);
            if let Err(code) = self.send(channel, line).await {
                return Ok(Err(log_refusal_text(log, code)));
            }
        }

        let stored = store(&data.guilds, &self.guild_id, &self.draft, &self.opened).await?;
        Ok(Ok(stored))
    }

    /// Posts `message` in `channel`. The error is Discord's code, when it
    /// gave one.
    async fn send(
        &self,
        channel: serenity::ChannelId,
        message: serenity::CreateMessage,
    ) -> Result<serenity::Message, Option<isize>> {
        let sending = channel.send_message(self.http, message);
        match tokio::time::timeout(SEND_TIMEOUT, sending).await {
            Ok(Ok(sent)) => Ok(sent),
            Ok(Err(e)) => {
                let code = discord_code(&e);
                tracing::warn!(%channel, ?code, error = %e, "setup: message refused");
                Err(code)
            }
            Err(_) => {
                tracing::warn!(%channel, "setup: message timed out");
                Err(None)
            }
        }
    }

    // -- after saving ------------------------------------------------------

    /// Opens the gallery. Discord's launch response is the answer; if it
    /// is refused, the way through the app launcher is spelled out.
    async fn open_gallery(&self, press: &serenity::ComponentInteraction) -> Result<(), Error> {
        let launch = serenity::CreateInteractionResponse::LaunchActivity;
        if let Err(e) = press.create_response(self.http, launch).await {
            tracing::warn!(code = ?discord_code(&e), error = %e, "could not launch the Activity");
            let fallback = serenity::CreateInteractionResponse::Message(
                serenity::CreateInteractionResponseMessage::new()
                    .content(OPEN_GALLERY_FALLBACK)
                    .ephemeral(true),
            );
            press.create_response(self.http, fallback).await?;
        }
        Ok(())
    }

    /// Posts the public how-to in the first series channel leaf can post
    /// in. Nobody is pinged: mentions render without notifying.
    async fn post_how_to(&mut self, press: &serenity::ComponentInteraction) -> Result<(), Error> {
        let via = Via::Press(press);
        let Stage::Saved(HowTo::Offered(target) | HowTo::Failed(target, _)) = self.stage.clone()
        else {
            return self.dismiss(via).await;
        };
        let Some(channel) = snowflake(&target.id).map(serenity::ChannelId::new) else {
            return self.dismiss(via).await;
        };
        if !self.still_admin(press).await? {
            return Ok(());
        }
        // Answered first, with the button gone, so a second tap cannot
        // post the how-to twice.
        let offered = std::mem::replace(
            &mut self.stage,
            Stage::Saved(HowTo::Posting(target.clone())),
        );
        if let Err(e) = self.show_saved(via).await {
            self.stage = offered;
            return Err(e.into());
        }

        let message = serenity::CreateMessage::new()
            .content(clip(&how_to_text(&self.opened), MESSAGE_MAX_CHARS))
            .allowed_mentions(serenity::CreateAllowedMentions::new())
            .components(vec![serenity::CreateActionRow::Buttons(vec![
                open_gallery_button(None).style(serenity::ButtonStyle::Primary),
            ])]);
        let outcome = match self.send(channel, message).await {
            Ok(sent) => HowTo::Posted(format!(
                "https://discord.com/channels/{}/{channel}/{}",
                self.guild_id, sent.id
            )),
            Err(code) => HowTo::Failed(target, send_refusal(code)),
        };
        self.stage = Stage::Saved(outcome);
        self.show_saved(Via::Edit).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use leaf_core::domain::{Cadence, DetectionMode, Privacy};

    use super::*;

    /// 2025-07-01 12:00 UTC: daylight saving in the north.
    const JULY: i64 = 1_751_371_200;
    /// 2025-01-15 12:00 UTC: standard time in the north.
    const JANUARY: i64 = 1_736_942_400;

    const EVERYTHING: serenity::Permissions = SERIES_CHANNEL_NEEDS;

    fn text(name: &str, permissions: serenity::Permissions) -> ChannelFacts {
        ChannelFacts::new(name, serenity::ChannelType::Text, Some(permissions))
    }

    fn facts(channels: &[(&str, ChannelFacts)], roles: &[(&str, bool)]) -> GuildFacts {
        GuildFacts {
            channels: channels
                .iter()
                .map(|(id, channel)| ((*id).to_owned(), channel.clone()))
                .collect(),
            roles: roles
                .iter()
                .map(|(id, bot_role)| ((*id).to_owned(), *bot_role))
                .collect(),
        }
    }

    fn ids(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| (*id).to_owned()).collect()
    }

    fn draft(watched: &[&str]) -> Draft {
        Draft {
            watched: ids(watched),
            log_channel: None,
            creator_role: None,
            timezone: "UTC".to_owned(),
        }
    }

    fn settings(watched: &[&str]) -> GuildSettings {
        let mut settings = GuildSettings::defaults_for("900");
        settings.watched_channels = ids(watched);
        settings.setup_complete = !watched.is_empty();
        settings
    }

    fn series(name: &str, channels: &[&str], state: SeriesState) -> Series {
        Series {
            id: 1,
            guild_id: "900".into(),
            creator_id: "u".into(),
            name: name.into(),
            description: String::new(),
            channels: ids(channels),
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
            emoji: "🍃".into(),
            state,
            created_at: 0,
        }
    }

    fn view<'a>(draft: &'a Draft, stale: &'a Stale, warnings: &'a [String]) -> FormView<'a> {
        FormView {
            first_run: false,
            draft,
            stale,
            warnings,
            notice: None,
            plain: false,
            has_link: true,
            now_unix: JULY,
        }
    }

    // -- what leaf knows ----------------------------------------------------

    #[test]
    fn a_hidden_channel_grants_nothing_whatever_the_cache_computed() {
        let hidden = ChannelFacts::new(
            HIDDEN_CHANNEL_NAME,
            serenity::ChannelType::Text,
            Some(serenity::Permissions::all()),
        );
        assert_eq!(hidden.permissions, Some(serenity::Permissions::empty()));

        let known = facts(&[("1", hidden)], &[]);
        assert_eq!(footing(Some(&known), "1", POSTING_NEEDS), Footing::Unseen);
    }

    #[test]
    fn only_text_and_announcement_channels_are_offered() {
        use serenity::ChannelType as Kind;
        for (kind, selectable) in [
            (Kind::Text, true),
            (Kind::News, true),
            (Kind::Forum, false),
            (Kind::Voice, false),
            (Kind::PublicThread, false),
            (Kind::Category, false),
        ] {
            assert_eq!(
                ChannelFacts::new("c", kind, None).selectable,
                selectable,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn footing_tells_unseen_from_missing_from_unknown() {
        use serenity::Permissions as P;
        let known = facts(
            &[
                ("1", text("all", EVERYTHING)),
                ("2", text("no-view", P::SEND_MESSAGES | P::ADD_REACTIONS)),
                (
                    "3",
                    text("read-only", P::VIEW_CHANNEL | P::READ_MESSAGE_HISTORY),
                ),
                (
                    "4",
                    ChannelFacts::new("thread", serenity::ChannelType::PublicThread, None),
                ),
            ],
            &[],
        );
        let known = Some(&known);
        assert_eq!(footing(known, "1", SERIES_CHANNEL_NEEDS), Footing::Fine);
        // Without View Channel nothing else counts.
        assert_eq!(footing(known, "2", SERIES_CHANNEL_NEEDS), Footing::Unseen);
        assert_eq!(
            footing(known, "3", SERIES_CHANNEL_NEEDS),
            Footing::Missing(P::SEND_MESSAGES | P::ADD_REACTIONS)
        );
        // Only what is asked for is reported.
        assert_eq!(
            footing(known, "3", POSTING_NEEDS),
            Footing::Missing(P::SEND_MESSAGES)
        );
        // Permissions leaf cannot work out are not a finding.
        assert_eq!(footing(known, "4", POSTING_NEEDS), Footing::Fine);
        // Not in the server, or not shown to leaf.
        assert_eq!(footing(known, "5", POSTING_NEEDS), Footing::Unseen);
        // No cache: nothing can be said.
        assert_eq!(footing(None, "5", POSTING_NEEDS), Footing::Fine);
    }

    #[test]
    fn permissions_are_named_as_discord_names_them() {
        assert_eq!(
            permission_names(SERIES_CHANNEL_NEEDS),
            [
                "View Channel",
                "Send Messages",
                "Add Reactions",
                "Read Message History"
            ]
        );
        assert!(permission_names(serenity::Permissions::empty()).is_empty());
    }

    // -- the draft ----------------------------------------------------------

    #[test]
    fn the_form_opens_on_what_is_saved() {
        let mut saved = settings(&["1", "2"]);
        saved.log_channel_id = Some("3".into());
        saved.creator_role_id = Some("7".into());
        saved.timezone = "america/chicago".into();
        let known = facts(
            &[
                ("1", text("a", EVERYTHING)),
                ("2", text("b", EVERYTHING)),
                ("3", text("log", EVERYTHING)),
            ],
            &[("7", false)],
        );

        let (draft, stale) = Draft::open(&saved, Some(&known));
        assert_eq!(draft.watched, ids(&["1", "2"]));
        assert_eq!(draft.log_channel.as_deref(), Some("3"));
        assert_eq!(draft.creator_role.as_deref(), Some("7"));
        // Stored names are shown in their canonical spelling.
        assert_eq!(draft.timezone, "America/Chicago");
        assert_eq!(stale, Stale::default());
    }

    #[test]
    fn saved_values_that_point_at_nothing_are_dropped_and_reported() {
        let mut saved = settings(&["1", "gone", "2", "also-gone"]);
        saved.log_channel_id = Some("deleted-log".into());
        saved.creator_role_id = Some("deleted-role".into());
        saved.timezone = "Mars/Olympus".into();
        let known = facts(
            &[("1", text("a", EVERYTHING)), ("2", text("b", EVERYTHING))],
            &[],
        );

        let (draft, stale) = Draft::open(&saved, Some(&known));
        assert_eq!(draft.watched, ids(&["1", "2"]));
        assert_eq!(draft.log_channel, None);
        assert_eq!(draft.creator_role, None);
        assert_eq!(draft.timezone, "UTC");
        assert_eq!(
            stale,
            Stale {
                channels: 2,
                log_channel: true,
                creator_role: true,
                timezone: Some("Mars/Olympus".into()),
            }
        );

        let notes = stale_notes(&stale, &draft).join("\n");
        assert!(notes.contains("2 saved series channels no longer exist"));
        assert!(notes.contains("log is switched off when you save"));
        assert!(notes.contains("Saving lets anyone start one"));
        assert!(notes.contains("Mars/Olympus"));

        // Once the admin picks replacements, those notes have done their job.
        let mut replaced = draft;
        replaced.log_channel = Some("2".into());
        replaced.creator_role = Some("8".into());
        let notes = stale_notes(&stale, &replaced).join("\n");
        assert!(!notes.contains("log is switched off"));
        assert!(!notes.contains("anyone start one"));
    }

    #[test]
    fn without_the_cache_nothing_saved_is_dropped() {
        let mut saved = settings(&["1", "2"]);
        saved.log_channel_id = Some("3".into());
        saved.creator_role_id = Some("7".into());

        let (draft, stale) = Draft::open(&saved, None);
        assert_eq!(draft.watched, ids(&["1", "2"]));
        assert_eq!(draft.log_channel.as_deref(), Some("3"));
        assert_eq!(draft.creator_role.as_deref(), Some("7"));
        assert_eq!(stale, Stale::default());
    }

    #[test]
    fn menu_defaults_are_only_what_the_menu_can_show() {
        let known = facts(
            &[
                ("1", text("a", EVERYTHING)),
                (
                    "2",
                    ChannelFacts::new("forum", serenity::ChannelType::Forum, None),
                ),
                (
                    "3",
                    ChannelFacts::new("news", serenity::ChannelType::News, None),
                ),
            ],
            &[("7", false)],
        );
        // A forum, a deleted channel, a repeat and a non-id are left out.
        let saved = ids(&["1", "2", "3", "4", "1", "not-an-id", "0"]);
        assert_eq!(
            channel_defaults(&saved, Some(&known)),
            [serenity::ChannelId::new(1), serenity::ChannelId::new(3)]
        );
        // Without the cache every id that parses is tried.
        assert_eq!(
            channel_defaults(&saved, None),
            [1, 2, 3, 4].map(serenity::ChannelId::new)
        );

        // Never more defaults than the menu takes.
        let many: Vec<String> = (1..=40).map(|id| id.to_string()).collect();
        assert_eq!(
            channel_defaults(&many, None).len(),
            usize::from(SELECT_MAX_VALUES)
        );

        assert_eq!(
            role_default(Some("7"), Some(&known)),
            Some(serenity::RoleId::new(7))
        );
        assert_eq!(role_default(Some("8"), Some(&known)), None);
        assert_eq!(
            role_default(Some("8"), None),
            Some(serenity::RoleId::new(8))
        );
        assert_eq!(role_default(None, Some(&known)), None);
    }

    // -- timezones ----------------------------------------------------------

    #[test]
    fn common_zones_are_real_distinct_and_leave_room_for_one_more() {
        let mut seen = HashSet::new();
        for (name, places) in COMMON_ZONES {
            assert_eq!(
                series_ops::canonical_timezone(name),
                Some(name),
                "{name} is not a canonical timezone name"
            );
            assert!(seen.insert(name), "{name} is listed twice");
            assert!(!places.is_empty() && places.chars().count() <= 100);
        }
        assert!(COMMON_ZONES.len() < usize::from(SELECT_MAX_VALUES));
    }

    #[test]
    fn offsets_read_like_the_gallery() {
        let tz = |name: &str| localtime::parse_tz(name).unwrap();
        assert_eq!(offset_label(tz("UTC"), JULY), "UTC");
        assert_eq!(offset_label(tz("America/Chicago"), JULY), "UTC-5");
        assert_eq!(offset_label(tz("America/Chicago"), JANUARY), "UTC-6");
        assert_eq!(offset_label(tz("Asia/Kolkata"), JULY), "UTC+5:30");
        assert_eq!(offset_label(tz("Asia/Kathmandu"), JULY), "UTC+5:45");
        assert_eq!(offset_label(tz("Pacific/Auckland"), JANUARY), "UTC+13");
        // London is on UTC in winter, and says so.
        assert_eq!(offset_label(tz("Europe/London"), JANUARY), "UTC");

        assert_eq!(
            zone_text("America/Chicago", JULY),
            "America/Chicago (UTC-5)"
        );
        assert_eq!(zone_text("UTC", JULY), "UTC");
        assert_eq!(zone_text("Europe/London", JANUARY), "Europe/London (UTC)");
        assert_eq!(
            zone_text_with_clock("America/Chicago", JULY),
            "America/Chicago (UTC-5, 07:00 there now)"
        );
        assert_eq!(zone_text_with_clock("UTC", JULY), "UTC (12:00 there now)");
        // A name leaf cannot read is shown as it is.
        assert_eq!(zone_text("Mars/Olympus", JULY), "Mars/Olympus");
        assert_eq!(zone_text_with_clock("Mars/Olympus", JULY), "Mars/Olympus");
    }

    #[test]
    fn the_timezone_menu_always_shows_what_is_set() {
        let common = zone_options("Asia/Tokyo", JULY);
        assert_eq!(common.len(), COMMON_ZONES.len());
        let selected: Vec<&ZoneOption> = common.iter().filter(|zone| zone.selected).collect();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].value, "Asia/Tokyo");
        assert_eq!(selected[0].label, "Tokyo, Seoul");
        assert_eq!(selected[0].description, "Asia/Tokyo · UTC+9");

        // A zone from the modal (or the admin panel) leads the list.
        let other = zone_options("Europe/Lisbon", JULY);
        assert_eq!(other.len(), COMMON_ZONES.len() + 1);
        assert!(other.len() <= usize::from(SELECT_MAX_VALUES));
        assert_eq!(other[0].value, "Europe/Lisbon");
        assert_eq!(other[0].description, "Current choice · UTC+1");
        assert_eq!(other.iter().filter(|zone| zone.selected).count(), 1);

        for zone in &other {
            assert!(zone.label.chars().count() <= 100, "{}", zone.label);
            assert!(
                zone.description.chars().count() <= 100,
                "{}",
                zone.description
            );
        }
    }

    #[test]
    fn a_city_or_a_full_name_finds_the_timezone() {
        for (typed, expected) in [
            ("Europe/Paris", "Europe/Paris"),
            ("europe/paris", "Europe/Paris"),
            ("  Paris ", "Europe/Paris"),
            ("new york", "America/New_York"),
            ("America / New  York", "America/New_York"),
            ("tokyo", "Asia/Tokyo"),
            ("Ho Chi Minh", "Asia/Ho_Chi_Minh"),
            ("port-au-prince", "America/Port-au-Prince"),
            // A city wins over the legacy alias of the same name.
            ("singapore", "Asia/Singapore"),
            // Not `Etc/UTC`.
            ("utc", "UTC"),
            ("US/Central", "US/Central"),
            ("japan", "Japan"),
        ] {
            assert_eq!(resolve_zone(typed), ZoneInput::Found(expected), "{typed}");
        }

        for typed in ["", "   ", "/", "Mars/Olympus", "CST", "UTC+2", "nowhere"] {
            assert_eq!(resolve_zone(typed), ZoneInput::Unknown, "{typed:?}");
        }
    }

    #[test]
    fn a_city_with_an_old_and_a_new_name_is_one_timezone() {
        for (typed, expected) in [
            // The name the menu lists.
            ("Buenos Aires", "America/Argentina/Buenos_Aires"),
            ("istanbul", "Europe/Istanbul"),
            // Otherwise the fullest spelling.
            ("louisville", "America/Kentucky/Louisville"),
            ("Indianapolis", "America/Indiana/Indianapolis"),
            ("cordoba", "America/Argentina/Cordoba"),
            ("nicosia", "Europe/Nicosia"),
        ] {
            assert_eq!(resolve_zone(typed), ZoneInput::Found(expected), "{typed}");
        }
        // Typed in full, either name is taken as it is.
        assert_eq!(
            resolve_zone("america/louisville"),
            ZoneInput::Found("America/Louisville")
        );

        // Different timezones that share a last word stay a question, even
        // where their clocks agree today.
        assert_eq!(
            resolve_zone("central"),
            ZoneInput::Several(vec!["Canada/Central", "US/Central"])
        );
        assert_eq!(
            resolve_zone("west"),
            ZoneInput::Several(vec!["Australia/West", "Brazil/West"])
        );

        let tz = |name: &str| localtime::parse_tz(name).unwrap();
        assert!(same_zone(tz("Asia/Istanbul"), tz("Europe/Istanbul")));
        assert!(!same_zone(tz("America/Chicago"), tz("America/Winnipeg")));
        assert_eq!(one_zone(&[]), None);
        assert_eq!(one_zone(&["Asia/Tokyo"]), Some("Asia/Tokyo"));
    }

    #[test]
    fn an_abbreviation_without_daylight_saving_is_turned_away() {
        for typed in ["EST", "est", " Mst "] {
            assert_eq!(resolve_zone(typed), ZoneInput::NoDaylightSaving, "{typed}");
        }
        // Hawaii keeps no daylight saving, so its abbreviation is right.
        assert_eq!(resolve_zone("HST"), ZoneInput::Found("HST"));
        // These follow daylight saving.
        assert_eq!(resolve_zone("CET"), ZoneInput::Found("CET"));
        assert_eq!(resolve_zone("est5edt"), ZoneInput::Found("EST5EDT"));

        let notice = zone_problem_text("est", &ZoneInput::NoDaylightSaving, "UTC");
        assert!(notice.starts_with("⚠️ **est** is a fixed offset that never switches"));
        assert!(notice.contains("type a city instead (New York, Denver, Phoenix)"));
        assert!(notice.ends_with("The timezone is still **UTC**."));
        // The cities it names are ones leaf finds.
        for city in ["New York", "Denver", "Phoenix"] {
            assert!(matches!(resolve_zone(city), ZoneInput::Found(_)), "{city}");
        }
    }

    #[test]
    fn a_timezone_leaf_cannot_use_says_what_to_type_and_what_is_kept() {
        let unknown = zone_problem_text("Mars/**Olympus**", &ZoneInput::Unknown, "Asia/Tokyo");
        assert!(unknown.contains("doesn't know a timezone"));
        // The echo cannot break out of its bold.
        assert!(unknown.contains(r"Mars/\*\*Olympus\*\*"));
        assert!(unknown.contains("Europe/Paris"));
        assert!(unknown.ends_with("The timezone is still **Asia/Tokyo**."));

        let several = zone_problem_text("central", &resolve_zone("central"), "UTC");
        assert!(several.contains("`Canada/Central` and `US/Central`"));
        assert!(several.contains("type one of them in full"));

        let many = ZoneInput::Several(vec!["A/x", "B/x", "C/x", "D/x", "E/x", "F/x"]);
        assert!(zone_problem_text("x", &many, "UTC").contains("`D/x` and 2 more"));

        let empty = zone_problem_text("   ", &ZoneInput::Unknown, "UTC");
        assert!(empty.contains("field was empty"));
        assert!(!empty.contains("****"));

        let long = "x".repeat(200);
        assert!(
            zone_problem_text(&long, &ZoneInput::Unknown, "UTC")
                .chars()
                .count()
                < 300
        );
    }

    // -- warnings -----------------------------------------------------------

    #[test]
    fn channels_leaf_cannot_use_are_named_with_what_stops_working() {
        use serenity::Permissions as P;
        let known = facts(
            &[
                ("1", text("fine", EVERYTHING)),
                ("2", text("private", P::empty())),
                ("3", text("no-react", EVERYTHING - P::ADD_REACTIONS)),
                ("4", text("no-react-2", EVERYTHING - P::ADD_REACTIONS)),
                ("5", text("no-send", EVERYTHING - P::SEND_MESSAGES)),
            ],
            &[],
        );
        let lines = warnings(&draft(&["1", "2", "3", "4", "5"]), &[], &[], Some(&known));
        assert_eq!(
            lines,
            [
                "⚠️ leaf can't see <#2>. Posts there can still be archived, but leaf can't \
                 react, post milestones or send channel reminders there until its role is \
                 allowed to View Channel.",
                "⚠️ leaf is missing Add Reactions in <#3> and <#4>, so the reaction on archived \
                 posts won't work there.",
                "⚠️ leaf is missing Send Messages in <#5>, so milestone posts and channel \
                 reminders won't work there.",
            ]
        );

        // Nothing to say when all is granted, or when nothing is known.
        assert!(warnings(&draft(&["1"]), &[], &[], Some(&known)).is_empty());
        assert!(warnings(&draft(&["2", "9"]), &[], &[], None).is_empty());
    }

    #[test]
    fn a_log_channel_leaf_cannot_write_to_is_flagged() {
        use serenity::Permissions as P;
        let known = facts(
            &[
                ("1", text("a", EVERYTHING)),
                ("2", text("mute", P::VIEW_CHANNEL)),
                ("3", text("private", P::empty())),
            ],
            &[],
        );
        let mut draft = draft(&["1"]);
        draft.log_channel = Some("2".into());
        assert_eq!(
            warnings(&draft, &[], &[], Some(&known)),
            ["⚠️ leaf is missing Send Messages in <#2>, so it can't write the log there."]
        );
        draft.log_channel = Some("3".into());
        assert_eq!(
            warnings(&draft, &[], &[], Some(&known)),
            ["⚠️ leaf can't see <#3>, so it can't write the log there."]
        );
        // The log channel needs no reactions.
        draft.log_channel = Some("1".into());
        assert!(warnings(&draft, &[], &[], Some(&known)).is_empty());
    }

    #[test]
    fn dropping_a_channel_that_series_post_in_is_called_out() {
        let known = facts(
            &[
                ("1", text("a", EVERYTHING)),
                ("2", text("b", EVERYTHING)),
                ("3", text("c", EVERYTHING)),
            ],
            &[],
        );
        let all = [
            series("Daily *Art*", &["2"], SeriesState::Active),
            series("Sketches", &["2", "1"], SeriesState::Sprout),
            series("Taken down", &["3"], SeriesState::Revoked),
            series("Elsewhere", &["gone"], SeriesState::Active),
        ];
        let saved = ids(&["1", "2", "3", "gone"]);

        // 2 is dropped and two live series post there; 3 is dropped but only
        // a revoked series used it; "gone" no longer exists.
        let dropped = dropped_in_use(&saved, &ids(&["1"]), &all, Some(&known));
        assert_eq!(
            dropped,
            [("2".to_owned(), ids(&["Daily *Art*", "Sketches"]))]
        );

        let lines = warnings(&draft(&["1"]), &saved, &all, Some(&known));
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with(r"⚠️ **Daily \*Art\*** and **Sketches** post in <#2>"));
        assert!(lines[0].contains("Their posts there can't be archived"));
        assert!(lines[0].contains("Series settings"));

        // One series reads as one.
        let one = [series("Solo", &["2"], SeriesState::Active)];
        let lines = warnings(&draft(&["1"]), &saved, &one, Some(&known));
        assert!(lines[0].starts_with("⚠️ **Solo** posts in <#2>"));
        assert!(lines[0].contains("Its posts there"));

        // Keeping the channel, or never having had it, is no warning.
        assert!(warnings(&draft(&["1", "2"]), &saved, &one, Some(&known)).is_empty());
        assert!(warnings(&draft(&["1"]), &ids(&["1"]), &one, Some(&known)).is_empty());

        // Many series are counted, not listed.
        let many: Vec<Series> = (0..6)
            .map(|i| series(&format!("S{i}"), &["2"], SeriesState::Active))
            .collect();
        let lines = warnings(&draft(&["1"]), &saved, &many, Some(&known));
        assert!(lines[0].starts_with("⚠️ **S0**, **S1**, **S2** and 3 more post in <#2>"));
    }

    #[test]
    fn a_role_no_member_can_hold_is_flagged_first() {
        use serenity::Permissions as P;
        let known = facts(
            &[("1", text("a", P::empty()))],
            &[("7", true), ("8", false)],
        );
        let mut draft = draft(&["1"]);
        draft.creator_role = Some("7".into());
        let lines = warnings(&draft, &[], &[], Some(&known));
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("⚠️ <@&7> is a bot's own role, so no member can hold it"));

        draft.creator_role = Some("8".into());
        assert_eq!(warnings(&draft, &[], &[], Some(&known)).len(), 1);
    }

    #[test]
    fn only_a_bots_own_role_counts_as_one_no_member_can_hold() {
        let mut bot = serenity::Role::default();
        bot.managed = true;
        bot.tags.bot_id = Some(serenity::UserId::new(5));
        bot.tags.integration_id = Some(serenity::IntegrationId::new(6));
        assert!(is_bot_role(&bot));

        // Managed by Discord or an integration, and held by members.
        let mut booster = serenity::Role::default();
        booster.managed = true;
        booster.tags.premium_subscriber = true;
        assert!(!is_bot_role(&booster));
        let mut subscriber = serenity::Role::default();
        subscriber.managed = true;
        subscriber.tags.integration_id = Some(serenity::IntegrationId::new(6));
        assert!(!is_bot_role(&subscriber));
        assert!(!is_bot_role(&serenity::Role::default()));

        // So a booster role as the creator role is no warning.
        let known = facts(
            &[("1", text("a", EVERYTHING))],
            &[("9", is_bot_role(&booster))],
        );
        let mut draft = draft(&["1"]);
        draft.creator_role = Some("9".into());
        assert!(warnings(&draft, &[], &[], Some(&known)).is_empty());
    }

    #[test]
    fn long_lists_are_counted_not_spelled_out() {
        let many: Vec<String> = (1..=12).map(|id| id.to_string()).collect();
        let listed = channel_list(&many);
        assert!(listed.ends_with("<#8> and 4 more"), "{listed}");
        assert_eq!(channel_list(&ids(&["1"])), "<#1>");
        assert_eq!(channel_list(&ids(&["1", "2"])), "<#1> and <#2>");
    }

    #[test]
    fn a_message_with_too_much_to_say_counts_what_it_leaves_out() {
        let head = vec!["head".to_owned()];
        let tail = vec!["tail".to_owned()];
        assert_eq!(assemble(head.clone(), &[], tail.clone()), "head\ntail");
        let few = ids(&["one", "two"]);
        assert_eq!(
            assemble(head.clone(), &few, tail.clone()),
            "head\n\none\ntwo\ntail"
        );

        // Whole notes only, in order, and the head and tail always.
        let long: Vec<String> = (0..6).map(|i| format!("{i}{}", "x".repeat(499))).collect();
        let text = assemble(head.clone(), &long, tail.clone());
        assert!(text.chars().count() <= MESSAGE_MAX_CHARS);
        assert!(text.starts_with("head\n\n0x") && text.ends_with("\ntail"));
        assert!(text.contains("\n2x") && !text.contains("\n3x"));
        assert!(text.contains("⚠️ 3 more warnings don't fit in this message."));

        let text = assemble(head, &long[..4], tail);
        assert!(text.contains("⚠️ One more warning doesn't fit in this message."));
    }

    // -- the form's text ------------------------------------------------------

    #[test]
    fn the_form_lists_the_four_settings_in_menu_order() {
        let mut draft = draft(&["1", "2"]);
        draft.log_channel = Some("3".into());
        draft.creator_role = Some("7".into());
        draft.timezone = "America/Chicago".into();
        let text = form_text(&view(&draft, &Stale::default(), &[]));

        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {text}"))
        };
        assert!(text.starts_with("🍃 **leaf setup**"));
        assert!(text.contains("Nothing changes before that."));
        assert!(at("1. **Series channels**: <#1> <#2>") < at("2. **Log channel**: <#3>"));
        assert!(
            at("2. **Log channel**")
                < at("3. **Creator role**: <@&7>, so only members with it can start a series.")
        );
        assert!(
            at("3. **Creator role**")
                < at("4. **Timezone**: America/Chicago (UTC-5, 07:00 there now)")
        );
        assert!(text.ends_with("are in the **Admin panel**."));
    }

    #[test]
    fn an_empty_form_says_what_each_menu_is_for() {
        let draft = draft(&[]);
        let stale = Stale::default();
        let mut view = view(&draft, &stale, &[]);
        view.first_run = true;
        view.has_link = false;
        let text = form_text(&view);

        assert!(text.starts_with("🍃 **Set up leaf**"));
        assert!(text.contains("1. **Series channels**: none yet. Pick at least one"));
        assert!(text.contains("2. **Log channel**: none. Optional"));
        assert!(text.contains("3. **Creator role**: none, so anyone can start a series."));
        // A new server is nudged off the UTC default.
        assert!(text.contains("4. **Timezone**: UTC (12:00 there now). That is the default"));
        assert!(text.ends_with("your leaf address, then /admin."));
    }

    #[test]
    fn the_notice_comes_before_everything_else_under_the_list() {
        let draft = draft(&["1"]);
        let stale = Stale {
            channels: 1,
            ..Stale::default()
        };
        let warnings = vec!["⚠️ a warning".to_owned()];
        let mut view = view(&draft, &stale, &warnings);
        view.notice = Some("⚠️ the notice");
        view.plain = true;
        let text = form_text(&view);

        let at = |needle: &str| {
            text.find(needle)
                .unwrap_or_else(|| panic!("{needle}: {text}"))
        };
        assert!(at("4. **Timezone**") < at("⚠️ the notice"));
        assert!(at("⚠️ the notice") < at("One saved series channel no longer exists"));
        assert!(at("One saved series channel") < at("The menus couldn't be pre-filled"));
        assert!(at("The menus couldn't be pre-filled") < at("⚠️ a warning"));
    }

    #[test]
    fn the_fullest_form_fits_a_discord_message() {
        use serenity::Permissions as P;
        // 25 channels, each lacking something different where possible,
        // every channel of the old list dropped while series post there.
        let lacking = [
            P::empty(),
            EVERYTHING - P::ADD_REACTIONS,
            EVERYTHING - P::SEND_MESSAGES,
            EVERYTHING - P::READ_MESSAGE_HISTORY,
            EVERYTHING - P::ADD_REACTIONS - P::SEND_MESSAGES,
            EVERYTHING - P::ADD_REACTIONS - P::READ_MESSAGE_HISTORY,
            EVERYTHING - P::SEND_MESSAGES - P::READ_MESSAGE_HISTORY,
            P::VIEW_CHANNEL,
        ];
        let snowflake = |n: usize| (1_234_567_890_123_456_000 + n).to_string();
        let watched: Vec<String> = (0..25).map(snowflake).collect();
        let saved: Vec<String> = (100..110).map(snowflake).collect();
        let mut channels = HashMap::new();
        for (n, id) in watched.iter().enumerate() {
            channels.insert(id.clone(), text("c", lacking[n % lacking.len()]));
        }
        for id in &saved {
            channels.insert(id.clone(), text("old", EVERYTHING));
        }
        let known = GuildFacts {
            channels,
            roles: HashMap::from([(snowflake(7), true)]),
        };
        let all: Vec<Series> = saved
            .iter()
            .flat_map(|id| {
                (0..5)
                    .map(move |i| series(&"n".repeat(40 - i), &[id.as_str()], SeriesState::Active))
            })
            .collect();
        let draft = Draft {
            watched,
            log_channel: Some(snowflake(0)),
            creator_role: Some(snowflake(7)),
            timezone: "America/Argentina/Buenos_Aires".into(),
        };
        let stale = Stale {
            channels: 3,
            log_channel: false,
            creator_role: false,
            timezone: Some("x".repeat(80)),
        };
        let warnings = warnings(&draft, &saved, &all, Some(&known));
        assert!(warnings.len() > 15);

        let mut view = view(&draft, &stale, &warnings);
        view.plain = true;
        view.has_link = false;
        let notice = log_refusal_text(&snowflake(0), Some(MISSING_PERMISSIONS));
        view.notice = Some(&notice);
        let text = form_text(&view);
        assert!(
            text.chars().count() <= MESSAGE_MAX_CHARS,
            "{} chars",
            text.chars().count()
        );
        // What answers the last press is there, and so is the way out.
        assert!(text.contains(&notice));
        assert!(text.contains("more warnings don't fit in this message."));
        assert!(text.ends_with("your leaf address, then /admin."));

        let mut stored = settings(&[]);
        stored.watched_channels = draft.watched.clone();
        stored.log_channel_id = draft.log_channel.clone();
        stored.creator_role_id = draft.creator_role.clone();
        stored.timezone = draft.timezone.clone();
        stored.min_account_age_days = 30;
        stored.min_membership_age_days = 14;
        stored.sprout_enabled = true;
        let target = Target {
            id: snowflake(0),
            name: Some("c".into()),
        };
        let failed = HowTo::Failed(target, send_refusal(Some(MISSING_ACCESS)));
        let text = saved_text(&stored, true, &warnings, &failed, true, false, JULY);
        assert!(
            text.chars().count() <= MESSAGE_MAX_CHARS,
            "{} chars",
            text.chars().count()
        );
        // The summary keeps its last lines: the how-to and the way to the panel.
        assert!(text.contains("couldn't post the how-to"));
        assert!(text.ends_with("your leaf address, then /admin."));
        assert!(how_to_text(&stored).chars().count() <= MESSAGE_MAX_CHARS);
    }

    // -- after Save -----------------------------------------------------------

    #[test]
    fn the_summary_names_the_real_policy() {
        let mut stored = settings(&["1"]);
        assert_eq!(
            policy_text(&stored),
            "anyone in the server (up to 3 series each)"
        );

        stored.creator_role_id = Some("7".into());
        stored.max_series_per_user = 1;
        stored.min_account_age_days = 1;
        stored.min_membership_age_days = 30;
        assert_eq!(
            policy_text(&stored),
            "members with <@&7> (up to 1 series each; Discord account at least 1 day old; \
             a member here for at least 30 days)"
        );

        stored.max_series_per_user = 0;
        assert!(policy_text(&stored).starts_with("nobody: the limit is 0"));
    }

    #[test]
    fn the_summary_shows_what_was_stored_and_what_comes_next() {
        let mut stored = settings(&["1", "2"]);
        stored.log_channel_id = Some("3".into());
        stored.timezone = "Asia/Tokyo".into();
        let target = Target {
            id: "1".into(),
            name: Some("daily".into()),
        };
        let offered = HowTo::Offered(target.clone());

        let first = saved_text(&stored, true, &[], &offered, true, true, JULY);
        assert!(first.starts_with("🍃 **leaf is set up.**\n**Series channels**: <#1> <#2>\n"));
        assert!(first.contains("**Log channel**: <#3>"));
        assert!(first.contains("**Who can start a series**: anyone in the server"));
        assert!(first.contains("**Timezone**: Asia/Tokyo (UTC+9)"));
        assert!(first.contains("**Post a how-to** puts a short public guide in <#1>"));
        assert!(!first.contains("sprouts: only"));
        assert!(!first.contains("⚠️"));

        stored.log_channel_id = None;
        stored.sprout_enabled = true;
        let warnings = vec!["⚠️ careful".to_owned()];
        let again = saved_text(&stored, false, &warnings, &offered, true, true, JULY);
        assert!(again.starts_with("🍃 **Setup saved.**"));
        assert!(again.contains("**Log channel**: none"));
        assert!(again.contains("until 3 days are archived"));
        assert!(again.contains("⚠️ careful"));

        let posting = how_to_line(&HowTo::Posting(target.clone()), true);
        assert_eq!(posting, "⏳ Posting the how-to in <#1>…");
        let posted = how_to_line(
            &HowTo::Posted("https://discord.com/channels/9/1/5".into()),
            true,
        );
        assert_eq!(
            posted,
            "✅ The how-to is posted: https://discord.com/channels/9/1/5"
        );
        let failed = how_to_line(
            &HowTo::Failed(target, send_refusal(Some(MISSING_PERMISSIONS))),
            true,
        );
        assert!(failed.contains("couldn't post the how-to in <#1>"));
        assert!(failed.contains("allow its role to Send Messages"));
        assert!(failed.ends_with("Press the button to try again."));
        let nowhere = how_to_line(&HowTo::NoChannel, true);
        assert!(nowhere.contains("telling members is up to you"));
        assert!(nowhere.contains("Archive to Series"));
    }

    #[test]
    fn a_summary_that_stopped_listening_points_at_no_button() {
        let stored = settings(&["1"]);
        let target = Target {
            id: "1".into(),
            name: Some("daily".into()),
        };
        let rerun = "To post a public how-to for members, run `/setup` and press **Save** again.";

        let offered = HowTo::Offered(target.clone());
        let text = saved_text(&stored, true, &[], &offered, false, true, JULY);
        // The summary itself is all there.
        assert!(text.starts_with("🍃 **leaf is set up.**\n**Series channels**: <#1>\n"));
        assert!(text.contains(rerun));
        assert!(!text.contains("**Post a how-to**"));
        assert_eq!(how_to_line(&HowTo::Posting(target.clone()), false), rerun);

        let failed = HowTo::Failed(target, send_refusal(Some(MISSING_PERMISSIONS)));
        let line = how_to_line(&failed, false);
        assert!(line.starts_with("⚠️ leaf couldn't post the how-to in <#1>."));
        assert!(line.contains("allow its role to Send Messages"));
        assert!(line.ends_with("To try again, run `/setup` and press **Save**."));
        assert!(!line.contains("button"));

        // Lines that never pointed at a button read the same.
        for how_to in [HowTo::Posted("link".into()), HowTo::NoChannel] {
            assert_eq!(how_to_line(&how_to, false), how_to_line(&how_to, true));
        }
    }

    #[test]
    fn the_how_to_goes_to_the_first_channel_leaf_can_post_in() {
        use serenity::Permissions as P;
        let known = facts(
            &[
                ("1", text("private", P::empty())),
                ("2", text("read-only", P::VIEW_CHANNEL)),
                ("3", text("daily-art", POSTING_NEEDS)),
                ("4", text("later", EVERYTHING)),
            ],
            &[],
        );
        let watched = ids(&["1", "2", "3", "4"]);
        assert_eq!(
            how_to_target(&watched, Some(&known)),
            Some(Target {
                id: "3".into(),
                name: Some("daily-art".into())
            })
        );
        assert_eq!(how_to_target(&ids(&["1", "2"]), Some(&known)), None);
        // Without the cache the first channel is tried, unnamed.
        assert_eq!(
            how_to_target(&watched, None),
            Some(Target {
                id: "1".into(),
                name: None
            })
        );

        let named = Target {
            id: "3".into(),
            name: Some("daily-art".into()),
        };
        assert_eq!(how_to_label(&named), "Post a how-to in #daily-art");
        let unnamed = Target {
            id: "3".into(),
            name: None,
        };
        assert_eq!(how_to_label(&unnamed), "Post a how-to");
        let long = Target {
            id: "3".into(),
            name: Some("x".repeat(100)),
        };
        assert_eq!(how_to_label(&long).chars().count(), BUTTON_LABEL_MAX_CHARS);
    }

    #[test]
    fn the_public_how_to_teaches_the_gesture_and_names_who_can_start() {
        let mut stored = settings(&["1", "2"]);
        let open = how_to_text(&stored);
        assert!(open.starts_with("🍃 **leaf is ready in this server.**"));
        assert!(open.contains("press **Open gallery** below, then **Start a series**"));
        // A phone may land on a series page, where the button is an icon.
        assert!(open.contains("it is the **+** at the top"));
        assert!(open.contains("Anyone here can start one."));
        assert!(open.contains(
            "post your photo or video in <#1> or <#2>, then long-press the post (right-click \
             on desktop), then Apps, then Archive to Series."
        ));
        // A creator posts in one channel, so the list never reads as "post
        // in all of these".
        assert!(!open.contains("<#1> and <#2>"));
        assert_eq!(post_in_text(&ids(&["1"])), "<#1>");
        assert_eq!(
            post_in_text(&ids(&["1", "2", "3"])),
            "one of the series channels (<#1>, <#2>, <#3>)"
        );
        let many: Vec<String> = (1..=CHANNELS_SHOWN_MAX + 3)
            .map(|n| n.to_string())
            .collect();
        let listed = post_in_text(&many);
        assert!(listed.starts_with("one of the series channels (<#1>, <#2>, "));
        assert!(listed.ends_with(", 3 more)"));
        assert_eq!(listed.matches("<#").count(), CHANNELS_SHOWN_MAX);
        // The way in when the button does not launch.
        assert!(open.contains("tap + next to the message box, then Apps, then leaf"));

        stored.creator_role_id = Some("7".into());
        assert!(how_to_text(&stored).contains("Members with <@&7> can start one."));
    }

    #[test]
    fn the_public_how_to_states_the_age_rules_and_a_closed_door() {
        let mut stored = settings(&["1"]);
        stored.min_account_age_days = 30;
        assert_eq!(
            starters_text(&stored).unwrap(),
            "Anyone here can start one once their Discord account is at least 30 days old."
        );
        assert!(!how_to_text(&stored).contains("Anyone here can start one."));

        stored.creator_role_id = Some("7".into());
        stored.min_membership_age_days = 1;
        assert_eq!(
            starters_text(&stored).unwrap(),
            "Members with <@&7> can start one once their Discord account is at least 30 days \
             old and they have been a member of this server for at least 1 day."
        );
        assert!(how_to_text(&stored).contains(&starters_text(&stored).unwrap()));

        // A limit of 0 closes creation, whatever the other rules say.
        stored.max_series_per_user = 0;
        assert_eq!(starters_text(&stored), None);
        let closed = how_to_text(&stored);
        assert!(closed.contains(
            "**Start a series**: closed for now. This server's limit is 0 series per member.\n"
        ));
        assert!(!closed.contains("can start one"));
        // Archiving and browsing what exists still apply.
        assert!(closed.contains("**Archive a post**") && closed.contains("**Browse**"));
    }

    #[test]
    fn refusals_say_what_to_change() {
        assert!(send_refusal(Some(MISSING_ACCESS)).contains("View Channel"));
        assert!(send_refusal(Some(MISSING_PERMISSIONS)).contains("Send Messages"));
        assert!(send_refusal(Some(UNKNOWN_CHANNEL)).contains("no longer exists"));
        // An unfamiliar code, or no answer in time.
        assert!(send_refusal(Some(40_005)).ends_with("That may be temporary."));
        assert!(send_refusal(None).contains("or took too long to answer"));

        let refusal = log_refusal_text("3", Some(MISSING_PERMISSIONS));
        assert!(
            refusal.starts_with("⚠️ leaf couldn't post a test line in <#3>, so nothing was saved.")
        );
        assert!(refusal.contains("pick another log channel in the 2nd menu"));

        let line = log_test_text(serenity::UserId::new(42));
        assert!(line.contains("leaf's log channel"));
        assert!(line.ends_with("by <@42>."));
    }

    #[test]
    fn a_form_that_closes_unsaved_says_nothing_was_saved() {
        let timed_out = closed_text(true, false);
        assert!(timed_out.starts_with("⏳ Setup closed after 10 minutes without a change."));
        assert!(timed_out.contains("leaf still isn't set up here"));
        let cancelled = closed_text(false, true);
        assert!(cancelled.starts_with("Setup cancelled. Nothing was saved"));
        assert!(cancelled.contains("the settings are as they were"));
        assert_eq!(IDLE, Duration::from_mins(10));
        // The form closes while its token can still edit the message.
        assert!(IDLE < TOKEN_LIFE && TOKEN_LIFE < Duration::from_mins(15));
    }

    #[test]
    fn save_and_the_how_to_need_manage_server_at_the_press() {
        use serenity::Permissions as P;
        assert!(may_manage(Some(P::MANAGE_GUILD)));
        assert!(may_manage(Some(P::ADMINISTRATOR)));
        assert!(may_manage(Some(P::all())));
        assert!(!may_manage(Some(P::MANAGE_CHANNELS | P::SEND_MESSAGES)));
        assert!(!may_manage(Some(P::empty())));
        // Discord sends the permissions with every press in a server.
        assert!(!may_manage(None));

        assert!(demoted_text(false).contains("so nothing was saved"));
        assert!(demoted_text(true).contains("the how-to wasn't posted"));
        assert!(demoted_text(true).contains("The settings you saved are unchanged."));
    }

    // -- saving ------------------------------------------------------------------

    /// A migrated database in its own temp directory, removed on drop.
    struct TestDb {
        dir: std::path::PathBuf,
        guilds: GuildSettingsRepo,
    }

    impl TestDb {
        async fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let dir = std::env::temp_dir().join(format!(
                "leaf-bot-setup-{}-{}-{}",
                std::process::id(),
                checks::now_unix(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&dir).unwrap();
            let pool = leaf_core::db::connect(&dir.join("test.db")).await.unwrap();
            Self {
                dir,
                guilds: GuildSettingsRepo::new(pool),
            }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            let _removed = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[tokio::test]
    async fn a_first_save_completes_setup_and_stores_all_four() {
        let db = TestDb::new().await;
        db.guilds.ensure_exists("900").await.unwrap();
        let opened = db.guilds.get("900").await.unwrap().unwrap();
        assert!(!opened.setup_complete);

        let draft = Draft {
            watched: ids(&["1", "2"]),
            log_channel: Some("3".into()),
            creator_role: Some("7".into()),
            timezone: "Asia/Tokyo".into(),
        };
        let stored = store(&db.guilds, "900", &draft, &opened).await.unwrap();
        assert!(stored.setup_complete);
        assert_eq!(stored.watched_channels, ids(&["1", "2"]));
        assert_eq!(stored.log_channel_id.as_deref(), Some("3"));
        assert_eq!(stored.creator_role_id.as_deref(), Some("7"));
        assert_eq!(stored.timezone, "Asia/Tokyo");
        // What is returned is what a fresh read finds.
        assert_eq!(db.guilds.get("900").await.unwrap().unwrap(), stored);
        // The policies the form does not own keep their defaults.
        assert_eq!(stored.max_series_per_user, opened.max_series_per_user);
    }

    #[tokio::test]
    async fn a_save_does_not_undo_what_the_admin_panel_changed_meanwhile() {
        let db = TestDb::new().await;
        let mut saved = settings(&["1"]);
        saved.creator_role_id = Some("7".into());
        saved.timezone = "Europe/Paris".into();
        db.guilds.upsert(&saved).await.unwrap();

        // The form opens...
        let opened = db.guilds.get("900").await.unwrap().unwrap();
        let (mut draft, _) = Draft::open(&opened, None);

        // ...and while it is open the panel changes policies, the timezone
        // and the creator role.
        let mut panel = opened.clone();
        panel.timezone = "America/Chicago".into();
        panel.creator_role_id = Some("8".into());
        panel.max_series_per_user = 9;
        panel.min_account_age_days = 30;
        panel.sprout_enabled = true;
        db.guilds.upsert(&panel).await.unwrap();

        // The admin in chat only adds a channel and a log channel.
        draft.watched.push("2".into());
        draft.log_channel = Some("3".into());
        let stored = store(&db.guilds, "900", &draft, &opened).await.unwrap();

        assert_eq!(stored.watched_channels, ids(&["1", "2"]));
        assert_eq!(stored.log_channel_id.as_deref(), Some("3"));
        assert_eq!(stored.timezone, "America/Chicago");
        assert_eq!(stored.creator_role_id.as_deref(), Some("8"));
        assert_eq!(stored.max_series_per_user, 9);
        assert_eq!(stored.min_account_age_days, 30);
        assert!(stored.sprout_enabled);
    }

    #[tokio::test]
    async fn what_the_admin_changed_in_the_form_is_written() {
        let db = TestDb::new().await;
        let mut saved = settings(&["1"]);
        saved.log_channel_id = Some("3".into());
        saved.creator_role_id = Some("7".into());
        saved.timezone = "Europe/Paris".into();
        db.guilds.upsert(&saved).await.unwrap();
        let opened = db.guilds.get("900").await.unwrap().unwrap();

        // The role and the log channel are cleared, the timezone changed.
        let draft = Draft {
            watched: ids(&["2"]),
            log_channel: None,
            creator_role: None,
            timezone: "Asia/Kolkata".into(),
        };
        let stored = store(&db.guilds, "900", &draft, &opened).await.unwrap();
        assert_eq!(stored.watched_channels, ids(&["2"]));
        assert_eq!(stored.log_channel_id, None);
        assert_eq!(stored.creator_role_id, None);
        assert_eq!(stored.timezone, "Asia/Kolkata");
        assert!(stored.setup_complete);
    }

    #[test]
    fn only_a_new_log_channel_gets_a_test_line() {
        let mut draft = draft(&["1"]);
        assert_eq!(log_to_test(&draft, None), None);
        assert_eq!(log_to_test(&draft, Some("3")), None);
        draft.log_channel = Some("3".into());
        assert_eq!(log_to_test(&draft, None), Some("3"));
        assert_eq!(log_to_test(&draft, Some("4")), Some("3"));
        // Saving again with the same channel stays silent.
        assert_eq!(log_to_test(&draft, Some("3")), None);
    }

    #[test]
    fn a_quiet_form_closes_while_its_token_can_still_edit_it() {
        let now = Instant::now();
        let minutes = |n: u64| now + Duration::from_mins(n);

        // The usual case: the idle time ends first, and that is the end.
        assert_eq!(
            wake(minutes(10), Some(minutes(14)), false),
            (minutes(10), false)
        );
        assert_eq!(
            wake(minutes(10), Some(minutes(14)), true),
            (minutes(10), false)
        );
        // A modal opened late pushed the idle time past the token's life:
        // the message is closed at the token's end, and the form waits on
        // for the submit.
        assert_eq!(
            wake(minutes(19), Some(minutes(14)), true),
            (minutes(14), true)
        );
        // Past the token's life with no modal to wait for: just the end.
        assert_eq!(
            wake(minutes(19), Some(minutes(14)), false),
            (minutes(14), false)
        );
        // Already closed on screen: only the idle time is left.
        assert_eq!(wake(minutes(19), None, true), (minutes(19), false));
    }

    // -- ids, stages and rows -------------------------------------------------

    #[test]
    fn ids_round_trip_and_belong_to_one_form() {
        let form = Ids::new(u64::MAX);
        let other = Ids::new(1);
        for action in Action::ALL {
            let id = form.of(action);
            assert!(id.len() <= 100, "{id}");
            assert_eq!(form.action(&id), Some(action));
            assert_eq!(other.action(&id), None);
        }
        let modal = form.modal(u64::MAX);
        assert!(modal.len() <= 100, "{modal}");
        assert!(modal.starts_with(&form.prefix));
        // A modal id is not a button, and a fresh open gets a fresh id.
        assert_eq!(form.action(&modal), None);
        assert_ne!(modal, form.modal(7));

        assert_eq!(form.action("leaf:open"), None);
        assert_eq!(form.action(&format!("{}bogus", form.prefix)), None);
        // Never the stateless `leaf:` namespace the global router answers.
        assert!(!form.prefix.starts_with("leaf:"));
    }

    #[test]
    fn each_stage_takes_only_its_own_presses() {
        let target = Target {
            id: "1".into(),
            name: None,
        };
        let form_only = [
            Action::Watched,
            Action::Log,
            Action::Role,
            Action::Zone,
            Action::OtherZone,
            Action::Save,
            Action::Cancel,
        ];
        for action in Action::ALL {
            assert_eq!(Stage::Form.allows(action), form_only.contains(&action));
            assert!(!Stage::Closed.allows(action));
        }

        let offered = Stage::Saved(HowTo::Offered(target.clone()));
        let failed = Stage::Saved(HowTo::Failed(target.clone(), "why"));
        let posting = Stage::Saved(HowTo::Posting(target));
        let posted = Stage::Saved(HowTo::Posted("link".into()));
        let nowhere = Stage::Saved(HowTo::NoChannel);
        for stage in [&offered, &failed, &posting, &posted, &nowhere] {
            assert!(stage.allows(Action::OpenGallery));
            // A late tap on Save cannot save twice.
            assert!(!stage.allows(Action::Save));
            assert!(!stage.allows(Action::Watched));
        }
        assert!(offered.allows(Action::HowTo));
        assert!(failed.allows(Action::HowTo));
        // A second tap while posting, or after, cannot post again.
        assert!(!posting.allows(Action::HowTo));
        assert!(!posted.allows(Action::HowTo));
        assert!(!nowhere.allows(Action::HowTo));
    }

    /// The buttons of the last row.
    fn buttons(rows: &[serenity::CreateActionRow]) -> &[serenity::CreateButton] {
        match rows.last() {
            Some(serenity::CreateActionRow::Buttons(buttons)) => buttons,
            _ => panic!("the last row is not buttons"),
        }
    }

    #[test]
    fn the_form_is_four_menus_and_a_button_row() {
        let form = Ids::new(5);
        let known = facts(&[("1", text("a", EVERYTHING))], &[("7", false)]);
        let mut draft = draft(&["1"]);
        draft.log_channel = Some("1".into());
        draft.creator_role = Some("7".into());
        let url = "https://leaf.example/admin?guild=900";

        let rows = form_rows(&form, &draft, Some(&known), Some(url), Prefill::ALL, JULY);
        assert_eq!(rows.len(), 5);
        assert!(
            rows.iter()
                .take(4)
                .all(|row| matches!(row, serenity::CreateActionRow::SelectMenu(_)))
        );
        let kinds = Some(vec![
            serenity::ChannelType::Text,
            serenity::ChannelType::News,
        ]);
        // The first menu opens on the saved channels and takes up to 25.
        assert_eq!(
            rows[0],
            serenity::CreateActionRow::SelectMenu(
                serenity::CreateSelectMenu::new(
                    form.of(Action::Watched),
                    serenity::CreateSelectMenuKind::Channel {
                        channel_types: kinds.clone(),
                        default_channels: Some(vec![serenity::ChannelId::new(1)]),
                    },
                )
                .placeholder("Series channels: where posts can be archived from")
                .min_values(1)
                .max_values(25)
            )
        );
        // The log channel can be cleared: zero values are allowed.
        assert_eq!(
            rows[1],
            serenity::CreateActionRow::SelectMenu(
                serenity::CreateSelectMenu::new(
                    form.of(Action::Log),
                    serenity::CreateSelectMenuKind::Channel {
                        channel_types: kinds,
                        default_channels: Some(vec![serenity::ChannelId::new(1)]),
                    },
                )
                .placeholder("Log channel (optional): one quiet line per archive")
                .min_values(0)
                .max_values(1)
            )
        );
        assert_eq!(
            rows[2],
            serenity::CreateActionRow::SelectMenu(
                serenity::CreateSelectMenu::new(
                    form.of(Action::Role),
                    serenity::CreateSelectMenuKind::Role {
                        default_roles: Some(vec![serenity::RoleId::new(7)]),
                    },
                )
                .placeholder("Creator role (optional): leave empty and anyone can start a series")
                .min_values(0)
                .max_values(1)
            )
        );
        assert_eq!(
            buttons(&rows),
            [
                serenity::CreateButton::new(form.of(Action::Save))
                    .style(serenity::ButtonStyle::Success)
                    .label("Save")
                    .disabled(false),
                serenity::CreateButton::new(form.of(Action::OtherZone))
                    .style(serenity::ButtonStyle::Secondary)
                    .label("Other timezone"),
                serenity::CreateButton::new(form.of(Action::Cancel))
                    .style(serenity::ButtonStyle::Secondary)
                    .label("Cancel"),
                serenity::CreateButton::new_link(url).label("Admin panel"),
            ]
        );
    }

    #[test]
    fn save_is_disabled_until_a_series_channel_is_picked() {
        let form = Ids::new(5);
        let rows = form_rows(&form, &draft(&[]), None, None, Prefill::ALL, JULY);
        let save = serenity::CreateButton::new(form.of(Action::Save))
            .style(serenity::ButtonStyle::Success)
            .label("Save");
        assert_eq!(buttons(&rows).len(), 3);
        assert_eq!(buttons(&rows)[0], save.clone().disabled(true));
        // Nothing saved: the menus open empty, with their placeholders.
        assert_eq!(
            rows[2],
            serenity::CreateActionRow::SelectMenu(
                serenity::CreateSelectMenu::new(
                    form.of(Action::Role),
                    serenity::CreateSelectMenuKind::Role {
                        default_roles: None
                    },
                )
                .placeholder("Creator role (optional): leave empty and anyone can start a series")
                .min_values(0)
                .max_values(1)
            )
        );

        let rows = form_rows(&form, &draft(&["1"]), None, None, Prefill::ALL, JULY);
        assert_eq!(buttons(&rows)[0], save.disabled(false));
    }

    #[test]
    fn the_fallback_form_shows_only_what_the_admin_picked_since() {
        let form = Ids::new(5);
        let mut draft = draft(&["1"]);
        draft.log_channel = Some("2".into());
        draft.creator_role = Some("7".into());
        let channel_menu = |action: Action, min: u8, max: u8, shown: Option<u64>, hint: &str| {
            serenity::CreateActionRow::SelectMenu(
                serenity::CreateSelectMenu::new(
                    form.of(action),
                    serenity::CreateSelectMenuKind::Channel {
                        channel_types: Some(vec![
                            serenity::ChannelType::Text,
                            serenity::ChannelType::News,
                        ]),
                        default_channels: shown.map(|id| vec![serenity::ChannelId::new(id)]),
                    },
                )
                .placeholder(hint)
                .min_values(min)
                .max_values(max),
            )
        };
        let watched_hint = "Series channels: where posts can be archived from";
        let log_hint = "Log channel (optional): one quiet line per archive";

        // Refused by Discord: no defaults (and the session drops the link).
        let rows = form_rows(&form, &draft, None, None, Prefill::NONE, JULY);
        assert_eq!(
            rows[0],
            channel_menu(Action::Watched, 1, 25, None, watched_hint)
        );
        assert_eq!(rows[1], channel_menu(Action::Log, 0, 1, None, log_hint));
        assert_eq!(buttons(&rows).len(), 3);
        assert!(Prefill::NONE.hides(&draft));

        // A menu the admin has picked in since shows its picks again, or
        // the next pick would replace the list unseen.
        let touched = Prefill {
            watched: true,
            ..Prefill::NONE
        };
        let rows = form_rows(&form, &draft, None, None, touched, JULY);
        assert_eq!(
            rows[0],
            channel_menu(Action::Watched, 1, 25, Some(1), watched_hint)
        );
        assert_eq!(rows[1], channel_menu(Action::Log, 0, 1, None, log_hint));
        assert!(touched.hides(&draft));

        // The note about empty menus goes once nothing is hidden.
        assert!(!Prefill::ALL.hides(&draft));
        assert!(!Prefill::NONE.hides(&super::tests::draft(&[])));
    }

    #[test]
    fn summary_buttons_come_off_when_the_form_stops_listening() {
        let form = Ids::new(5);
        let url = "https://leaf.example/admin?guild=900";
        let target = Target {
            id: "1".into(),
            name: Some("daily".into()),
        };
        let open = serenity::CreateButton::new(form.of(Action::OpenGallery))
            .style(serenity::ButtonStyle::Primary)
            .label("Open gallery");
        let how_to = serenity::CreateButton::new(form.of(Action::HowTo))
            .style(serenity::ButtonStyle::Secondary)
            .label("Post a how-to in #daily");
        let link = serenity::CreateButton::new_link(url).label("Admin panel");

        let offered = HowTo::Offered(target.clone());
        let live = saved_rows(&form, &offered, Some(url), true);
        assert_eq!(buttons(&live), [open.clone(), how_to, link.clone()]);

        // Once posted (or while posting) the how-to button is gone.
        let posting = saved_rows(&form, &HowTo::Posting(target), Some(url), true);
        assert_eq!(buttons(&posting), [open.clone(), link.clone()]);
        let posted = saved_rows(&form, &HowTo::Posted("l".into()), None, true);
        assert_eq!(buttons(&posted), [open]);

        // After the form stops listening only the link, which needs no bot.
        let quiet = saved_rows(&form, &offered, Some(url), false);
        assert_eq!(buttons(&quiet), [link]);
        assert!(saved_rows(&form, &offered, None, false).is_empty());
    }

    #[test]
    fn the_admin_link_opens_on_this_server() {
        assert_eq!(
            admin_url("https://leaf.example", "900"),
            "https://leaf.example/admin?guild=900"
        );
        // An address saved with a trailing slash still makes one path.
        assert_eq!(
            admin_url("https://leaf.example/", "900"),
            "https://leaf.example/admin?guild=900"
        );
    }

    #[test]
    fn the_timezone_modal_fits_discords_limits() {
        // Discord: a modal title and a field label take 45 characters, a
        // placeholder 100.
        assert!(ZONE_MODAL_TITLE.chars().count() <= 45);
        assert!(ZONE_INPUT_LABEL.chars().count() <= 45);
        assert!(ZONE_INPUT_PLACEHOLDER.chars().count() <= 100);
        // Every name leaf knows can be typed in full.
        let longest = chrono_tz::TZ_VARIANTS
            .iter()
            .map(|tz| tz.name().len())
            .max()
            .unwrap();
        assert!(longest <= usize::from(ZONE_INPUT_MAX_CHARS));
    }
}
