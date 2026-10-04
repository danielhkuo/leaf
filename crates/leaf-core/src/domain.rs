//! Domain types shared across the bot, server, and migrator.
//!
//! Database rows deserialize into these via the repository layer; string
//! enum columns round-trip through [`std::str::FromStr`]/[`AsRef`] impls.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Error returned when a string column holds a value outside an enum's
/// domain (schema CHECKs make this unreachable short of DB corruption).
#[derive(Debug, thiserror::Error)]
#[error("invalid {kind} value: {value}")]
pub struct InvalidEnumValue {
    /// Which enum was being parsed.
    pub kind: &'static str,
    /// The offending value.
    pub value: String,
}

macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$vmeta:meta])* $variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name {
            $($(#[$vmeta])* $variant,)+
        }

        impl $name {
            /// The canonical database/text representation.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }
        }

        impl FromStr for $name {
            type Err = InvalidEnumValue;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($text => Ok(Self::$variant),)+
                    other => Err(InvalidEnumValue { kind: stringify!($name), value: other.to_owned() }),
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

string_enum! {
    /// How often a series expects posts; drives reminders and "missed day"
    /// stats. `Freeform` means no schedule (and therefore no reminders).
    Cadence {
        /// One post every calendar day.
        Daily => "daily",
        /// Monday through Friday.
        Weekdays => "weekdays",
        /// One post per calendar week.
        Weekly => "weekly",
        /// No schedule.
        Freeform => "freeform",
    }
}

string_enum! {
    /// How posts enter the archive for a series.
    DetectionMode {
        /// Only via the right-click context menu (default).
        ContextMenu => "context_menu",
        /// Context menu plus the passive watcher with creator confirmation.
        Passive => "passive",
    }
}

string_enum! {
    /// Who can see a series in the gallery and query commands.
    Privacy {
        /// Every guild member.
        Public => "public",
        /// Members holding `privacy_role_id`.
        RoleGated => "role_gated",
        /// Only the creator (and admins).
        CreatorOnly => "creator_only",
    }
}

string_enum! {
    /// Lifecycle state of a series.
    SeriesState {
        /// Probation: archiving works, gallery listing is hidden.
        Sprout => "sprout",
        /// Normal operation.
        Active => "active",
        /// Revoked by an admin; read-only, hidden.
        Revoked => "revoked",
    }
}

/// Per-guild configuration (Tier 2; one row per guild).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildSettings {
    /// Guild snowflake.
    pub guild_id: String,
    /// Whether `/setup` has been completed; gates all series features.
    pub setup_complete: bool,
    /// Channel for quiet one-line archive confirmations.
    pub log_channel_id: Option<String>,
    /// Channels series are allowed to watch.
    pub watched_channels: Vec<String>,
    /// Role required to create series, if any.
    pub creator_role_id: Option<String>,
    /// Default IANA timezone for the guild.
    pub timezone: String,
    /// Creation policy: maximum live series per creator.
    pub max_series_per_user: i64,
    /// Creation policy: minimum Discord account age in days.
    pub min_account_age_days: i64,
    /// Creation policy: minimum guild membership age in days.
    pub min_membership_age_days: i64,
    /// Whether new series start in sprout probation.
    pub sprout_enabled: bool,
    /// Archived-post count at which a sprout is promoted.
    pub sprout_threshold: i64,
    /// Active dialogue persona name.
    pub active_persona: String,
}

impl GuildSettings {
    /// Settings for a freshly joined, not-yet-set-up guild.
    #[must_use]
    pub fn defaults_for(guild_id: &str) -> Self {
        Self {
            guild_id: guild_id.to_owned(),
            setup_complete: false,
            log_channel_id: None,
            watched_channels: Vec::new(),
            creator_role_id: None,
            timezone: "UTC".to_owned(),
            max_series_per_user: 3,
            min_account_age_days: 0,
            min_membership_age_days: 0,
            sprout_enabled: false,
            sprout_threshold: 3,
            active_persona: "default".to_owned(),
        }
    }
}

/// A creator's ongoing archive within a guild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Series {
    /// Database id.
    pub id: i64,
    /// Guild snowflake.
    pub guild_id: String,
    /// Creator user snowflake.
    pub creator_id: String,
    /// Display name, unique per guild.
    pub name: String,
    /// Free-text description.
    pub description: String,
    /// Watched channels (subset of the guild's `watched_channels`).
    pub channels: Vec<String>,
    /// Posting cadence.
    pub cadence: Cadence,
    /// Capture mode.
    pub detection_mode: DetectionMode,
    /// Visibility.
    pub privacy: Privacy,
    /// Role for [`Privacy::RoleGated`].
    pub privacy_role_id: Option<String>,
    /// Day number the series starts at (usually 1).
    pub start_day: i64,
    /// Whether reminders are enabled.
    pub reminder_enabled: bool,
    /// Reminder time of day, `HH:MM`.
    pub reminder_time: Option<String>,
    /// IANA timezone override for reminders.
    pub reminder_timezone: Option<String>,
    /// Remind via DM (true) or channel ping (false).
    pub reminder_dm: bool,
    /// Template for milestone announcements.
    pub milestone_template: Option<String>,
    /// Reaction emoji applied to archived posts.
    pub emoji: String,
    /// Lifecycle state.
    pub state: SeriesState,
    /// Creation time, unix seconds.
    pub created_at: i64,
}

/// Fields required to create a series.
#[derive(Debug, Clone)]
pub struct NewSeries {
    /// Guild snowflake.
    pub guild_id: String,
    /// Creator user snowflake.
    pub creator_id: String,
    /// Display name, unique per guild.
    pub name: String,
    /// Free-text description.
    pub description: String,
    /// Watched channels.
    pub channels: Vec<String>,
    /// Posting cadence.
    pub cadence: Cadence,
    /// Capture mode.
    pub detection_mode: DetectionMode,
    /// Visibility.
    pub privacy: Privacy,
    /// Role for [`Privacy::RoleGated`].
    pub privacy_role_id: Option<String>,
    /// Day number the series starts at.
    pub start_day: i64,
    /// Initial lifecycle state ([`SeriesState::Sprout`] when probation is on).
    pub state: SeriesState,
}

/// One archived day within a series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Post {
    /// Owning series id.
    pub series_id: i64,
    /// Day number (unique within the series).
    pub day: i64,
    /// Source message snowflake.
    pub message_id: String,
    /// Source channel snowflake.
    pub channel_id: String,
    /// Caption (message content at archive time).
    pub caption: String,
    /// Original message timestamp, unix seconds.
    pub posted_at: i64,
    /// Archive timestamp, unix seconds.
    pub archived_at: i64,
}

