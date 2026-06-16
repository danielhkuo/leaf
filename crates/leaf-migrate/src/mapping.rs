//! Pure translation from walpurgisbot-v2 records to leaf domain types.
//!
//! Everything here is deterministic and side-effect-free, so the mapping
//! rules can be table-tested without a database, Discord, or object storage.
//! The orchestration that calls these lives in [`crate::importer`].

use std::hash::{Hash as _, Hasher as _};

use leaf_core::domain::NewMediaAttachment;
use leaf_core::media::{MediaMeta, StoredMedia};

/// Content type recorded for media we keep as a missing placeholder but
/// cannot classify from a filename. The bytes are gone, so this is purely
/// informational (the schema requires the column to be non-null).
const UNKNOWN_CONTENT_TYPE: &str = "application/octet-stream";

/// Maps a v2 day number to its leaf day by applying the `--day-offset`.
/// Saturating so a pathological offset can never wrap into a negative day.
#[must_use]
pub const fn leaf_day(v2_day: i64, offset: i64) -> i64 {
    v2_day.saturating_add(offset)
}

/// The R2 storage coordinates for one attachment of an imported day.
#[must_use]
pub fn media_meta(
    guild_id: &str,
    series_id: i64,
    day: i64,
    attachment_id: &str,
    content_type: &str,
) -> MediaMeta {
    MediaMeta {
        guild_id: guild_id.to_owned(),
        series_id,
        day,
        attachment_id: attachment_id.to_owned(),
        content_type: content_type.to_owned(),
    }
}

/// Parses a Discord CDN attachment URL into `(attachment_id, filename)`.
///
/// CDN paths are `/attachments/<channel>/<attachment_id>/<filename>` on both
/// `cdn.discordapp.com` and `media.discordapp.net`. The id and filename live
/// in the path, so they survive even after the signed query string has long
/// expired — which is exactly the state v2's stored URLs are in. That lets us
/// recover a *stable* attachment id for a `media_missing` row whose bytes can
/// no longer be fetched.
#[must_use]
pub fn parse_cdn_attachment(url: &str) -> Option<(String, String)> {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let after = path.split("/attachments/").nth(1)?;
    let mut segments = after.split('/');
    let _channel = segments.next()?;
    let attachment_id = segments.next()?;
    let filename = segments.next()?;
    if attachment_id.is_empty() || filename.is_empty() {
        return None;
    }
    Some((attachment_id.to_owned(), filename.to_owned()))
}

/// Guesses a MIME type from a filename extension, restricted to the formats
/// leaf archives. Anything else (or no extension) is treated as unknown.
#[must_use]
pub fn guess_content_type(filename: &str) -> String {
    let ext = filename
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" | "qt" => "video/quicktime",
        _ => UNKNOWN_CONTENT_TYPE,
    }
    .to_owned()
}

/// Builds the attachment row for media whose bytes were successfully
/// re-fetched and stored in R2.
#[must_use]
pub fn stored_attachment(
    attachment_id: &str,
    channel_id: &str,
    message_id: &str,
    content_type: &str,
    stored: &StoredMedia,
) -> NewMediaAttachment {
    NewMediaAttachment {
        attachment_id: attachment_id.to_owned(),
        channel_id: channel_id.to_owned(),
        message_id: message_id.to_owned(),
        content_type: content_type.to_owned(),
        original_key: Some(stored.original_key.clone()),
        thumb_key: Some(stored.thumb_key.clone()),
        media_missing: false,
    }
}

/// Builds a `media_missing` attachment row from a v2 CDN URL whose bytes can
/// no longer be fetched. Recovers the real attachment id and a guessed
/// content type when the URL is a parseable CDN link; otherwise synthesizes a
/// deterministic id (stable across re-runs) so the proxy can still address
/// the row.
#[must_use]
pub fn missing_attachment_from_url(
    url: &str,
    channel_id: &str,
    message_id: &str,
) -> NewMediaAttachment {
    let (attachment_id, content_type) = parse_cdn_attachment(url).map_or_else(
        || {
            (
                synthetic_attachment_id(message_id, url),
                UNKNOWN_CONTENT_TYPE.to_owned(),
            )
        },
        |(id, filename)| (id, guess_content_type(&filename)),
    );
    NewMediaAttachment {
        attachment_id,
        channel_id: channel_id.to_owned(),
        message_id: message_id.to_owned(),
        content_type,
        original_key: None,
        thumb_key: None,
        media_missing: true,
    }
}

