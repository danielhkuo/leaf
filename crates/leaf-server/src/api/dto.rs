//! Response shapes the embedded app consumes.
//!
//! Plain serde structs built from `leaf-core` domain types; media is
//! referenced by relative API paths (never raw Discord/R2 URLs) so the
//! frontend stays inside Discord's CSP.

use leaf_core::domain::{DaySummary, MediaAttachment, Post, Series, SeriesState};
use leaf_core::localtime::{self, Tz};
use leaf_core::stats::SeriesStats;
use serde::Serialize;

use crate::api::auth::MediaSigner;

/// A sprout's progress towards showing in the gallery.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SproutDto {
    /// Days archived so far.
    pub archived: i64,
    /// Days needed before the series is published.
    pub threshold: i64,
}

/// Per-series numbers the list attaches, each one cheap query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SeriesFacts {
    /// Highest archived day, if any.
    pub max_day: Option<i64>,
    /// Number of archived days.
    pub total_days: i64,
    /// Newest source-message time, unix seconds, if any day is archived.
    pub last_posted_at: Option<i64>,
}

/// Guild-wide values every series entry repeats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuildView {
    /// The zone calendar dates are computed in (the guild's timezone).
    pub tz: Tz,
    /// Archived days at which a sprout is published.
    pub sprout_threshold: i64,
}

/// A series as the gallery lists it.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SeriesDto {
    /// Database id (used in subsequent paths).
    pub id: i64,
    /// Display name.
    pub name: String,
    /// Free-text description.
    pub description: String,
    /// Creator snowflake.
    pub creator_id: String,
    /// `daily` / `weekdays` / `weekly` / `freeform`.
    pub cadence: String,
    /// Reaction emoji.
    pub emoji: String,
    /// The series' configured first day number (for heatmap rendering).
    pub start_day: i64,
    /// Highest archived day, if any.
    pub max_day: Option<i64>,
    /// `sprout` / `active` / `revoked`. A revoked series is listed only for
    /// its creator, and its days and stats stay 404.
    pub state: String,
    /// `public` / `role_gated` / `creator_only`.
    pub privacy: String,
    /// Whether the caller created this series.
    pub is_owner: bool,
    /// Channels the series archives from.
    pub channel_ids: Vec<String>,
    /// IANA zone the days' `local_date` values are computed in.
    pub timezone: String,
    /// Newest source-message time, unix seconds; `null` for an empty series.
    pub last_posted_at: Option<i64>,
    /// Number of archived days.
    pub total_days: i64,
    /// Progress towards publication; `null` unless the series is a sprout.
    pub sprout: Option<SproutDto>,
}

impl SeriesDto {
    /// Builds a list entry for the viewer `viewer_id`.
    #[must_use]
    pub fn build(s: &Series, viewer_id: &str, facts: SeriesFacts, guild: &GuildView) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            description: s.description.clone(),
            creator_id: s.creator_id.clone(),
            cadence: s.cadence.as_str().to_owned(),
            emoji: s.emoji.clone(),
            start_day: s.start_day,
            max_day: facts.max_day,
            state: s.state.as_str().to_owned(),
            privacy: s.privacy.as_str().to_owned(),
            is_owner: s.creator_id == viewer_id,
            channel_ids: s.channels.clone(),
            timezone: guild.tz.name().to_owned(),
            last_posted_at: facts.last_posted_at,
            total_days: facts.total_days,
            sprout: (s.state == SeriesState::Sprout).then_some(SproutDto {
                archived: facts.total_days,
                threshold: guild.sprout_threshold,
            }),
        }
    }
}

/// One media file, referenced by proxied API paths only.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MediaDto {
    /// Full-size: `/api/media/{attachment_id}`.
    pub url: String,
    /// Thumbnail: `/api/media/{attachment_id}?thumb`.
    pub thumb_url: String,
    /// MIME type.
    pub content_type: String,
    /// True when the bytes were never captured (imported placeholder).
    pub missing: bool,
}

impl MediaDto {
    fn from_attachment(m: &MediaAttachment, signer: &MediaSigner<'_>) -> Self {
        let (url, thumb_url) = signer.urls(&m.attachment_id);
        Self {
            url,
            thumb_url,
            content_type: m.content_type.clone(),
            missing: m.media_missing,
        }
    }
}

