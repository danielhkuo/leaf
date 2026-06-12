//! Import/export: the migration safety net and backup story.
//!
//! The wire format is walpurgisbot-v2's export shape — a JSON array of
//! posts — so a v2 export imports directly. On export, `media` carries
//! leaf's object keys (v2 carried CDN URLs, which expire and are useless
//! anyway); on import, media entries become `media_missing` placeholders —
//! actual bytes are re-fetched by `leaf-migrate`, which needs the original
//! messages, not stale URLs.

use serde::{Deserialize, Serialize};

/// One archived day in the transfer format (v2-compatible).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferPost {
    /// Day number.
    pub day: i64,
    /// Source message snowflake.
    pub message_id: String,
    /// Source channel snowflake.
    pub channel_id: String,
    /// Creator snowflake (v2: the tracked user).
    pub user_id: String,
    /// Original post time, unix seconds.
    pub timestamp: i64,
    /// v2: attachment URLs (expired); leaf: object keys. Informational on
    /// import either way.
    #[serde(default)]
    pub media: Vec<String>,
}

/// Parse failure with the JSON path that caused it.
#[derive(Debug, thiserror::Error)]
#[error("invalid import file at `{path}`: {message}")]
pub struct TransferParseError {
    /// JSON path of the offending value.
    pub path: String,
    /// What went wrong there.
    pub message: String,
}

/// Parses a transfer file, reporting the precise path on failure.
/// Unknown fields are tolerated (old v1 exports carry extras).
pub fn parse(raw: &[u8]) -> Result<Vec<TransferPost>, TransferParseError> {
    let de = &mut serde_json::Deserializer::from_slice(raw);
    serde_path_to_error::deserialize(de).map_err(|e| TransferParseError {
        path: e.path().to_string(),
        message: e.inner().to_string(),
    })
}

/// Serializes posts to the transfer format (pretty, stable order).
pub fn serialize(posts: &[TransferPost]) -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec_pretty(posts)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    fn sample() -> Vec<TransferPost> {
        vec![
            TransferPost {
                day: 1,
                message_id: "m1".into(),
                channel_id: "c1".into(),
                user_id: "u1".into(),
                timestamp: 1_700_000_000,
                media: vec!["g/1/s/1/d/1/a1".into()],
            },
            TransferPost {
                day: 2,
                message_id: "m2".into(),
                channel_id: "c1".into(),
                user_id: "u1".into(),
                timestamp: 1_700_086_400,
                media: vec![],
            },
        ]
    }

    #[test]
    fn round_trip_is_lossless() {
        let bytes = serialize(&sample()).unwrap();
        assert_eq!(parse(&bytes).unwrap(), sample());
    }

    #[test]
    fn real_v2_export_shape_parses() {
        // Verbatim shape from walpurgisbot-v2's findAllWithMedia export,
        // including a legacy extra field that must be tolerated.
        let raw = br#"[
          {
            "day": 150,
            "message_id": "1112223334445556667",
            "channel_id": "9998887776665554443",
            "user_id": "1231231231231231231",
            "timestamp": 1689000000,
            "user_mention": "<@123>",
            "media": ["https://cdn.discordapp.com/attachments/a/b/c.png"]
          }
        ]"#;
        let posts = parse(raw).unwrap();
        assert_eq!(posts.len(), 1);
        let first = posts.first().unwrap();
        assert_eq!(first.day, 150);
        assert_eq!(first.media.len(), 1);
    }

    #[test]
    fn missing_media_field_defaults_empty() {
        let raw = br#"[{"day":1,"message_id":"m","channel_id":"c","user_id":"u","timestamp":0}]"#;
        assert_eq!(parse(raw).unwrap().first().unwrap().media.len(), 0);
    }

    #[test]
    fn errors_carry_the_json_path() {
        let raw = br#"[{"day":"not-a-number","message_id":"m","channel_id":"c","user_id":"u","timestamp":0}]"#;
        let err = parse(raw).unwrap_err();
        assert!(err.path.starts_with("[0]"), "path was {}", err.path);
        let raw = b"not json at all";
        assert!(parse(raw).is_err());
    }
}
