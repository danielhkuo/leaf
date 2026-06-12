//! leaf Discord bot: serenity/poise client, slash commands, context menus,
//! the passive watcher, and scheduled jobs. Gateway connection is gated on
//! Tier-1 configuration existing (see `leaf-core::config`).
//!
//! Skeleton crate — filled in from Phase 4 of `docs/phases.md`.

#[cfg(test)]
mod smoke {
    #[test]
    fn harness_works() {
        assert_eq!(2 + 2, 4);
    }
}
