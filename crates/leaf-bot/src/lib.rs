//! leaf Discord bot: the serenity/poise client, slash commands, context
//! menus, and gateway event handling. Only started in run mode — the boot
//! state machine in the `leaf` binary gates this on Tier-1 config existing.

use std::time::Instant;

use anyhow::Context as _;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo, SqlitePool};
use leaf_core::media::MediaPipeline;
use poise::serenity_prelude as serenity;
use tracing::info;

pub mod checks;
pub mod commands;
pub mod error;
pub mod events;
pub mod reminders;

/// Shared state available to every command and event handler.
pub struct Data {
    /// Database pool (shared with leaf-server).
    pub pool: SqlitePool,
    /// Process start, for `/ping` uptime.
    pub started: Instant,
    /// Guild settings repository.
    pub guilds: GuildSettingsRepo,
    /// Series repository.
    pub series: SeriesRepo,
    /// Posts + media repository.
    pub posts: PostRepo,
    /// R2 media pipeline.
    pub media: MediaPipeline,
}

/// Error type carried by all commands.
pub type Error = anyhow::Error;
/// Poise context alias used by every command.
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// Gateway configuration for run mode.
#[derive(Debug, Clone)]
pub struct BotConfig {
    /// Discord bot token (Tier-1 config).
    pub token: String,
    /// When set, commands register guild-scoped here (instant updates)
    /// instead of globally (up to an hour of propagation).
    pub dev_guild: Option<u64>,
}

/// Connects to the gateway and runs until shutdown or a fatal error.
///
/// `shutdown` flipping to `true` ends the session cleanly. Network blips
/// are handled by serenity's internal reconnect; this only returns on
/// auth-level failures or shutdown.
pub async fn run(
    cfg: BotConfig,
    pool: SqlitePool,
    media: MediaPipeline,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let intents = serenity::GatewayIntents::GUILDS
        | serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::MESSAGE_CONTENT;

    let dev_guild = cfg.dev_guild.map(serenity::GuildId::new);

    // Clones for the reminder scheduler, taken before `pool` and `shutdown`
    // are moved into the setup closure and the shard-shutdown task.
    let sched_series = SeriesRepo::new(pool.clone());
    let sched_shutdown = shutdown.clone();

    // A setup-hook failure (e.g. registering commands in a guild the bot
    // was not yet invited to) must not leave a zombie gateway: connected,
    // resuming, but with no command handler. `fatal` lets the hook demand
    // a shard shutdown, and `run` returns an error the caller logs.
    let (fatal_tx, mut fatal_rx) = tokio::sync::watch::channel(false);

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            on_error: |e| Box::pin(error::on_error(e)),
            event_handler: |ctx, event, fw, data| Box::pin(events::handle(ctx, event, fw, data)),
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                let registered = if let Some(guild) = dev_guild {
                    poise::builtins::register_in_guild(ctx, &framework.options().commands, guild)
                        .await
                        .inspect(|()| info!(%guild, "commands registered (guild-scoped, dev)"))
                        .map_err(|e| {
                            anyhow::anyhow!(e).context(format!(
                                "registering commands in DEV_GUILD_ID {guild} — \
                                 is the bot actually invited to that guild?"
                            ))
                        })
                } else {
                    poise::builtins::register_globally(ctx, &framework.options().commands)
                        .await
                        .inspect(|()| info!("commands registered (global)"))
                        .map_err(|e| anyhow::anyhow!(e).context("registering global commands"))
                };

                if let Err(e) = registered {
                    tracing::error!(error = format!("{e:#}"), "framework setup failed");
                    let _send = fatal_tx.send(true);
                    return Err(e);
                }

                info!(user = %ready.user.name, "gateway connected");
                Ok(Data {
                    guilds: GuildSettingsRepo::new(pool.clone()),
                    series: SeriesRepo::new(pool.clone()),
                    posts: PostRepo::new(pool.clone()),
                    media,
                    pool,
                    started: Instant::now(),
                })
            })
        })
        .build();

    let mut client = serenity::ClientBuilder::new(&cfg.token, intents)
        .framework(framework)
        .await
        .context("building gateway client")?;

    // Reminder scheduler: shares the gateway's HTTP client, stops on the
    // same shutdown signal.
    tokio::spawn(reminders::run(
        client.http.clone(),
        sched_series,
        sched_shutdown,
    ));

    // Shut the shards down on either the process-wide shutdown signal or
    // a fatal setup failure; client.start() then returns.
    let shard_manager = client.shard_manager.clone();
    let fatal_watch = fatal_rx.clone();
    tokio::spawn(async move {
        tokio::select! {
            () = flag_raised(shutdown) => {}
            () = flag_raised(fatal_watch) => {}
        }
        shard_manager.shutdown_all().await;
        info!("gateway shut down");
    });

    client.start().await.context("gateway connection failed")?;

    if *fatal_rx.borrow_and_update() {
        anyhow::bail!("gateway stopped: framework setup failed (see error above)");
    }
    Ok(())
}

/// Resolves when the watched flag becomes `true` — and **only** then.
/// A dropped sender means "this signal can no longer fire", so it pends
/// forever rather than resolving (a `watch::changed()` call alone returns
/// on sender drop, which once shut the gateway down the moment poise
/// dropped the setup closure holding the fatal sender).
async fn flag_raised(mut rx: tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn flag_raised_resolves_only_on_true() {
        // Already true → immediate.
        let (tx, rx) = tokio::sync::watch::channel(true);
        drop(tx);
        tokio::time::timeout(Duration::from_millis(50), flag_raised(rx))
            .await
            .unwrap();

        // Set true later → resolves.
        let (tx, rx) = tokio::sync::watch::channel(false);
        let wait = tokio::spawn(flag_raised(rx));
        tx.send(true).unwrap();
        tokio::time::timeout(Duration::from_millis(50), wait)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn dropped_sender_is_not_a_signal() {
        // The regression: sender dropped without ever sending `true`
        // (poise dropping the setup closure) must NOT resolve.
        let (tx, rx) = tokio::sync::watch::channel(false);
        drop(tx);
        let result = tokio::time::timeout(Duration::from_millis(100), flag_raised(rx)).await;
        assert!(result.is_err(), "flag_raised resolved on sender drop");
    }
}