/// One media file attached to an archived day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaAttachment {
    /// Database id.
    pub id: i64,
    /// Owning series id.
    pub series_id: i64,
    /// Owning day number.
    pub day: i64,
    /// Discord attachment snowflake.
    pub attachment_id: String,
    /// Source channel snowflake.
    pub channel_id: String,
    /// Source message snowflake.
    pub message_id: String,
    /// MIME type as reported by Discord.
    pub content_type: String,
    /// R2 object key of the original; `None` iff `media_missing`.
    pub original_key: Option<String>,
    /// R2 object key of the thumbnail.
    pub thumb_key: Option<String>,
    /// True when the source message was gone before media could be fetched.
    pub media_missing: bool,
}

/// Media fields recorded at archive time (ids are assigned by the DB).
#[derive(Debug, Clone)]
pub struct NewMediaAttachment {
    /// Discord attachment snowflake.
    pub attachment_id: String,
    /// Source channel snowflake.
    pub channel_id: String,
    /// Source message snowflake.
    pub message_id: String,
    /// MIME type.
    pub content_type: String,
    /// R2 object key of the original.
    pub original_key: Option<String>,
    /// R2 object key of the thumbnail.
    pub thumb_key: Option<String>,
    /// True when media could not be fetched.
    pub media_missing: bool,
}

/// One archived day's headline for the gallery index: enough to draw a
/// calendar tile without loading the day itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaySummary {
    /// Day number.
    pub day: i64,
    /// Original message timestamp, unix seconds.
    pub posted_at: i64,
    /// The attachment that represents the day on a tile: the first one with
    /// a stored thumbnail, else the first media row. `None` when the day
    /// has no media rows at all.
    pub first_attachment_id: Option<String>,
    /// Whether that attachment has a stored thumbnail to serve.
    pub has_thumb: bool,
    /// True when no media file is stored for the day: every media row is a
    /// `media_missing` placeholder, or there are none.
    pub missing: bool,
    /// Number of media rows (stored and placeholder).
    pub media_count: i64,
}

/// One archived day flattened for `/export`: the post plus the object keys
/// of its stored originals, in attachment order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportRow {
    /// Day number.
    pub day: i64,
    /// Source message snowflake.
    pub message_id: String,
    /// Source channel snowflake.
    pub channel_id: String,
    /// Caption (message content at archive time).
    pub caption: String,
    /// Original message timestamp, unix seconds.
    pub posted_at: i64,
    /// Object keys of the stored originals; placeholders contribute none.
    pub media_keys: Vec<String>,
}

