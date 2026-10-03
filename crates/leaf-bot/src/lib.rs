//! leaf Discord bot: the serenity/poise client, slash commands, context
//! menus, and gateway event handling. Only started in run mode — the boot
//! state machine in the `leaf` binary gates this on Tier-1 config existing.
//!
//! [`supervise`] is the entry point: it connects, and reconnects with a
//! growing wait when a connection attempt fails for a reason that may pass.
//! Where the connection stands is recorded in [`leaf_core::status`], which
//! the HTTP server shows on `GET /api/status`.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo, SqlitePool};
use leaf_core::media::MediaPipeline;
use leaf_core::status::{self, GatewayState};
use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

pub mod checks;
pub mod commands;
pub mod components;
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
    /// leaf's public address (Tier-1 config), for links to the admin panel.
    /// `None` when the binary did not pass one.
    pub public_url: Option<String>,
    /// Id of the registered `/setup` command, so messages can mention it as
    /// a tappable chip. Zero until registration has succeeded; a watch, so
    /// a message that can afford to may wait for it (see
    /// [`checks::setup_mention_once_registered`]).
    pub setup_command: tokio::sync::watch::Receiver<u64>,
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
    /// When set, commands register in this one guild instead of globally.
    /// A development convenience: a guild's command list can be told apart
    /// from the global one while testing.
    pub dev_guild: Option<u64>,
    /// leaf's public address, for the admin-panel link in `/setup`.
    pub public_url: Option<String>,
}

/// Wait before the first reconnect; doubles after each failed attempt.
const RETRY_MIN: Duration = Duration::from_secs(5);
/// Longest wait between reconnects.
const RETRY_MAX: Duration = Duration::from_mins(5);
/// A connection that lasted this long counts as having worked: the next
/// failure starts again from [`RETRY_MIN`].
const STABLE_AFTER: Duration = Duration::from_mins(10);
/// Wait before the first repeat of a failed command registration.
const REGISTER_RETRY_MIN: Duration = Duration::from_secs(30);
/// Longest wait between registration attempts.
const REGISTER_RETRY_MAX: Duration = Duration::from_mins(30);
/// Shown on `GET /api/status` while the bot is connected but its command
/// list has not been accepted. A plain sentence: it is displayed as it is.
const REGISTRATION_FAILED: &str = "Discord has not accepted leaf's command list, so commands \
     may be missing or out of date. leaf keeps trying.";
/// Guild command lists cleared at startup at most (see [`clear_guild_scope`]).
const GUILD_SCOPE_SWEEP_MAX: usize = 25;

/// Runs the bot until shutdown, reconnecting when a connection attempt
/// fails for a reason that may pass (Discord down, network gone).
///
/// A failure that repeating cannot fix (Discord rejects the token) ends it:
/// retrying would only hammer Discord's login. The state and its reason
/// stay on `GET /api/status` for the server owner, and the HTTP server
/// keeps running so `--reconfigure` remains reachable.
pub async fn supervise(
    cfg: BotConfig,
    pool: SqlitePool,
    media: MediaPipeline,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut wait = RETRY_MIN;
    loop {
        if *shutdown.borrow() {
            return;
        }
        status::set_gateway(GatewayState::Starting);
        // A notice is about one connection; this is a new one.
        status::set_notice(None);
        let began = Instant::now();
        let outcome = run(cfg.clone(), pool.clone(), media.clone(), shutdown.clone()).await;
        if *shutdown.borrow() {
            return;
        }
        let permanent = match outcome {
            Ok(()) => {
                // The shards stopped although nobody asked them to.
                warn!("gateway stopped unexpectedly; reconnecting");
                status::set_gateway(GatewayState::Error(
                    "The bot's connection to Discord stopped. leaf is reconnecting.".to_owned(),
                ));
                false
            }
            Err(e) => {
                error!(error = format!("{e:#}"), "gateway exited with error");
                is_permanent(&e)
            }
        };
        if permanent {
            error!("gateway will not be retried: fix the cause, then restart leaf");
            return;
        }
        if began.elapsed() >= STABLE_AFTER {
            wait = RETRY_MIN;
        }
        info!(
            seconds = wait.as_secs(),
            "reconnecting to the gateway after a wait"
        );
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = flag_raised(shutdown.clone()) => return,
        }
        wait = next_wait(wait, RETRY_MAX);
    }
}

/// The wait after `current`: doubled, up to `max`.
fn next_wait(current: Duration, max: Duration) -> Duration {
    current.saturating_mul(2).min(max)
}

