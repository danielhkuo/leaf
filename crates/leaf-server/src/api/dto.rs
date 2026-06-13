//! Response shapes the embedded app consumes.
//!
//! Plain serde structs built from `leaf-core` domain types; media is
//! referenced by relative API paths (never raw Discord/R2 URLs) so the
//! frontend stays inside Discord's CSP.

use leaf_core::domain::{MediaAttachment, Post, Series};
use leaf_core::stats::SeriesStats;
use serde::Serialize;

use crate::api::auth::MediaSigner;

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
    /// Highest archived day, if any.
    pub max_day: Option<i64>,
}

impl SeriesDto {
    /// Builds a list entry, attaching the series' current max day.
    #[must_use]
    pub fn from_series(s: &Series, max_day: Option<i64>) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            description: s.description.clone(),
            creator_id: s.creator_id.clone(),
            cadence: s.cadence.as_str().to_owned(),
            emoji: s.emoji.clone(),
            max_day,
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

/// A day's headline within a paginated list: the grid tile (first
/// attachment's signed thumbnail) plus the day number.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DaySummaryDto {
    /// Day number.
    pub day: i64,
    /// First attachment's signed thumbnail URL, if any.
    pub thumb_url: Option<String>,
}

impl DaySummaryDto {
    /// Builds a grid entry from the day's first attachment (if any).
    #[must_use]
    pub fn build(day: i64, first: Option<&MediaAttachment>, signer: &MediaSigner<'_>) -> Self {
        Self {
            day,
            thumb_url: first.map(|m| signer.urls(&m.attachment_id).1),
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
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use leaf_core::domain::{Cadence, DetectionMode, Privacy, SeriesState};

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
        let signer = MediaSigner::new(&key, 0, 100);
        let m = MediaDto::from_attachment(&attachment(false), &signer);
        assert!(m.url.starts_with("/api/media/att42?exp=100&sig="));
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
        let signer = MediaSigner::new(&key, 0, 100);
        let dto = DayDto::build("guild9", &post, &[attachment(true)], &signer);
        assert_eq!(dto.jump_url, "https://discord.com/channels/guild9/chan/msg");
        assert!(dto.media.first().unwrap().missing);
    }

    #[test]
    fn series_dto_carries_cadence_string_and_max_day() {
        let s = Series {
            id: 3,
            guild_id: "g".to_owned(),
            creator_id: "u".to_owned(),
            name: "art".to_owned(),
            description: String::new(),
            channels: vec![],
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
            state: SeriesState::Active,
            created_at: 0,
        };
        let dto = SeriesDto::from_series(&s, Some(12));
        assert_eq!(dto.cadence, "weekdays");
        assert_eq!(dto.max_day, Some(12));
    }
}