string_enum! {
    /// Why a reminder could not be delivered.
    ///
    /// These texts are the whole vocabulary of `series.reminder_error`: the
    /// scheduler writes [`Self::as_str`], the API passes the text through
    /// unchanged, and the Activity picks its advice by it. Add a variant
    /// here before writing a new reason anywhere.
    ReminderFailureKind {
        /// Discord refused the DM: the creator does not accept DMs from
        /// this server, blocked the bot, or no longer shares a server with
        /// it (codes 50007, 50278).
        DmClosed => "dm_closed",
        /// The channel to ping is gone (code 10003), or the series has no
        /// usable channel to ping.
        ChannelMissing => "channel_missing",
        /// leaf cannot see or post in the channel (codes 50001, 50013).
        NoPermission => "no_permission",
    }
}

impl ReminderFailureKind {
    /// Classifies a Discord JSON error code from a failed reminder send
    /// (typed as serenity reports it). `None` means the failure is not one
    /// of the permanent kinds and is not recorded.
    #[must_use]
    pub const fn from_discord_code(code: isize) -> Option<Self> {
        match code {
            50_007 | 50_278 => Some(Self::DmClosed),
            10_003 => Some(Self::ChannelMissing),
            50_001 | 50_013 => Some(Self::NoPermission),
            _ => None,
        }
    }
}

/// Why a series' last reminder could not be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReminderFailure {
    /// The recorded reason: a [`ReminderFailureKind`] text.
    pub reason: String,
    /// When the failure was recorded, unix seconds.
    pub at: i64,
}

impl ReminderFailure {
    /// The recorded reason as a [`ReminderFailureKind`]; `None` when the
    /// column holds a text outside the vocabulary (treat it as a generic
    /// "could not deliver").
    #[must_use]
    pub fn kind(&self) -> Option<ReminderFailureKind> {
        self.reason.parse().ok()
    }
}

/// Where the gallery should open after a chat button launched it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchIntent {
    /// Series to open.
    pub series_id: i64,
    /// Day to open within the series; `None` opens the series itself.
    pub day: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_round_trip_through_strings() {
        for c in [
            Cadence::Daily,
            Cadence::Weekdays,
            Cadence::Weekly,
            Cadence::Freeform,
        ] {
            assert_eq!(c.as_str().parse::<Cadence>().ok(), Some(c));
        }
        for p in [Privacy::Public, Privacy::RoleGated, Privacy::CreatorOnly] {
            assert_eq!(p.as_str().parse::<Privacy>().ok(), Some(p));
        }
        for s in [
            SeriesState::Sprout,
            SeriesState::Active,
            SeriesState::Revoked,
        ] {
            assert_eq!(s.as_str().parse::<SeriesState>().ok(), Some(s));
        }
        for d in [DetectionMode::ContextMenu, DetectionMode::Passive] {
            assert_eq!(d.as_str().parse::<DetectionMode>().ok(), Some(d));
        }
    }

    #[test]
    fn reminder_failure_kinds_round_trip_and_classify() {
        for (kind, text) in [
            (ReminderFailureKind::DmClosed, "dm_closed"),
            (ReminderFailureKind::ChannelMissing, "channel_missing"),
            (ReminderFailureKind::NoPermission, "no_permission"),
        ] {
            assert_eq!(kind.as_str(), text);
            assert_eq!(text.parse::<ReminderFailureKind>().ok(), Some(kind));
            let failure = ReminderFailure {
                reason: text.to_owned(),
                at: 0,
            };
            assert_eq!(failure.kind(), Some(kind));
        }
        let unknown = ReminderFailure {
            reason: "something else".to_owned(),
            at: 0,
        };
        assert_eq!(unknown.kind(), None);

        for (code, kind) in [
            (50_007, Some(ReminderFailureKind::DmClosed)),
            (50_278, Some(ReminderFailureKind::DmClosed)),
            (10_003, Some(ReminderFailureKind::ChannelMissing)),
            (50_001, Some(ReminderFailureKind::NoPermission)),
            (50_013, Some(ReminderFailureKind::NoPermission)),
            // Unknown message: not a delivery-route problem.
            (10_008, None),
            (0, None),
        ] {
            assert_eq!(ReminderFailureKind::from_discord_code(code), kind);
        }
    }

    #[test]
    fn unknown_enum_value_is_an_error() {
        let err = "bogus".parse::<Cadence>().err();
        assert!(err.is_some());
    }
}