/// The serenity error under a gateway failure, if there is one.
fn serenity_error(e: &anyhow::Error) -> Option<&serenity::Error> {
    e.chain().find_map(|cause| cause.downcast_ref())
}

/// Whether a failed connection attempt cannot succeed when repeated.
fn is_permanent(e: &anyhow::Error) -> bool {
    matches!(
        serenity_error(e),
        Some(serenity::Error::Gateway(
            serenity::GatewayError::InvalidAuthentication
                | serenity::GatewayError::InvalidGatewayIntents
                | serenity::GatewayError::DisallowedGatewayIntents
        ))
    )
}

/// What the server owner is told about a failed connection attempt: a full
/// sentence, with nothing from the error itself (it may quote a URL).
fn failure_text(e: &anyhow::Error) -> &'static str {
    match serenity_error(e) {
        Some(serenity::Error::Gateway(serenity::GatewayError::InvalidAuthentication)) => {
            "Discord rejected the bot token. Restart leaf with --reconfigure and enter the \
             current token from the Developer Portal."
        }
        Some(serenity::Error::Gateway(
            serenity::GatewayError::InvalidGatewayIntents
            | serenity::GatewayError::DisallowedGatewayIntents,
        )) => {
            "Discord refused the bot's connection settings. Update leaf to the latest version, \
             then restart it."
        }
        _ => "The bot couldn't reach Discord. leaf keeps trying.",
    }
}

/// Connects to the gateway and runs until shutdown or a failure.
///
/// `shutdown` flipping to `true` ends the session cleanly. Network blips
/// are handled by serenity's internal reconnect; this only returns on
/// failures serenity gives up on, or on shutdown. A failure is recorded in
/// [`leaf_core::status`] before it is returned.
pub async fn run(
    cfg: BotConfig,
    pool: SqlitePool,
    media: MediaPipeline,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    // Nothing leaf does needs message events or message content: an archive
    // starts from a command, which carries its message.
    let intents = serenity::GatewayIntents::GUILDS;

    let dev_guild = cfg.dev_guild.map(serenity::GuildId::new);

    // Clones for the reminder scheduler and the registration retry, taken
    // before `pool` and `shutdown` move into the setup closure and the
    // shard-shutdown task.
    let sched_series = SeriesRepo::new(pool.clone());
    let sched_shutdown = shutdown.clone();
    let register_shutdown = shutdown.clone();
    let public_url = cfg.public_url.clone();
    // The registration retry of this connection attempt, so it can be
    // stopped with it: the next attempt starts its own.
    let registering: Arc<Mutex<Option<tokio::task::AbortHandle>>> = Arc::default();
    let registering_slot = Arc::clone(&registering);

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            on_error: |e| Box::pin(error::on_error(e)),
            event_handler: |ctx, event, fw, data| Box::pin(events::handle(ctx, event, fw, data)),
            // Nothing pings unless the message that should asks for it.
            allowed_mentions: Some(serenity::CreateAllowedMentions::new()),
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            Box::pin(async move {
                let (setup_command_tx, setup_command) = tokio::sync::watch::channel(0);
                let builders =
                    poise::builtins::create_application_commands(&framework.options().commands);
                info!(user = %ready.user.name, "gateway connected");
                status::set_gateway(GatewayState::Online);
                // Registration never takes the gateway down: it is tried
                // again, and the commands Discord already lists stay.
                let task = tokio::spawn(keep_registering(
                    ctx.http.clone(),
                    builders,
                    dev_guild,
                    ready.guilds.iter().map(|g| g.id).collect(),
                    setup_command_tx,
                    register_shutdown,
                ));
                *registering_slot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner) = Some(task.abort_handle());

                Ok(Data {
                    guilds: GuildSettingsRepo::new(pool.clone()),
                    series: SeriesRepo::new(pool.clone()),
                    posts: PostRepo::new(pool.clone()),
                    media,
                    pool,
                    started: Instant::now(),
                    public_url,
                    setup_command,
                })
            })
        })
        .build();

    // Messages ping nobody by default; the few that should (a reminder in
    // a channel) say so themselves.
    let http = serenity::HttpBuilder::new(&cfg.token)
        .default_allowed_mentions(serenity::CreateAllowedMentions::new())
        .build();
    let built = serenity::ClientBuilder::new_with_http(http, intents)
        .framework(framework)
        .await
        .context("building gateway client");
    let mut client = match built {
        Ok(client) => client,
        Err(e) => {
            status::set_gateway(GatewayState::Error(failure_text(&e).to_owned()));
            return Err(e);
        }
    };

    // Reminder scheduler: shares the gateway's HTTP client, stops on the
    // same shutdown signal, and does not outlive this connection attempt.
    let reminders = tokio::spawn(reminders::run(
        client.http.clone(),
        sched_series,
        sched_shutdown,
    ));

    // The shutdown signal ends the session here rather than through
    // `client.start()` returning: serenity only makes it return when a shard
    // is connected, so a shutdown while Discord is unreachable would
    // otherwise wait forever.
    let shard_manager = client.shard_manager.clone();
    let ended = tokio::select! {
        ended = client.start() => ended.context("gateway connection failed"),
        () = flag_raised(shutdown) => {
            info!("shutting the gateway down");
            shard_manager.shutdown_all().await;
            Ok(())
        }
    };
    // Neither task outlives this connection attempt: the next one starts
    // its own.
    reminders.abort();
    let registration = registering
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
    if let Some(registration) = registration {
        registration.abort();
    }
    if let Err(e) = &ended {
        status::set_gateway(GatewayState::Error(failure_text(e).to_owned()));
    }
    ended
}

