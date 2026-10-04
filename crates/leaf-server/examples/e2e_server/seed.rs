//! What the e2e server starts with: two guilds, six people, six series and
//! the media behind them.
//!
//! Everything here is fixed (ids, names, post times), so a browser suite can
//! assert on it literally and a reset brings back exactly this. The server
//! reads the real clock, so nothing in the seed depends on "now"; the one
//! place that has to outlast it says so.

use anyhow::{Context as _, bail};
use bytes::Bytes;
use chrono::NaiveDate;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo, SqlitePool};
use leaf_core::domain::{
    Cadence, DetectionMode, GuildSettings, NewMediaAttachment, NewSeries, Post, Privacy,
    ReminderFailureKind, Series, SeriesState,
};
use leaf_core::localtime;
use leaf_core::media::{MediaMeta, original_key, thumb_key};
use leaf_server::api::auth::{CHANNEL_KIND_ANNOUNCEMENT, CHANNEL_KIND_TEXT};
use object_store::ObjectStore;
use object_store::path::Path as ObjectPath;
use serde_json::{Map, Value, json};

/// The Discord application id the server runs as. All digits, so the Activity
/// can be served from `<id>.discordsays.com`; on any other host it has to be
/// built with `VITE_DISCORD_CLIENT_ID` set to this.
pub const CLIENT_ID: &str = "800000000000000001";

/// The set-up guild's timezone: the gallery's calendar dates are in it.
pub const TIMEZONE: &str = "America/Chicago";

/// The guild that completed `/setup` and holds every series.
pub const MAIN_GUILD_ID: &str = "900000000000000001";
/// A guild leaf's bot has joined where nobody has run `/setup`.
pub const UNSET_GUILD_ID: &str = "900000000000000002";

const DAILY_SKETCH_CHANNEL_ID: &str = "200000000000000001";
const PHOTO_SHARE_CHANNEL_ID: &str = "200000000000000002";
/// Where the main guild's log lines go.
pub const LOG_CHANNEL_ID: &str = "200000000000000009";

/// The role the main guild requires to start a series.
const CREATOR_ROLE_ID: &str = "300000000000000001";
/// The role the role-gated series is limited to.
const PATRON_ROLE_ID: &str = "300000000000000002";
const MEMBER_ROLE_ID: &str = "300000000000000003";

/// Discord channel type: a voice channel (never offered by a picker).
const CHANNEL_KIND_VOICE: u8 = 2;

const DAY_SECS: i64 = 86_400;
const HOUR_SECS: i64 = 3_600;

/// When Day 1 of the long series was posted: 2025-11-13 18:00 UTC, noon in
/// the guild's timezone. Every other day of it is whole days after this, so
/// each lands in the middle of its local date.
pub const FIRST_POST_AT: i64 = 1_763_056_800;

/// A clock for the browser to be fixed at: 2026-05-20 15:00 UTC, the instant
/// the Activity's browser suites already use (`FIXED_NOW` in
/// `activity/e2e/support/env.ts`). Every post in the seed is before it; the
/// newest is 45 hours earlier.
const SUGGESTED_NOW: i64 = FIRST_POST_AT + 188 * DAY_SECS - 3 * HOUR_SECS;

/// When everyone but the newcomer joined the main guild: 2024-01-15.
const JOINED_LONG_AGO: i64 = 1_705_320_000;

/// When the newcomer "joined": 2099-01-01 12:00 UTC. The membership-age rule
/// is checked against the real clock, so only a join date ahead of every run
/// keeps the rule failing and the date it reports the same.
const NEWCOMER_JOINED_AT: i64 = 4_070_952_000;

/// Days someone must have been in the main guild before starting a series.
const MIN_MEMBERSHIP_AGE_DAYS: i64 = 7;

/// When the membership-age rule stops applying to the newcomer, as the
/// eligibility answer reports it.
pub const NEWCOMER_ELIGIBLE_AT: i64 = NEWCOMER_JOINED_AT + MIN_MEMBERSHIP_AGE_DAYS * DAY_SECS;

/// Archived days a sprout needs before it is published.
const SPROUT_THRESHOLD: i64 = 3;