/// One archived day with its media.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DayDto {
    /// Day number.
    pub day: i64,
    /// Caption (original message text).
    pub caption: String,
    /// Original post time, unix seconds.
    pub posted_at: i64,
    /// Jump link to the source message.
    pub jump_url: String,
    /// Attached media.
    pub media: Vec<MediaDto>,
}

impl DayDto {
    /// Builds a day view. `guild_id` forms the jump URL; `signer` produces
    /// the signed media URLs.
    #[must_use]
    pub fn build(
        guild_id: &str,
        post: &Post,
        media: &[MediaAttachment],
        signer: &MediaSigner<'_>,
    ) -> Self {
        Self {
            day: post.day,
            caption: post.caption.clone(),
            posted_at: post.posted_at,
            jump_url: format!(
                "https://discord.com/channels/{guild_id}/{}/{}",
                post.channel_id, post.message_id
            ),
            media: media
                .iter()
                .map(|m| MediaDto::from_attachment(m, signer))
                .collect(),
        }
    }
}

/// A day's headline in the series index: the grid tile, the day number, and
/// where the day sits on the calendar.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DaySummaryDto {
    /// Day number.
    pub day: i64,
    /// Original post time, unix seconds.
    pub posted_at: i64,
    /// Signed thumbnail URL of the day's tile; `null` when no thumbnail is
    /// stored (an imported placeholder, or a day without media).
    pub thumb_url: Option<String>,
    /// `YYYY-MM-DD` of `posted_at` in the guild's timezone: the calendar
    /// cell for this day, the same for every viewer.
    pub local_date: String,
    /// Number of media files on the day.
    pub count: i64,
    /// True when no file was captured for the day.
    pub missing: bool,
}

impl DaySummaryDto {
    /// Builds an index entry from the day's summary row.
    #[must_use]
    pub fn build(row: &DaySummary, tz: Tz, signer: &MediaSigner<'_>) -> Self {
        // A URL is only worth sending when the proxy has something to serve.
        let thumb_url = row
            .first_attachment_id
            .as_deref()
            .filter(|_| row.has_thumb)
            .map(|id| signer.urls(id).1);
        Self {
            day: row.day,
            posted_at: row.posted_at,
            thumb_url,
            local_date: localtime::local_date(row.posted_at, tz),
            count: row.media_count,
            missing: row.missing,
        }
    }
}

/// Aggregate stats for the stats panel.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct StatsDto {
    /// Total archived days.
    pub total: i64,
    /// Current consecutive-day streak.
    pub current_streak: i64,
    /// Longest consecutive-day streak.
    pub longest_streak: i64,
    /// Days missed within `[start_day, max_day]`.
    pub missed: i64,
    /// Highest archived day.
    pub max_day: Option<i64>,
}

impl From<SeriesStats> for StatsDto {
    fn from(s: SeriesStats) -> Self {
        Self {
            total: s.total,
            current_streak: s.current_streak,
            longest_streak: s.longest_streak,
            missed: s.missed,
            max_day: s.max_day,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; JSON indexing is fine in assertions"
    )]

    use leaf_core::domain::{Cadence, DetectionMode, Privacy};

    use super::*;
    use crate::api::auth::SessionKey;

    fn attachment(missing: bool) -> MediaAttachment {
        MediaAttachment {
            id: 1,
            series_id: 1,
            day: 5,
            attachment_id: "att42".to_owned(),
            channel_id: "c".to_owned(),
            message_id: "m".to_owned(),
            content_type: "image/png".to_owned(),
            original_key: (!missing).then(|| "orig".to_owned()),
            thumb_key: (!missing).then(|| "thumb".to_owned()),
            media_missing: missing,
        }
    }

    #[test]
    fn media_dto_uses_signed_proxied_paths_only() {
        let key = SessionKey::derive("k");
        let signer = MediaSigner::new(&key, 0);
        let m = MediaDto::from_attachment(&attachment(false), &signer);
        assert!(m.url.starts_with("/api/media/att42?exp="));
        assert!(m.thumb_url.contains("thumb=1"));
        assert!(!m.url.contains("discord") && !m.url.contains("r2"));
    }

    #[test]
    fn day_dto_builds_jump_url_and_marks_missing_media() {
        let post = Post {
            series_id: 1,
            day: 5,
            message_id: "msg".to_owned(),
            channel_id: "chan".to_owned(),
            caption: "Day 5".to_owned(),
            posted_at: 1700,
            archived_at: 1701,
        };
        let key = SessionKey::derive("k");
        let signer = MediaSigner::new(&key, 0);
        let dto = DayDto::build("guild9", &post, &[attachment(true)], &signer);
        assert_eq!(dto.jump_url, "https://discord.com/channels/guild9/chan/msg");
        assert!(dto.media.first().unwrap().missing);
    }