/// Registers the commands, repeating with a growing wait until it works or
/// leaf shuts down, then tidies the other command scope once.
async fn keep_registering(
    http: Arc<serenity::Http>,
    builders: Vec<serenity::CreateCommand>,
    dev_guild: Option<serenity::GuildId>,
    guilds: Vec<serenity::GuildId>,
    setup_command: tokio::sync::watch::Sender<u64>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut wait = REGISTER_RETRY_MIN;
    loop {
        match register(&http, builders.clone(), dev_guild).await {
            Ok(registered) => {
                if let Some(id) = setup_command_id(&registered) {
                    // Nobody listening only means the connection is gone.
                    let _unheard = setup_command.send(id.get());
                }
                status::set_notice(None);
                info!(
                    count = registered.len(),
                    scope = if dev_guild.is_some() {
                        "guild"
                    } else {
                        "global"
                    },
                    "commands registered"
                );
                break;
            }
            Err(e) => {
                error!(
                    error = %e,
                    retry_in_seconds = wait.as_secs(),
                    "registering commands failed; the server keeps the command list of an \
                     earlier run, so new or changed commands do not work yet"
                );
                // The connection is up, but the server owner would
                // otherwise read "online" with no commands to use.
                status::set_notice(Some(REGISTRATION_FAILED.to_owned()));
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = flag_raised(shutdown.clone()) => return,
                }
                wait = next_wait(wait, REGISTER_RETRY_MAX);
            }
        }
    }
    if dev_guild.is_none() {
        clear_guild_scope(&http, &guilds).await;
    }
}

/// Registers leaf's commands in the configured scope and returns what
/// Discord now lists there.
///
/// Discord adds an Entry Point command of its own to an application with
/// Activities enabled, and refuses a bulk overwrite of the global list that
/// leaves it out (error 50240). So the global list is always written with
/// that command carried through. In guild mode the global list is reduced
/// to it, so no command shows twice.
async fn register(
    http: &serenity::Http,
    builders: Vec<serenity::CreateCommand>,
    dev_guild: Option<serenity::GuildId>,
) -> Result<Vec<serenity::Command>, serenity::Error> {
    let existing = serenity::Command::get_global_commands(http).await?;
    let entry_points = entry_point_builders(&existing);
    let Some(guild) = dev_guild else {
        let mut all = builders;
        all.extend(entry_points);
        return serenity::Command::set_global_commands(http, all).await;
    };
    let registered = guild.set_commands(http, builders).await?;
    let leftovers = existing
        .iter()
        .any(|c| c.kind != serenity::CommandType::PrimaryEntryPoint);
    if leftovers {
        // Commands from a run without DEV_GUILD_ID. Not worth failing
        // over: they still work, they just show twice.
        if let Err(e) = serenity::Command::set_global_commands(http, entry_points).await {
            warn!(error = %e, "could not clear the global command list");
        }
    }
    Ok(registered)
}

/// Builders that re-submit the Entry Point commands already registered, as
/// they are.
fn entry_point_builders(existing: &[serenity::Command]) -> Vec<serenity::CreateCommand> {
    existing
        .iter()
        .filter(|c| c.kind == serenity::CommandType::PrimaryEntryPoint)
        .map(|c| {
            let mut builder = serenity::CreateCommand::new(&c.name)
                .kind(serenity::CommandType::PrimaryEntryPoint)
                .description(&c.description);
            if let Some(handler) = c.handler {
                builder = builder.handler(handler);
            }
            if !c.integration_types.is_empty() {
                builder = builder.integration_types(c.integration_types.clone());
            }
            if let Some(contexts) = &c.contexts {
                builder = builder.contexts(contexts.clone());
            }
            builder
        })
        .collect()
}