/// Series each member may own. The creator owns five live ones, so there is
/// room to create more; an admin test can lower it to reach the limit.
const MAX_SERIES_PER_USER: i64 = 10;

/// A guild channel as the stub Discord lists it.
pub struct Channel {
    /// Name in the manifest.
    pub key: &'static str,
    /// Channel snowflake.
    pub id: &'static str,
    /// Channel name, without the `#`.
    pub name: &'static str,
    /// Discord's channel type.
    pub kind: u8,
    /// Position in the sidebar.
    pub position: i64,
}

/// A guild role as the stub Discord lists it.
pub struct Role {
    /// Name in the manifest.
    pub key: &'static str,
    /// Role snowflake.
    pub id: &'static str,
    /// Role name.
    pub name: &'static str,
    /// Position in the role list; higher sits nearer the top.
    pub position: i64,
}

/// A guild leaf's bot is in.
pub struct Guild {
    /// Name in the manifest (`main`, `unset`).
    pub key: &'static str,
    /// Guild snowflake.
    pub id: &'static str,
    /// Guild name.
    pub name: &'static str,
    /// Every channel of the guild.
    pub channels: &'static [Channel],
    /// Every role members can hold.
    pub roles: &'static [Role],
}

/// The guilds the stub Discord knows.
pub const GUILDS: &[Guild] = &[
    Guild {
        key: "main",
        id: MAIN_GUILD_ID,
        name: "Leaf Test Garden",
        channels: &[
            channel(
                "general",
                "200000000000000003",
                "general",
                CHANNEL_KIND_TEXT,
                0,
            ),
            channel(
                "daily_sketch",
                DAILY_SKETCH_CHANNEL_ID,
                "daily-sketch",
                CHANNEL_KIND_TEXT,
                1,
            ),
            channel(
                "photo_share",
                PHOTO_SHARE_CHANNEL_ID,
                "photo-share",
                CHANNEL_KIND_TEXT,
                2,
            ),
            channel(
                "announcements",
                "200000000000000004",
                "announcements",
                CHANNEL_KIND_ANNOUNCEMENT,
                3,
            ),
            channel(
                "voice",
                "200000000000000005",
                "Voice Lounge",
                CHANNEL_KIND_VOICE,
                4,
            ),
            channel("leaf_log", LOG_CHANNEL_ID, "leaf-log", CHANNEL_KIND_TEXT, 5),
        ],
        roles: &[
            role("artists", CREATOR_ROLE_ID, "Artists", 3),
            role("patrons", PATRON_ROLE_ID, "Patrons", 2),
            role("members", MEMBER_ROLE_ID, "Members", 1),
        ],
    },
    Guild {
        key: "unset",
        id: UNSET_GUILD_ID,
        name: "Unconfigured Server",
        channels: &[
            channel(
                "general",
                "210000000000000001",
                "general",
                CHANNEL_KIND_TEXT,
                0,
            ),
            channel("art", "210000000000000002", "art", CHANNEL_KIND_TEXT, 1),
        ],
        roles: &[role("regulars", "310000000000000001", "Regulars", 1)],
    },
];

const fn channel(
    key: &'static str,
    id: &'static str,
    name: &'static str,
    kind: u8,
    position: i64,
) -> Channel {
    Channel {
        key,
        id,
        name,
        kind,
        position,
    }
}

const fn role(key: &'static str, id: &'static str, name: &'static str, position: i64) -> Role {
    Role {
        key,
        id,
        name,
        position,
    }
}

/// The guild with this id, if the stub Discord knows it.
pub fn guild(id: &str) -> Option<&'static Guild> {
    GUILDS.iter().find(|g| g.id == id)
}

/// One person's place in one guild.
pub struct Membership {
    /// The guild.
    pub guild_id: &'static str,
    /// The roles they hold there.
    pub roles: &'static [&'static str],
    /// When they joined, unix seconds.
    pub joined_at: i64,
}

/// A Discord user the suites sign in as.
pub struct Persona {
    /// What the control routes and the `OAuth` code call them.
    pub key: &'static str,
    /// User snowflake.
    pub id: &'static str,
    /// Discord username.
    pub username: &'static str,
    /// Display name, in every guild.
    pub display_name: &'static str,
    /// The guilds they are a member of.
    pub memberships: &'static [Membership],
    /// The guilds they hold Manage Server in.
    pub manages: &'static [&'static str],
}