    fn series(state: SeriesState) -> Series {
        Series {
            id: 3,
            guild_id: "g".to_owned(),
            creator_id: "u".to_owned(),
            name: "art".to_owned(),
            description: String::new(),
            channels: vec!["c1".to_owned()],
            cadence: Cadence::Weekdays,
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
            state,
            created_at: 0,
        }
    }

    const GUILD: GuildView = GuildView {
        tz: chicago(),
        sprout_threshold: 5,
    };

    const fn chicago() -> Tz {
        Tz::America__Chicago
    }

    #[test]
    fn series_dto_carries_listing_fields() {
        let facts = SeriesFacts {
            max_day: Some(12),
            total_days: 9,
            last_posted_at: Some(1700),
        };
        let dto = SeriesDto::build(&series(SeriesState::Active), "u", facts, &GUILD);
        assert_eq!(dto.cadence, "weekdays");
        assert_eq!(dto.max_day, Some(12));
        assert_eq!(dto.state, "active");
        assert_eq!(dto.privacy, "public");
        assert!(dto.is_owner);
        assert_eq!(dto.channel_ids, ["c1"]);
        assert_eq!(dto.timezone, "America/Chicago");
        assert_eq!(dto.last_posted_at, Some(1700));
        assert_eq!(dto.total_days, 9);
        assert_eq!(dto.sprout, None);

        let other = SeriesDto::build(&series(SeriesState::Active), "someone", facts, &GUILD);
        assert!(!other.is_owner);
    }

    #[test]
    fn series_dto_reports_sprout_progress_only_for_sprouts() {
        let facts = SeriesFacts {
            max_day: Some(2),
            total_days: 2,
            last_posted_at: Some(1),
        };
        let dto = SeriesDto::build(&series(SeriesState::Sprout), "u", facts, &GUILD);
        assert_eq!(
            dto.sprout,
            Some(SproutDto {
                archived: 2,
                threshold: 5
            })
        );
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["sprout"]["archived"], 2);

        // An empty active series sends explicit nulls, not missing keys.
        let empty = SeriesDto::build(
            &series(SeriesState::Active),
            "u",
            SeriesFacts::default(),
            &GUILD,
        );
        let json = serde_json::to_value(&empty).unwrap();
        assert!(json["sprout"].is_null());
        assert!(json["last_posted_at"].is_null());
        assert!(json["max_day"].is_null());
    }

    fn summary(first: Option<&str>, has_thumb: bool, missing: bool, count: i64) -> DaySummary {
        DaySummary {
            day: 7,
            // 2024-01-01 03:30 UTC, which is still 31 Dec in Chicago.
            posted_at: 1_704_079_800,
            first_attachment_id: first.map(ToOwned::to_owned),
            has_thumb,
            missing,
            media_count: count,
        }
    }

    #[test]
    fn day_summary_dates_the_day_in_the_guild_timezone() {
        let key = SessionKey::derive("k");
        let signer = MediaSigner::new(&key, 0);
        let row = summary(Some("att1"), true, false, 2);
        let dto = DaySummaryDto::build(&row, chicago(), &signer);
        assert_eq!(dto.local_date, "2023-12-31");
        assert_eq!(dto.count, 2);
        assert!(!dto.missing);
        assert!(dto.thumb_url.unwrap().contains("/api/media/att1?thumb=1"));

        let utc = DaySummaryDto::build(&row, Tz::UTC, &signer);
        assert_eq!(utc.local_date, "2024-01-01");
    }

    #[test]
    fn day_summary_has_no_thumb_url_without_a_stored_thumbnail() {
        let key = SessionKey::derive("k");
        let signer = MediaSigner::new(&key, 0);
        // An imported placeholder: a media row, but nothing stored.
        let placeholder = summary(Some("import-m-0"), false, true, 1);
        let dto = DaySummaryDto::build(&placeholder, Tz::UTC, &signer);
        assert_eq!(dto.thumb_url, None);
        assert!(dto.missing);
        // A day with no media rows at all.
        let bare = DaySummaryDto::build(&summary(None, false, true, 0), Tz::UTC, &signer);
        assert_eq!(bare.thumb_url, None);
        assert_eq!(bare.count, 0);
    }
}
