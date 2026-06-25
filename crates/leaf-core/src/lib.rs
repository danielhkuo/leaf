//! leaf domain logic: configuration, database schema and repositories,
//! series/post domain types, day parsing, streak math, and the media
//! pipeline. No Discord and no HTTP serving — those live in `leaf-bot`
//! and `leaf-server` respectively.

pub mod config;
pub mod db;
pub mod domain;
pub mod media;
pub mod milestone;
pub mod parser;
pub mod policy;
pub mod reminder;
pub mod series_ops;
pub mod stats;
pub mod transfer;
pub mod wrapped;

#[cfg(test)]
mod smoke {
    #[test]
    fn harness_works() {
        assert_eq!(2 + 2, 4);
    }
}