impl Persona {
    /// Their membership of `guild_id`, if they are in it.
    pub fn membership(&self, guild_id: &str) -> Option<&'static Membership> {
        self.memberships.iter().find(|m| m.guild_id == guild_id)
    }
}

const fn member(
    guild_id: &'static str,
    roles: &'static [&'static str],
    joined_at: i64,
) -> Membership {
    Membership {
        guild_id,
        roles,
        joined_at,
    }
}

/// The user id of the creator, who owns every seeded series.
const CREATOR_ID: &str = "100000000000000001";

/// Everyone the stub Discord knows.
pub const PERSONAS: &[Persona] = &[
    // Holds the creator role and owns every seeded series.
    Persona {
        key: "creator",
        id: CREATOR_ID,
        username: "mika",
        display_name: "Mika",
        memberships: &[
            member(
                MAIN_GUILD_ID,
                &[CREATOR_ROLE_ID, MEMBER_ROLE_ID],
                JOINED_LONG_AGO,
            ),
            member(UNSET_GUILD_ID, &[], JOINED_LONG_AGO),
        ],
        manages: &[],
    },
    // A plain member: sees what is public, may not start a series.
    Persona {
        key: "viewer",
        id: "100000000000000002",
        username: "sam",
        display_name: "Sam",
        memberships: &[
            member(MAIN_GUILD_ID, &[MEMBER_ROLE_ID], JOINED_LONG_AGO),
            member(UNSET_GUILD_ID, &[], JOINED_LONG_AGO),
        ],
        manages: &[],
    },
    // Manage Server in both guilds; in the gallery, a plain member.
    Persona {
        key: "admin",
        id: "100000000000000003",
        username: "avery",
        display_name: "Avery",
        memberships: &[
            member(MAIN_GUILD_ID, &[MEMBER_ROLE_ID], JOINED_LONG_AGO),
            member(UNSET_GUILD_ID, &[], JOINED_LONG_AGO),
        ],
        manages: &[MAIN_GUILD_ID, UNSET_GUILD_ID],
    },
    // A real Discord user who is in neither guild.
    Persona {
        key: "outsider",
        id: "100000000000000004",
        username: "olive",
        display_name: "Olive",
        memberships: &[],
        manages: &[],
    },
    // In the main guild with no role and too short a membership.
    Persona {
        key: "newcomer",
        id: "100000000000000005",
        username: "nico",
        display_name: "Nico",
        memberships: &[member(MAIN_GUILD_ID, &[], NEWCOMER_JOINED_AT)],
        manages: &[],
    },
    // Holds the role the role-gated series is limited to.
    Persona {
        key: "patron",
        id: "100000000000000006",
        username: "pat",
        display_name: "Pat",
        memberships: &[member(
            MAIN_GUILD_ID,
            &[PATRON_ROLE_ID, MEMBER_ROLE_ID],
            JOINED_LONG_AGO,
        )],
        manages: &[],
    },
];

/// The persona called `key`.
pub fn persona(key: &str) -> Option<&'static Persona> {
    PERSONAS.iter().find(|p| p.key == key)
}

/// The persona with this user id.
pub fn persona_by_id(id: &str) -> Option<&'static Persona> {
    PERSONAS.iter().find(|p| p.id == id)
}

/// One stored file and its thumbnail, as committed under `fixtures/` (see
/// `fixtures/generate.sh`).
pub struct Fixture {
    /// The content type the attachment is archived under.
    pub content_type: &'static str,
    /// The original's bytes.
    pub original: &'static [u8],
    /// Its `WebP` thumbnail's bytes.
    pub thumb: &'static [u8],
}

macro_rules! fixture {
    ($name:literal, $content_type:literal, $thumb:literal) => {
        Fixture {
            content_type: $content_type,
            original: include_bytes!(concat!("fixtures/", $name)),
            thumb: include_bytes!(concat!("fixtures/", $thumb)),
        }
    };
}