/// The id of the registered `/setup` slash command.
fn setup_command_id(registered: &[serenity::Command]) -> Option<serenity::CommandId> {
    registered
        .iter()
        .find(|c| c.kind == serenity::CommandType::ChatInput && c.name == "setup")
        .map(|c| c.id)
}

/// Empties the guild-scoped command lists left behind by a run with
/// `DEV_GUILD_ID`, which would show every command twice next to the global
/// ones. Best effort, and only for an install of ordinary size: one request
/// per guild.
async fn clear_guild_scope(http: &serenity::Http, guilds: &[serenity::GuildId]) {
    if guilds.len() > GUILD_SCOPE_SWEEP_MAX {
        return;
    }
    for guild in guilds {
        if let Err(e) = guild.set_commands(http, Vec::new()).await {
            warn!(%guild, error = %e, "could not clear guild-scoped commands");
        }
    }
}

/// Resolves when the watched flag becomes `true` — and **only** then.
/// A dropped sender means "this signal can no longer fire", so it pends
/// forever rather than resolving (a `watch::changed()` call alone returns
/// on sender drop, which would read a dropped sender as a shutdown).
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
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; JSON indexing is fine in assertions"
    )]

    use super::*;

    fn command(json: serenity::json::Value) -> serenity::Command {
        serenity::json::from_value(json).unwrap()
    }

    fn entry_point() -> serenity::Command {
        command(serenity::json::json!({
            "id": "11", "type": 4, "application_id": "1", "name": "launch",
            "description": "Launch leaf", "version": "1", "handler": 2,
            "integration_types": [0], "contexts": [0],
        }))
    }

    fn slash(id: &str, name: &str) -> serenity::Command {
        command(serenity::json::json!({
            "id": id, "type": 1, "application_id": "1", "name": name,
            "description": "x", "version": "1",
        }))
    }

    #[test]
    fn the_entry_point_command_is_carried_through_unchanged() {
        let existing = [slash("5", "setup"), entry_point()];
        let builders = entry_point_builders(&existing);
        assert_eq!(builders.len(), 1);
        let sent = serenity::json::to_value(&builders[0]).unwrap();
        assert_eq!(sent["name"], "launch");
        assert_eq!(sent["type"], 4);
        assert_eq!(sent["description"], "Launch leaf");
        assert_eq!(sent["handler"], 2);
        assert_eq!(sent["integration_types"], serenity::json::json!([0]));
        assert_eq!(sent["contexts"], serenity::json::json!([0]));

        // An application without Activities has none to carry.
        assert!(entry_point_builders(&[slash("5", "setup")]).is_empty());
    }

    #[test]
    fn setup_command_id_is_the_slash_command_s() {
        let registered = [slash("4", "ping"), entry_point(), slash("5", "setup")];
        assert_eq!(
            setup_command_id(&registered),
            Some(serenity::CommandId::new(5))
        );
        assert_eq!(setup_command_id(&[slash("4", "ping")]), None);
    }

    #[test]
    fn waits_double_up_to_the_cap() {
        assert_eq!(next_wait(RETRY_MIN, RETRY_MAX), RETRY_MIN * 2);
        assert_eq!(next_wait(RETRY_MAX, RETRY_MAX), RETRY_MAX);
        assert_eq!(next_wait(Duration::from_mins(4), RETRY_MAX), RETRY_MAX);
    }

    #[test]
    fn a_rejected_token_is_permanent_and_explained() {
        let rejected = anyhow::Error::new(serenity::Error::Gateway(
            serenity::GatewayError::InvalidAuthentication,
        ))
        .context("gateway connection failed");
        assert!(is_permanent(&rejected));
        assert!(failure_text(&rejected).contains("--reconfigure"));

        let passing = anyhow::anyhow!("connection reset").context("gateway connection failed");
        assert!(!is_permanent(&passing));
        // Shown verbatim on the setup page: a sentence, ending in a period.
        for text in [
            failure_text(&rejected),
            failure_text(&passing),
            REGISTRATION_FAILED,
        ] {
            assert!(text.ends_with('.'), "{text}");
        }
    }

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
        // A sender dropped without ever sending `true` must NOT resolve.
        let (tx, rx) = tokio::sync::watch::channel(false);
        drop(tx);
        let result = tokio::time::timeout(Duration::from_millis(100), flag_raised(rx)).await;
        assert!(result.is_err(), "flag_raised resolved on sender drop");
    }
}
