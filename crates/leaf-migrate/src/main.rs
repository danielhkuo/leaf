//! `leaf-migrate`: one-shot CLI that migrates a walpurgisbot-v2 archive
//! (`SQLite` DB or JSON export) into a leaf series, re-fetching media from
//! Discord while the original messages still exist.
//!
//! Skeleton — implemented in Phase 20 of `docs/phases.md`.

fn main() {
    tracing_subscriber::fmt().init();
    tracing::error!("leaf-migrate is not implemented yet (Phase 20)");
    std::process::exit(1);
}