/// The stills, one per shape (640×480, 480×640, 512×512, 800×400, 360×720).
pub static STILLS: [Fixture; 5] = [
    fixture!("landscape.png", "image/png", "landscape.thumb.webp"),
    fixture!("portrait.jpg", "image/jpeg", "portrait.thumb.webp"),
    fixture!("square.png", "image/png", "square.thumb.webp"),
    fixture!("wide.jpg", "image/jpeg", "wide.thumb.webp"),
    fixture!("tall.png", "image/png", "tall.thumb.webp"),
];

/// Four seconds of H.264 + AAC, index first, about 150 KB.
pub static MP4: Fixture = fixture!("clip.mp4", "video/mp4", "clip.mp4.thumb.webp");

/// The same clip as VP9 + Opus, for a browser built without H.264.
pub static WEBM: Fixture = fixture!("clip.webm", "video/webm", "clip.webm.thumb.webp");

/// The still a day shows when nothing else is planned for it; cycles
/// through all five so neighbouring tiles differ.
pub fn still(n: i64) -> &'static Fixture {
    let [landscape, portrait, square, wide, tall] = &STILLS;
    match n.rem_euclid(5) {
        0 => landscape,
        1 => portrait,
        2 => square,
        3 => wide,
        _ => tall,
    }
}

