//! Import/export: a series' day index as a file. Not a backup: the format
//! carries no media bytes and no captions.
//!
//! The wire format is walpurgisbot-v2's export shape — a JSON array of
//! posts — so a v2 export imports directly. On export, `media` carries
//! leaf's object keys (v2 carried CDN URLs, which expire and are useless
//! anyway); on import, media entries become `media_missing` placeholders and
//! no bytes are brought in. Only `leaf-migrate` fetches files, from the
//! original messages rather than stale URLs: for days that are not archived
//! yet, and for days an import left without any stored file.

use std::fmt;

use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};
use serde::{Deserialize, Serialize};

/// Most entries [`parse_import`] keeps from one file.
///
/// A daily series reaches this after more than fifty years; the cap keeps a
/// crafted file from filling memory or holding the database's write lock
/// for minutes.
pub const MAX_IMPORT_ENTRIES: usize = 20_000;

/// Most `media` items [`parse_import`] keeps for one entry: what a Discord
/// message can carry. A `media` list longer than that is not a post's.
pub const MAX_MEDIA_PER_ENTRY: usize = 10;

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

/// An uploaded transfer file, read with bounds (see [`parse_import`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// The file's entries in order, at most [`MAX_IMPORT_ENTRIES`] of them,
    /// each with at most [`MAX_MEDIA_PER_ENTRY`] `media` items.
    pub posts: Vec<TransferPost>,
    /// How many entries the file holds, kept or not.
    pub entries: usize,
}

/// Parses a transfer file someone uploaded, with memory bounded whatever
/// the file holds.
///
/// Entries past [`MAX_IMPORT_ENTRIES`] are counted but not kept, and `media`
/// items past [`MAX_MEDIA_PER_ENTRY`] are skipped as they are read.
/// Otherwise as [`parse`], which is for a file the operator supplies and
/// keeps everything.
pub fn parse_import(raw: &[u8]) -> Result<Import, TransferParseError> {
    let de = &mut serde_json::Deserializer::from_slice(raw);
    serde_path_to_error::deserialize(de).map_err(|e| TransferParseError {
        path: e.path().to_string(),
        message: e.inner().to_string(),
    })
}

/// [`TransferPost`] as [`parse_import`] reads it: the same fields, with the
/// `media` list cut short while it is read.
#[derive(Deserialize)]
struct ImportPost {
    day: i64,
    message_id: String,
    channel_id: String,
    user_id: String,
    timestamp: i64,
    #[serde(default, deserialize_with = "first_media")]
    media: Vec<String>,
}

impl From<ImportPost> for TransferPost {
    fn from(p: ImportPost) -> Self {
        Self {
            day: p.day,
            message_id: p.message_id,
            channel_id: p.channel_id,
            user_id: p.user_id,
            timestamp: p.timestamp,
            media: p.media,
        }
    }
}

/// Reads a `media` array, keeping its first [`MAX_MEDIA_PER_ENTRY`] items.
/// The rest are parsed (the JSON must still be well formed) but never held.
fn first_media<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    struct FirstMedia;

    impl<'de> Visitor<'de> for FirstMedia {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a sequence")
        }

        fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
            let mut kept = Vec::new();
            while kept.len() < MAX_MEDIA_PER_ENTRY {
                match seq.next_element::<String>()? {
                    Some(item) => kept.push(item),
                    None => return Ok(kept),
                }
            }
            while seq.next_element::<IgnoredAny>()?.is_some() {}
            Ok(kept)
        }
    }

    deserializer.deserialize_seq(FirstMedia)
}

impl<'de> Deserialize<'de> for Import {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;

        impl<'de> Visitor<'de> for Entries {
            type Value = Import;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a sequence")
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut posts = Vec::new();
                while posts.len() < MAX_IMPORT_ENTRIES {
                    let Some(post) = seq.next_element::<ImportPost>()? else {
                        let entries = posts.len();
                        return Ok(Import { posts, entries });
                    };
                    posts.push(post.into());
                }
                let mut entries = posts.len();
                while seq.next_element::<IgnoredAny>()?.is_some() {
                    entries = entries.saturating_add(1);
                }
                Ok(Import { posts, entries })
            }
        }

        deserializer.deserialize_seq(Entries)
    }
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
    fn an_import_reads_the_same_as_a_plain_parse() {
        let bytes = serialize(&sample()).unwrap();
        assert_eq!(
            parse_import(&bytes).unwrap(),
            Import {
                posts: sample(),
                entries: 2
            }
        );
        // Extra fields and a missing `media` are tolerated the same way.
        let raw = br#"[{"day":1,"message_id":"m","channel_id":"c","user_id":"u","timestamp":0,
                        "user_mention":"<@1>"}]"#;
        assert_eq!(parse_import(raw).unwrap().posts, parse(raw).unwrap());
        assert_eq!(
            parse_import(b"[]").unwrap(),
            Import {
                posts: vec![],
                entries: 0
            }
        );
    }

    #[test]
    fn an_import_keeps_only_the_first_media_items_of_an_entry() {
        let media: Vec<String> = (0..5_000).map(|n| format!("k{n}")).collect();
        let raw = format!(
            r#"[{{"day":1,"message_id":"m","channel_id":"c","user_id":"u","timestamp":0,
                 "media":{}}},
                {{"day":2,"message_id":"m2","channel_id":"c","user_id":"u","timestamp":0,
                 "media":["only"]}}]"#,
            serde_json::to_string(&media).unwrap()
        );
        let import = parse_import(raw.as_bytes()).unwrap();
        assert_eq!(import.entries, 2);
        let kept: Vec<usize> = import.posts.iter().map(|p| p.media.len()).collect();
        assert_eq!(kept, [MAX_MEDIA_PER_ENTRY, 1]);
        assert_eq!(
            import.posts.first().unwrap().media.last().unwrap(),
            &format!("k{}", MAX_MEDIA_PER_ENTRY - 1)
        );
    }

    #[test]
    fn an_import_counts_entries_past_the_cap_without_keeping_them() {
        let one = r#"{"day":1,"message_id":"m","channel_id":"c","user_id":"u","timestamp":0}"#;
        let raw = format!("[{}]", vec![one; MAX_IMPORT_ENTRIES + 3].join(","));
        let import = parse_import(raw.as_bytes()).unwrap();
        assert_eq!(import.entries, MAX_IMPORT_ENTRIES + 3);
        assert_eq!(import.posts.len(), MAX_IMPORT_ENTRIES);
    }

    #[test]
    fn import_errors_carry_the_json_path() {
        let raw = br#"[{"day":"x","message_id":"m","channel_id":"c","user_id":"u","timestamp":0}]"#;
        let err = parse_import(raw).unwrap_err();
        assert_eq!(err.path, parse(raw).unwrap_err().path);
        assert_eq!(err.message, parse(raw).unwrap_err().message);

        let raw = br#"[{"day":1,"message_id":"m","channel_id":"c","user_id":"u","timestamp":0,
                        "media":[1]}]"#;
        let err = parse_import(raw).unwrap_err();
        assert_eq!(err.path, parse(raw).unwrap_err().path);

        for raw in [&b"not json at all"[..], b"{}", b"[1", b"\"abc\""] {
            let (bounded, plain) = (parse_import(raw).unwrap_err(), parse(raw).unwrap_err());
            assert_eq!(bounded.path, plain.path, "{raw:?}");
        }
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