/// Builds a `media_missing` attachment row from live attachment metadata
/// (the message was fetched, but this attachment's bytes could not be stored).
#[must_use]
pub fn missing_attachment_live(
    attachment_id: &str,
    channel_id: &str,
    message_id: &str,
    content_type: &str,
) -> NewMediaAttachment {
    NewMediaAttachment {
        attachment_id: attachment_id.to_owned(),
        channel_id: channel_id.to_owned(),
        message_id: message_id.to_owned(),
        content_type: content_type.to_owned(),
        original_key: None,
        thumb_key: None,
        media_missing: true,
    }
}

/// A deterministic stand-in attachment id for a non-CDN URL: the message id
/// plus a stable hash of the URL. Stable across runs so re-imports address
/// the same row.
fn synthetic_attachment_id(message_id: &str, url: &str) -> String {
    // `DefaultHasher` is SipHash seeded with fixed keys, so its output is
    // stable across processes — exactly what idempotent keys need.
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{message_id}-{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    #[test]
    fn leaf_day_applies_offset_and_saturates() {
        assert_eq!(leaf_day(1, 0), 1);
        assert_eq!(leaf_day(100, 5), 105);
        assert_eq!(leaf_day(10, -3), 7);
        assert_eq!(leaf_day(1, i64::MIN), i64::MIN.saturating_add(1));
    }

    #[test]
    fn parses_real_cdn_urls() {
        let url = "https://cdn.discordapp.com/attachments/999/12345/photo.PNG?ex=a&is=b&hm=c";
        assert_eq!(
            parse_cdn_attachment(url),
            Some(("12345".to_owned(), "photo.PNG".to_owned()))
        );

        // media.discordapp.net is the same path shape.
        let url = "https://media.discordapp.net/attachments/1/2/clip.mp4";
        assert_eq!(
            parse_cdn_attachment(url),
            Some(("2".to_owned(), "clip.mp4".to_owned()))
        );
    }

    #[test]
    fn rejects_non_cdn_or_truncated_urls() {
        assert_eq!(parse_cdn_attachment("https://example.com/foo.png"), None);
        assert_eq!(
            parse_cdn_attachment("https://cdn.discordapp.com/attachments/999/12345"),
            None
        );
        assert_eq!(parse_cdn_attachment("not a url"), None);
    }

    #[test]
    fn guesses_content_types_from_extension() {
        let cases = [
            ("a.png", "image/png"),
            ("a.PNG", "image/png"),
            ("a.jpg", "image/jpeg"),
            ("a.jpeg", "image/jpeg"),
            ("a.gif", "image/gif"),
            ("a.webp", "image/webp"),
            ("a.mp4", "video/mp4"),
            ("a.webm", "video/webm"),
            ("a.mov", "video/quicktime"),
            ("a.bin", "application/octet-stream"),
            ("noext", "application/octet-stream"),
            ("", "application/octet-stream"),
        ];
        for (name, expected) in cases {
            assert_eq!(guess_content_type(name), expected, "for {name}");
        }
    }

    #[test]
    fn missing_from_cdn_url_recovers_id_and_type() {
        let m = missing_attachment_from_url(
            "https://cdn.discordapp.com/attachments/9/42/pic.jpg?ex=1",
            "chan",
            "msg",
        );
        assert_eq!(m.attachment_id, "42");
        assert_eq!(m.content_type, "image/jpeg");
        assert_eq!(m.channel_id, "chan");
        assert_eq!(m.message_id, "msg");
        assert!(m.media_missing);
        assert!(m.original_key.is_none());
        assert!(m.thumb_key.is_none());
    }

    #[test]
    fn missing_from_unparseable_url_is_deterministic() {
        let url = "https://example.com/weird";
        let a = missing_attachment_from_url(url, "c", "msg");
        let b = missing_attachment_from_url(url, "c", "msg");
        assert_eq!(a.attachment_id, b.attachment_id, "id must be stable");
        assert!(a.attachment_id.starts_with("msg-"));
        assert_eq!(a.content_type, "application/octet-stream");
        assert!(a.media_missing);
        // A different URL yields a different id.
        let c = missing_attachment_from_url("https://example.com/other", "c", "msg");
        assert_ne!(a.attachment_id, c.attachment_id);
    }

    #[test]
    fn stored_attachment_carries_keys_and_is_present() {
        let stored = StoredMedia {
            original_key: "orig".to_owned(),
            thumb_key: "thumb".to_owned(),
            size: 10,
        };
        let m = stored_attachment("att", "c", "m", "image/png", &stored);
        assert_eq!(m.original_key.as_deref(), Some("orig"));
        assert_eq!(m.thumb_key.as_deref(), Some("thumb"));
        assert!(!m.media_missing);
    }
}