/// One media row of a planned day.
#[derive(Clone, Copy)]
pub enum Media {
    /// A file in the store, with its thumbnail.
    Stored(&'static Fixture),
    /// A row with no file behind it, as an import leaves when the source
    /// message's attachment could no longer be downloaded.
    Missing,
}

/// A day to archive: what `archive` writes for it.
pub struct PlannedDay {
    /// Day number.
    pub day: i64,
    /// When the source message was posted, unix seconds.
    pub posted_at: i64,
    /// The message text.
    pub caption: String,
    /// Its attachments, in order.
    pub media: Vec<Media>,
}

// --- the long series --------------------------------------------------------
//
// "Daily Sketch": Day N was posted N-1 days after 2025-11-13, through Day 187
// on 2026-05-18. 150 of those days are archived; the rest were never posted.

/// The newest day of the long series.
const LONG_LAST_DAY: i64 = 187;

/// Day numbers of the long series that were never archived, as inclusive
/// spans. The third is every date of February 2026: a month with no tile.
const LONG_GAPS: [(i64, i64); 4] = [(30, 32), (60, 60), (81, 108), (150, 154)];

/// The calendar month those missing days leave empty.
const LONG_EMPTY_MONTH: &str = "2026-02";

/// A day with three images.
const LONG_THREE_IMAGES_DAY: i64 = 20;
/// A day that is an MP4 video.
const LONG_MP4_DAY: i64 = 66;
/// A day whose only media row has no file.
const LONG_MISSING_MEDIA_DAY: i64 = 75;
/// A day that is a `WebM` video.
const LONG_WEBM_DAY: i64 = 120;
/// A day with no caption.
const LONG_NO_CAPTION_DAY: i64 = 12;
/// A day with a caption that has to wrap, with a URL that cannot.
const LONG_WORDY_DAY: i64 = 145;

/// A day posted late on the evening of the day before it: 03:30 UTC on its
/// own nominal date, which is 21:30 the previous day in the guild's
/// timezone. It shares that calendar date with the day before it, and only
/// in the guild's timezone: in UTC the two fall on different dates.
const LONG_LATE_DAY: i64 = 34;
const LONG_LATE_OFFSET_SECS: i64 = 9 * HOUR_SECS + HOUR_SECS / 2;

const WORDY_CAPTION: &str = "ran out of daylight, so this one was drawn under the kitchen lamp \
    with the good pencil worn down to a stub. Reference photo: \
    https://example.com/references/kitchen-window/2026-04-06/late-evening-light-full-resolution";

const fn long_posted_at(day: i64) -> i64 {
    if day == LONG_LATE_DAY {
        return FIRST_POST_AT + (day - 2) * DAY_SECS + LONG_LATE_OFFSET_SECS;
    }
    FIRST_POST_AT + (day - 1) * DAY_SECS
}

fn long_caption(day: i64) -> String {
    match day {
        LONG_NO_CAPTION_DAY => String::new(),
        LONG_WORDY_DAY => format!("Day {day}: {WORDY_CAPTION}"),
        _ if day % 7 == 0 => format!("Day {day}: another week of pencil shavings."),
        _ => format!("Day {day}"),
    }
}

fn long_media(day: i64) -> Vec<Media> {
    match day {
        LONG_THREE_IMAGES_DAY => (0..3).map(|n| Media::Stored(still(n))).collect(),
        LONG_MP4_DAY => vec![Media::Stored(&MP4)],
        LONG_WEBM_DAY => vec![Media::Stored(&WEBM)],
        LONG_MISSING_MEDIA_DAY => vec![Media::Missing],
        _ => vec![Media::Stored(still(day))],
    }
}

fn long_days() -> Vec<PlannedDay> {
    (1..=LONG_LAST_DAY)
        .filter(|day| {
            !LONG_GAPS
                .iter()
                .any(|&(first, last)| (first..=last).contains(day))
        })
        .map(|day| PlannedDay {
            day,
            posted_at: long_posted_at(day),
            caption: long_caption(day),
            media: long_media(day),
        })
        .collect()
}

// --- the other series -------------------------------------------------------

/// 18:00 UTC on a date of 2026: the middle of that date in the guild's
/// timezone.
fn on(month: u32, day: u32) -> anyhow::Result<i64> {
    NaiveDate::from_ymd_opt(2026, month, day)
        .and_then(|date| date.and_hms_opt(18, 0, 0))
        .map(|at| at.and_utc().timestamp())
        .with_context(|| format!("2026-{month:02}-{day:02} is not a date"))
}

/// Days 1, 2, … posted on `dates` of 2026, one still each.
fn days_on(dates: &[(u32, u32)]) -> anyhow::Result<Vec<PlannedDay>> {
    (1_i64..)
        .zip(dates)
        .map(|(day, &(month, date))| {
            Ok(PlannedDay {
                day,
                posted_at: on(month, date)?,
                caption: format!("Day {day}"),
                media: vec![Media::Stored(still(day))],
            })
        })
        .collect()
}

/// How one seeded series is created.
struct SeriesSpec {
    key: &'static str,
    name: &'static str,
    description: &'static str,
    emoji: &'static str,
    cadence: Cadence,
    privacy: Privacy,
    privacy_role_id: Option<&'static str>,
    state: SeriesState,
    channel_id: &'static str,
}

/// The series, in the order they are created: on a fresh database their ids
/// are 1 to 6.
const SERIES: [SeriesSpec; 6] = [
    SeriesSpec {
        key: "long",
        name: "Daily Sketch",
        description: "One drawing a day, rain or shine.",
        emoji: "✏️",
        cadence: Cadence::Daily,
        privacy: Privacy::Public,
        privacy_role_id: None,
        state: SeriesState::Active,
        channel_id: DAILY_SKETCH_CHANNEL_ID,
    },
    // Two days archived of the three a sprout needs: only its creator sees it.
    SeriesSpec {
        key: "sprout",
        name: "Morning Coffee",
        description: "My cup, every single morning.",
        emoji: "☕",
        cadence: Cadence::Daily,
        privacy: Privacy::Public,
        privacy_role_id: None,
        state: SeriesState::Sprout,
        channel_id: PHOTO_SHARE_CHANNEL_ID,
    },
    SeriesSpec {
        key: "role_gated",
        name: "Patron Studies",
        description: "Longer pieces for the people who keep the lights on.",
        emoji: "🎨",
        cadence: Cadence::Weekly,
        privacy: Privacy::RoleGated,
        privacy_role_id: Some(PATRON_ROLE_ID),
        state: SeriesState::Active,
        channel_id: PHOTO_SHARE_CHANNEL_ID,
    },
    SeriesSpec {
        key: "creator_only",
        name: "Private Notes",
        description: "",
        emoji: "📓",
        cadence: Cadence::Freeform,
        privacy: Privacy::CreatorOnly,
        privacy_role_id: None,
        state: SeriesState::Active,
        channel_id: DAILY_SKETCH_CHANNEL_ID,
    },
    // Revoked by an admin: listed for its creator only, and it cannot be opened.
    SeriesSpec {
        key: "revoked",
        name: "Old Polaroids",
        description: "A shoebox, one print at a time.",
        emoji: "📸",
        cadence: Cadence::Freeform,
        privacy: Privacy::Public,
        privacy_role_id: None,
        state: SeriesState::Revoked,
        channel_id: PHOTO_SHARE_CHANNEL_ID,
    },
    // Reminders on, and the last one could not be delivered.
    SeriesSpec {
        key: "reminder",
        name: "Evening Walks",
        description: "Where my feet took me this week.",
        emoji: "🌆",
        cadence: Cadence::Weekly,
        privacy: Privacy::Public,
        privacy_role_id: None,
        state: SeriesState::Active,
        channel_id: PHOTO_SHARE_CHANNEL_ID,
    },
];

/// The key of the series whose reminders fail.
const REMINDER_SERIES: &str = "reminder";
/// When that series reminds its creator, in the guild's timezone.
const REMINDER_TIME: &str = "18:30";

fn days_of(key: &str) -> anyhow::Result<Vec<PlannedDay>> {
    match key {
        "long" => Ok(long_days()),
        "sprout" => days_on(&[(5, 16), (5, 17)]),
        "role_gated" => days_on(&[(4, 13), (4, 20), (4, 27), (5, 4), (5, 11)]),
        "creator_only" => days_on(&[(4, 25), (5, 2), (5, 9)]),
        "revoked" => days_on(&[(1, 10), (1, 11), (1, 12), (1, 13)]),
        REMINDER_SERIES => days_on(&[(4, 12), (4, 19), (4, 26), (5, 3), (5, 10), (5, 17)]),
        other => bail!("no days planned for series {other:?}"),
    }
}

/// What [`seed`] wrote, as it is stored: what the manifest is made from.
pub struct Seeded {
    /// The settings row of each of [`GUILDS`], in that order.
    pub settings: Vec<GuildSettings>,
    /// The series, in creation order.
    pub series: Vec<SeededSeries>,
}

/// A series as it was seeded.
pub struct SeededSeries {
    /// What the control routes call it.
    pub key: &'static str,
    /// The row as stored.
    pub series: Series,
    /// How many days were archived.
    pub archived_days: usize,
    /// Its highest day number.
    pub last_day: Option<i64>,
}

/// Fills an empty, migrated database and an empty store with the seed, and
/// returns what was stored.
pub async fn seed(pool: &SqlitePool, store: &dyn ObjectStore) -> anyhow::Result<Seeded> {
    let guilds = GuildSettingsRepo::new(pool.clone());
    guilds
        .upsert(&GuildSettings {
            setup_complete: true,
            timezone: TIMEZONE.to_owned(),
            watched_channels: vec![
                DAILY_SKETCH_CHANNEL_ID.to_owned(),
                PHOTO_SHARE_CHANNEL_ID.to_owned(),
            ],
            log_channel_id: Some(LOG_CHANNEL_ID.to_owned()),
            creator_role_id: Some(CREATOR_ROLE_ID.to_owned()),
            max_series_per_user: MAX_SERIES_PER_USER,
            min_membership_age_days: MIN_MEMBERSHIP_AGE_DAYS,
            sprout_enabled: true,
            sprout_threshold: SPROUT_THRESHOLD,
            ..GuildSettings::defaults_for(MAIN_GUILD_ID)
        })
        .await?;
    // What the bot writes when it joins a guild: a row, with setup still to do.
    guilds.ensure_exists(UNSET_GUILD_ID).await?;
    // Read back rather than repeated from the constants above, so the
    // manifest cannot say something the database does not.
    let mut settings = Vec::with_capacity(GUILDS.len());
    for guild in GUILDS {
        let stored = guilds.get(guild.id).await?;
        settings.push(stored.with_context(|| format!("guild {} was not seeded", guild.id))?);
    }

    let series_repo = SeriesRepo::new(pool.clone());
    let posts = PostRepo::new(pool.clone());
    let mut seeded = Vec::with_capacity(SERIES.len());
    for spec in &SERIES {
        let series = create_series(&series_repo, spec).await?;
        let days = days_of(spec.key)?;
        archive(&posts, store, &series, &days).await?;
        seeded.push(SeededSeries {
            key: spec.key,
            series,
            archived_days: days.len(),
            last_day: days.iter().map(|d| d.day).max(),
        });
    }
    Ok(Seeded {
        settings,
        series: seeded,
    })
}

async fn create_series(repo: &SeriesRepo, spec: &SeriesSpec) -> anyhow::Result<Series> {
    // The day before the first post of the seed.
    let created_at = FIRST_POST_AT - DAY_SECS;
    let mut series = repo
        .create(
            &NewSeries {
                guild_id: MAIN_GUILD_ID.to_owned(),
                creator_id: CREATOR_ID.to_owned(),
                name: spec.name.to_owned(),
                description: spec.description.to_owned(),
                channels: vec![spec.channel_id.to_owned()],
                cadence: spec.cadence,
                detection_mode: DetectionMode::ContextMenu,
                privacy: spec.privacy,
                privacy_role_id: spec.privacy_role_id.map(ToOwned::to_owned),
                start_day: 1,
                state: spec.state,
            },
            created_at,
        )
        .await?;

    spec.emoji.clone_into(&mut series.emoji);
    if spec.key == REMINDER_SERIES {
        series.reminder_enabled = true;
        series.reminder_time = Some(REMINDER_TIME.to_owned());
        series.reminder_dm = true;
    }
    repo.update(&series).await?;
    if spec.key == REMINDER_SERIES {
        // After the update: changing the delivery route clears a failure.
        let failed_at = on(5, 17)? + 6 * HOUR_SECS + HOUR_SECS / 2;
        repo.set_reminder_error(
            series.id,
            Some(ReminderFailureKind::DmClosed.as_str()),
            failed_at,
        )
        .await?;
    }
    Ok(series)
}

/// Archives `days` into `series`: the rows through [`PostRepo`], the files
/// into `store` under the keys the media pipeline would have used. Fails
/// if any of the days is already archived.
pub async fn archive(
    posts: &PostRepo,
    store: &dyn ObjectStore,
    series: &Series,
    days: &[PlannedDay],
) -> anyhow::Result<()> {
    let channel_id = series.channels.first().cloned().unwrap_or_default();
    let mut rows = Vec::with_capacity(days.len());
    for planned in days {
        let message_id = format!("60{:03}{:06}00", series.id, planned.day);
        let mut media = Vec::with_capacity(planned.media.len());
        for (index, item) in planned.media.iter().enumerate() {
            let meta = MediaMeta {
                guild_id: series.guild_id.clone(),
                series_id: series.id,
                day: planned.day,
                attachment_id: format!("70{:03}{:06}{index:02}", series.id, planned.day),
                content_type: match item {
                    Media::Stored(fixture) => fixture.content_type.to_owned(),
                    Media::Missing => "image/png".to_owned(),
                },
            };
            let keys = match item {
                Media::Stored(fixture) => Some(put_fixture(store, &meta, fixture).await?),
                Media::Missing => None,
            };
            media.push(NewMediaAttachment {
                attachment_id: meta.attachment_id,
                channel_id: channel_id.clone(),
                message_id: message_id.clone(),
                content_type: meta.content_type,
                media_missing: keys.is_none(),
                original_key: keys.as_ref().map(|(original, _)| original.clone()),
                thumb_key: keys.map(|(_, thumb)| thumb),
            });
        }
        rows.push((
            Post {
                series_id: series.id,
                day: planned.day,
                message_id,
                channel_id: channel_id.clone(),
                caption: planned.caption.clone(),
                posted_at: planned.posted_at,
                // Archived a minute after it was posted.
                archived_at: planned.posted_at + 60,
            },
            media,
        ));
    }
    let (_, skipped) = posts.insert_many(&rows).await?;
    if skipped > 0 {
        bail!(
            "{skipped} day(s) of series {} were already archived",
            series.id
        );
    }
    Ok(())
}

/// Stores a fixture's original and thumbnail for `meta`, returning their
/// keys.
async fn put_fixture(
    store: &dyn ObjectStore,
    meta: &MediaMeta,
    fixture: &'static Fixture,
) -> anyhow::Result<(String, String)> {
    let original = original_key(meta);
    let thumb = thumb_key(meta);
    store
        .put(
            &ObjectPath::from(original.clone()),
            Bytes::from_static(fixture.original).into(),
        )
        .await?;
    store
        .put(
            &ObjectPath::from(thumb.clone()),
            Bytes::from_static(fixture.thumb).into(),
        )
        .await?;
    Ok((original, thumb))
}

// --- the manifest -----------------------------------------------------------

/// Everything a suite needs to know about the seed, as `GET /__e2e/state`
/// answers it.
pub fn manifest(origin: &str, seeded: &Seeded) -> Value {
    let tz = localtime::tz_or_utc(TIMEZONE);
    let guilds = GUILDS.iter().zip(&seeded.settings);
    json!({
        "origin": origin,
        "client_id": CLIENT_ID,
        "timezone": TIMEZONE,
        "suggested_now_unix": SUGGESTED_NOW,
        "guilds": keyed(guilds.map(|(g, settings)| (g.key, guild_entry(g, settings)))),
        "personas": keyed(PERSONAS.iter().map(|p| (p.key, persona_entry(p)))),
        "series": keyed(seeded.series.iter().map(|s| (s.key, series_entry(s)))),
        "long_series": {
            "gaps": LONG_GAPS,
            "empty_month": LONG_EMPTY_MONTH,
            "same_date_days": [LONG_LATE_DAY - 1, LONG_LATE_DAY],
            "same_date": localtime::local_date(long_posted_at(LONG_LATE_DAY), tz),
            "three_images_day": LONG_THREE_IMAGES_DAY,
            "mp4_day": LONG_MP4_DAY,
            "webm_day": LONG_WEBM_DAY,
            "missing_media_day": LONG_MISSING_MEDIA_DAY,
            "no_caption_day": LONG_NO_CAPTION_DAY,
            "wordy_day": LONG_WORDY_DAY,
            "last_posted_at": long_posted_at(LONG_LAST_DAY),
        },
        "newcomer_eligible_at": NEWCOMER_ELIGIBLE_AT,
    })
}

fn keyed(entries: impl Iterator<Item = (&'static str, Value)>) -> Value {
    Value::Object(
        entries
            .map(|(key, value)| (key.to_owned(), value))
            .collect::<Map<_, _>>(),
    )
}

/// A guild as Discord lists it (from [`GUILDS`]) and as leaf has it set up
/// (from its stored settings row).
fn guild_entry(guild: &Guild, settings: &GuildSettings) -> Value {
    json!({
        "id": guild.id,
        "name": guild.name,
        "channels": keyed(guild.channels.iter().map(|c| {
            (c.key, json!({ "id": c.id, "name": c.name, "kind": c.kind }))
        })),
        "roles": keyed(guild.roles.iter().map(|r| (r.key, json!({ "id": r.id, "name": r.name })))),
        "setup_complete": settings.setup_complete,
        "timezone": settings.timezone,
        "watched_channel_ids": settings.watched_channels,
        "log_channel_id": settings.log_channel_id,
        "creator_role_id": settings.creator_role_id,
        "max_series_per_user": settings.max_series_per_user,
        "min_account_age_days": settings.min_account_age_days,
        "min_membership_age_days": settings.min_membership_age_days,
        "sprout_enabled": settings.sprout_enabled,
        "sprout_threshold": settings.sprout_threshold,
    })
}

fn persona_entry(persona: &Persona) -> Value {
    json!({
        "id": persona.id,
        "username": persona.username,
        "display_name": persona.display_name,
        "code": format!("{}{}", crate::discord::CODE_PREFIX, persona.key),
        "access_token": format!("{}{}", crate::discord::ACCESS_TOKEN_PREFIX, persona.key),
        "guild_ids": persona.memberships.iter().map(|m| m.guild_id).collect::<Vec<_>>(),
        "role_ids": persona.membership(MAIN_GUILD_ID).map_or(&[][..], |m| m.roles),
        "manages": persona.manages,
    })
}

fn series_entry(seeded: &SeededSeries) -> Value {
    let series = &seeded.series;
    json!({
        "id": series.id,
        "name": series.name,
        "guild_id": series.guild_id,
        "creator_id": series.creator_id,
        "state": series.state.as_str(),
        "privacy": series.privacy.as_str(),
        "cadence": series.cadence.as_str(),
        "channel_id": series.channels.first(),
        "archived_days": seeded.archived_days,
        "last_day": seeded.last_day,
    })
}
