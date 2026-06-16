//! `leaf-migrate`: one-shot CLI that imports a walpurgisbot-v2 archive
//! (`SQLite` DB or JSON export) into a leaf database, re-fetching media from
//! Discord while the original messages still exist.
//!
//! It reuses leaf-core throughout — [`leaf_core::db::connect`] for the target
//! (so it writes the exact schema the running bot reads),
//! [`leaf_core::config::Tier1Config`] for R2 + bot credentials, and
//! [`leaf_core::media::MediaPipeline`] for storage — and is idempotent, so a
//! re-run resumes where the last one stopped. See docs/phases.md Phase 20.

mod discord;
mod importer;
mod mapping;
mod source;

use std::path::PathBuf;
use std::time::Duration;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use clap::Parser;
use leaf_core::config::{CONFIG_FILE_NAME, Tier1Config};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

/// Import a walpurgisbot-v2 archive into a leaf database.
#[derive(Debug, Parser)]
#[command(name = "leaf-migrate", version, about)]
struct Cli {
    /// Source archive: a walpurgisbot-v2 `SQLite` DB or its JSON export
    /// (auto-detected by content).
    #[arg(long)]
    from: PathBuf,
    /// Target leaf `SQLite` database (created and migrated if absent).
    #[arg(long)]
    to: PathBuf,
    /// Target guild snowflake.
    #[arg(long)]
    guild: String,
    /// Creator (owner) snowflake for the imported series.
    #[arg(long)]
    creator: String,
    /// Name of the series to create or reuse in leaf.
    #[arg(long)]
    series_name: String,
    /// Watched channel snowflake for the series. Defaults to the distinct
    /// channels seen in the source.
    #[arg(long)]
    channel: Option<String>,
    /// Offset added to every v2 day number.
    #[arg(long, default_value_t = 0)]
    day_offset: i64,
    /// Read the source and print the plan without writing anything.
    #[arg(long)]
    dry_run: bool,
    /// Write a Markdown gaps report to this path.
    #[arg(long)]
    gaps_report: Option<PathBuf>,
    /// Path to leaf's Tier-1 config (R2 + bot credentials). Defaults to
    /// `$DATA_DIR/leaf.conf` (or `./data/leaf.conf`).
    #[arg(long)]
    config: Option<PathBuf>,
    /// Milliseconds to wait between Discord message fetches (politeness).
    #[arg(long, default_value_t = 250)]
    fetch_delay_ms: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_env("LOG_LEVEL").unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    let source = source::load(&cli.from).await?;
    info!(count = source.len(), from = %cli.from.display(), "loaded source archive");

    let pool = leaf_core::db::connect(&cli.to)
        .await
        .with_context(|| format!("opening target database {}", cli.to.display()))?;
    let series = leaf_core::db::SeriesRepo::new(pool.clone());
    let posts = leaf_core::db::PostRepo::new(pool.clone());
    let guilds = leaf_core::db::GuildSettingsRepo::new(pool.clone());

    let cfg = importer::ImportConfig {
        guild_id: cli.guild.clone(),
        creator_id: cli.creator.clone(),
        series_name: cli.series_name.clone(),
        series_channels: cli.channel.clone().map(|c| vec![c]).unwrap_or_default(),
        day_offset: cli.day_offset,
    };

    let summary = if cli.dry_run {
        let summary = importer::plan(&source, &cfg, &series, &posts).await?;
        info!(
            total = summary.total_source,
            would_import = summary.imported,
            already_present = summary.skipped_existing,
            predicted_gaps = summary.gaps.len(),
            "dry run — no changes written"
        );
        summary
    } else {
        let config_path = cli.config.clone().unwrap_or_else(default_config_path);
        let tier1 = Tier1Config::load(&config_path)?.with_context(|| {
            format!(
                "no leaf config at {} — run leaf setup first, or pass --config",
                config_path.display()
            )
        })?;
        let store = leaf_core::media::r2_store(&tier1.r2).context("building R2 store")?;
        let media =
            leaf_core::media::MediaPipeline::new(store).context("building media pipeline")?;
        let messages = discord::LiveMessageSource::new(
            &tier1.discord_token,
            Duration::from_millis(cli.fetch_delay_ms),
        )
        .context("building Discord client")?;
        let target = importer::Target {
            series: &series,
            posts: &posts,
            guilds: &guilds,
            media: &media,
        };
        let summary = importer::run(&source, &cfg, &target, &messages, now_unix()).await?;
        info!(
            series_id = summary.series_id,
            imported = summary.imported,
            skipped = summary.skipped_existing,
            deferred = summary.deferred,
            media_stored = summary.media_stored,
            media_missing = summary.media_missing,
            gaps = summary.gaps.len(),
            "migration complete"
        );
        if summary.deferred > 0 {
            warn!(
                deferred = summary.deferred,
                "some days were deferred after fetch errors — re-run to retry them"
            );
        }
        summary
    };

    if let Some(path) = &cli.gaps_report {
        let report = importer::render_gaps_markdown(&cli.series_name, &summary.gaps);
        tokio::fs::write(path, report)
            .await
            .with_context(|| format!("writing gaps report {}", path.display()))?;
        info!(path = %path.display(), gaps = summary.gaps.len(), "wrote gaps report");
    }

    pool.close().await;
    Ok(())
}

/// Default Tier-1 config location, mirroring the `leaf` binary's `DATA_DIR`.
fn default_config_path() -> PathBuf {
    let dir = std::env::var("DATA_DIR").unwrap_or_else(|_| "./data".to_owned());
    PathBuf::from(dir).join(CONFIG_FILE_NAME)
}

/// Current unix time in seconds.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}
