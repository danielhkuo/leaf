//! leaf Discord bot: the serenity/poise client, slash commands, context
//! menus, and gateway event handling. Only started in run mode — the boot
//! state machine in the `leaf` binary gates this on Tier-1 config existing.
//!
//! [`supervise`] is the entry point: it connects, and reconnects with a
//! growing wait when a connection attempt fails for a reason that may pass.
//! Where the connection stands is recorded in [`leaf_core::status`], which
//! the HTTP server shows on `GET /api/status`.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo, SqlitePool};
use leaf_core::media::MediaPipeline;
use leaf_core::status::{self, GatewayState};
use poise::serenity_prelude as serenity;
use tracing::{error, info, warn};

mod channels;
pub mod checks;
pub mod commands;
pub mod components;
pub mod error;
pub mod events;
mod menus;
pub mod reminders;
#[cfg(test)]
mod stub;

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
    /// The bot's own name, read with [`Data::app_name`].
    app_name: AppName,
}

impl Data {
    /// The name Discord's menus list leaf's app under: the bot's own name
    /// as Discord shows it, which is "leaf" only when whoever hosts it called
    /// it that. Messages that walk someone through those menus name it (the
    /// `menus` module).
    ///
    /// It is what the gateway last said: at connect, and again when the bot
    /// is renamed (see [`events::handle`]).
    #[must_use]
    pub fn app_name(&self) -> String {
        self.app_name.get()
    }
}

/// The bot's own name, kept to what the gateway last said it is.
///
/// One can only be made from the bot user the gateway describes
/// ([`AppName::of`]), so there is no [`Data`] that was never named. It is
/// the writing end of a watch: the reminder scheduler, which starts before
/// the gateway has connected, holds the reading end and learns the name
/// with everyone else.
///
/// Discord's Android app was seen to list the app under this name with the
/// bot and its application named alike. Which of the two the list follows
/// when they are named differently has not been checked; if it is the
/// application, [`AppName::rename`] is the one place that chooses.
pub(crate) struct AppName(tokio::sync::watch::Sender<String>);

impl AppName {
    /// The name of `bot`, published on `names`.
    fn of(names: tokio::sync::watch::Sender<String>, bot: &serenity::CurrentUser) -> Self {
        let name = Self(names);
        name.rename(bot);
        name
    }

    /// Takes the name `bot` has now: its display name where Discord gives
    /// it one, else its username, which is how Discord shows a bot. Whoever
    /// waits on the name is woken only when it is a new one: the gateway
    /// says the same name again at every reconnect.
    pub(crate) fn rename(&self, bot: &serenity::CurrentUser) {
        let name = bot.display_name();
        self.0.send_if_modified(|current| {
            let renamed = current != name;
            if renamed {
                name.clone_into(current);
            }
            renamed
        });
    }

    /// The name as it stands.
    fn get(&self) -> String {
        self.0.borrow().clone()
    }
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
/// How long Discord has to answer, twice over: while the client is built
/// (a question or two over HTTP), and then for the gateway to say the bot is
/// connected. With no answer at all (a firewall holding the connection, a
/// dead route) leaf would wait for ever with its status on "starting" and
/// nothing beside it to say why.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// What the server owner is told when Discord gives no answer at all.
const NO_ANSWER: &str = "Discord didn't answer the bot. Check this machine's network, and any \
     firewall that filters outgoing connections. leaf keeps trying.";
/// What they are told when Discord did answer, with a rate limit longer
/// than [`CONNECT_TIMEOUT`]. Nothing on this machine is in the way.
const TOLD_TO_WAIT: &str =
    "Discord told the bot to wait before it connects (too many requests). leaf keeps trying.";
/// Shown beside "starting" when the gateway has not said the bot is
/// connected [`CONNECT_TIMEOUT`] after it was asked. It says no more than
/// leaf knows: serenity keeps trying on its own and does not say why.
const NOT_CONNECTED_YET: &str = "The bot hasn't connected to Discord yet. Check this machine's \
     network, and any firewall that filters outgoing connections. leaf keeps trying.";
/// What they are told when a connection that was up ends by itself.
const STOPPED: &str = "The bot's connection to Discord stopped. leaf is reconnecting.";
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
    let connect = || run(cfg.clone(), pool.clone(), media.clone(), shutdown.clone());
    keep_connected(connect, shutdown.clone(), RETRY_MIN).await;
}

/// [`supervise`], given what one connection attempt is (`connect`, which
/// records its own failure in [`leaf_core::status`], as [`run`] does) and
/// the wait before the first reconnect.
async fn keep_connected<C, A>(
    mut connect: C,
    shutdown: tokio::sync::watch::Receiver<bool>,
    first_wait: Duration,
) where
    C: FnMut() -> A + Send,
    A: Future<Output = anyhow::Result<()>> + Send,
{
    let mut wait = first_wait;
    // Why the last attempt ended, kept beside "starting" while leaf tries
    // again: an attempt can take minutes, and "starting" alone says nothing.
    let mut last_failure: Option<&'static str> = None;
    loop {
        if *shutdown.borrow() {
            return;
        }
        status::set_gateway(GatewayState::Starting);
        // A notice is about one connection; this is a new one.
        status::set_notice(last_failure.map(str::to_owned));
        let began = Instant::now();
        let outcome = connect().await;
        if *shutdown.borrow() {
            return;
        }
        let permanent = match outcome {
            Ok(()) => {
                // The shards stopped although nobody asked them to.
                warn!("gateway stopped unexpectedly; reconnecting");
                status::set_gateway(GatewayState::Error(STOPPED.to_owned()));
                last_failure = Some(STOPPED);
                false
            }
            Err(e) => {
                error!(error = format!("{e:#}"), "gateway exited with error");
                last_failure = Some(failure_text(&e));
                is_permanent(&e)
            }
        };
        if permanent {
            error!("gateway will not be retried: fix the cause, then restart leaf");
            return;
        }
        if began.elapsed() >= STABLE_AFTER {
            wait = first_wait;
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

/// Discord gave no answer at all within [`CONNECT_TIMEOUT`].
#[derive(Debug)]
struct NoAnswer;

impl std::fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Discord did not answer within {} seconds",
            CONNECT_TIMEOUT.as_secs()
        )
    }
}

impl std::error::Error for NoAnswer {}

/// Discord answered within [`CONNECT_TIMEOUT`], and what it said was to
/// wait longer than that (a rate limit).
#[derive(Debug)]
struct ToldToWait;

impl std::fmt::Display for ToldToWait {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Discord rate-limited the bot for more than {} seconds",
            CONNECT_TIMEOUT.as_secs()
        )
    }
}

impl std::error::Error for ToldToWait {}

/// Whether Discord has told the bot to wait before it asks again (a rate
/// limit), on one connection attempt.
///
/// serenity waits a rate limit out by itself, for as long as Discord says,
/// and tells nobody but its event handlers. From outside, that wait looks
/// like a connection nothing answers; this is how leaf tells them apart.
#[derive(Debug, Default)]
struct RateLimits(AtomicBool);

impl RateLimits {
    /// Discord said to wait.
    fn note(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Why a wait for Discord ran out: it said to wait longer, or it said
    /// nothing.
    fn silence(&self) -> anyhow::Error {
        if self.0.load(Ordering::Relaxed) {
            anyhow::Error::new(ToldToWait)
        } else {
            anyhow::Error::new(NoAnswer)
        }
    }
}

#[poise::async_trait]
impl serenity::EventHandler for RateLimits {
    async fn ratelimit(&self, _limit: serenity::RatelimitInfo) {
        self.note();
    }
}

/// Whether the gateway has said the bot is connected, on one connection
/// attempt.
///
/// The gateway's word arrives on one task and the timer that waits for it
/// runs on another. The lock makes "is it connected?" and what
/// [`leaf_core::status`] is then told a single step, so a timer that runs
/// out as the gateway answers cannot write over "online".
#[derive(Debug, Default)]
struct Connected(Mutex<bool>);

impl Connected {
    /// The gateway said so: the bot is online, and whatever stood beside
    /// "starting" (the last attempt's failure, a slow connection) is over.
    fn now(&self) {
        // The lock is held until the status is written (see the type).
        let mut connected = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        status::set_gateway(GatewayState::Online);
        status::set_notice(None);
        *connected = true;
    }

    /// Waits `patience`, and if the gateway has still said nothing, says so
    /// beside "starting". Nothing is stopped: serenity keeps trying to
    /// connect on its own, and [`Connected::now`] clears this when it does.
    async fn or_say_so_after(&self, patience: Duration) {
        tokio::time::sleep(patience).await;
        let connected = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if !*connected {
            warn!(
                seconds = patience.as_secs(),
                "gateway not connected yet; still trying"
            );
            status::set_notice(Some(NOT_CONNECTED_YET.to_owned()));
        }
    }
}

/// Waits for the gateway client to be built (`building`), but for no longer
/// than `patience`, and not at all once `shutdown` is raised: `None` then,
/// since stopping must not wait for an answer that may never come.
///
/// Running out of patience is a failure of its own, named by what `limits`
/// saw: Discord said to wait ([`ToldToWait`]) or said nothing ([`NoAnswer`]).
async fn build_within<B, T>(
    building: B,
    patience: Duration,
    limits: &RateLimits,
    shutdown: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<Option<T>>
where
    B: Future<Output = Result<T, serenity::Error>> + Send,
{
    tokio::select! {
        built = tokio::time::timeout(patience, building) => match built {
            Ok(built) => built.map(Some).context("building gateway client"),
            Err(_elapsed) => Err(limits.silence()),
        },
        () = flag_raised(shutdown) => Ok(None),
    }
}

/// What the server owner is told about a failed connection attempt: a full
/// sentence, with nothing from the error itself (it may quote a URL).
fn failure_text(e: &anyhow::Error) -> &'static str {
    if e.chain().any(<dyn std::error::Error>::is::<NoAnswer>) {
        return NO_ANSWER;
    }
    if e.chain().any(<dyn std::error::Error>::is::<ToldToWait>) {
        return TOLD_TO_WAIT;
    }
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
    // Empty until the gateway says who the bot is. The scheduler gets the
    // reading end now: it starts before that.
    let (app_names, sched_app_name) = tokio::sync::watch::channel(String::new());
    // The registration retry of this connection attempt, so it can be
    // stopped with it: the next attempt starts its own.
    let registering: Arc<Mutex<Option<tokio::task::AbortHandle>>> = Arc::default();
    let registering_slot = Arc::clone(&registering);
    // Whether the gateway has said the bot is connected, for the timer
    // below that waits for it.
    let connected = Arc::new(Connected::default());
    let said_so = Arc::clone(&connected);

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
                let builders = command_builders(&framework.options().commands);
                info!(user = %ready.user.name, "gateway connected");
                said_so.now();
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
                    // Named as it is made, so no handler sees it unnamed.
                    app_name: AppName::of(app_names, &ready.user),
                })
            })
        })
        .build();

    // Messages ping nobody by default; the few that should (a reminder in
    // a channel) say so themselves.
    let http = serenity::HttpBuilder::new(&cfg.token)
        .default_allowed_mentions(serenity::CreateAllowedMentions::new())
        .build();
    // serenity tells a rate limit to its event handlers and to nobody else.
    let limits = Arc::new(RateLimits::default());
    let building = serenity::ClientBuilder::new_with_http(http, intents)
        .framework(framework)
        .event_handler_arc(Arc::clone(&limits))
        .into_future();
    let mut client = match build_within(building, CONNECT_TIMEOUT, &limits, shutdown.clone()).await
    {
        Ok(Some(client)) => client,
        // leaf is stopping.
        Ok(None) => return Ok(()),
        Err(e) => {
            status::set_gateway(GatewayState::Error(failure_text(&e).to_owned()));
            return Err(e);
        }
    };

    // Reminder scheduler: shares the gateway's HTTP client and its cache,
    // stops on the same shutdown signal, and does not outlive this
    // connection attempt.
    let reminders = tokio::spawn(reminders::run(
        client.http.clone(),
        client.cache.clone(),
        sched_series,
        sched_app_name,
        sched_shutdown,
    ));
    // `client.start()` below says nothing until it fails for good, and a
    // gateway nothing answers is retried inside serenity for ever. This
    // puts a reason beside "starting" when that goes on.
    let overdue = tokio::spawn(async move { connected.or_say_so_after(CONNECT_TIMEOUT).await });

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
    // None of these tasks outlives this connection attempt: the next one
    // starts its own.
    reminders.abort();
    overdue.abort();
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

/// The builders registration submits for `commands`: one per slash command
/// and one per context menu.
fn command_builders(commands: &[poise::Command<Data, Error>]) -> Vec<serenity::CreateCommand> {
    poise::builtins::create_application_commands(commands)
}

/// A command as Discord's command lists identify it: by type and name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandKey {
    /// Discord's command type: 1 slash command, 2 user menu, 3 message menu.
    pub kind: u8,
    /// The name it is registered under.
    pub name: String,
}

/// Discord's type for a command submitted without one: a slash command.
const SLASH_COMMAND_KIND: u8 = 1;

/// The commands leaf registers, read from the builders registration submits
/// (see [`register`]). `leaf doctor` compares Discord's lists against this
/// without connecting to the gateway.
pub fn registered_commands() -> anyhow::Result<Vec<CommandKey>> {
    command_builders(&commands::all())
        .iter()
        .map(command_key)
        .collect()
}

/// The type and name a builder is submitted with.
fn command_key(builder: &serenity::CreateCommand) -> anyhow::Result<CommandKey> {
    let sent = serenity::json::to_value(builder).context("reading a command as it is sent")?;
    let name = sent
        .get("name")
        .and_then(serenity::json::Value::as_str)
        .context("a command is submitted without a name")?
        .to_owned();
    let kind = match sent.get("type") {
        None => SLASH_COMMAND_KIND,
        Some(kind) => kind
            .as_u64()
            .and_then(|kind| u8::try_from(kind).ok())
            .with_context(|| format!("command `{name}` is submitted with an unreadable type"))?,
    };
    Ok(CommandKey { kind, name })
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
    fn registered_commands_are_what_registration_submits() {
        let keys = registered_commands().unwrap();
        // One key per builder, in the order they are submitted.
        let builders = command_builders(&commands::all());
        assert_eq!(keys.len(), builders.len());
        for (key, builder) in keys.iter().zip(&builders) {
            let sent = serenity::json::to_value(builder).unwrap();
            assert_eq!(sent["name"], key.name.as_str());
        }
        // A slash command carries no type (Discord's default, 1); a message
        // menu says 3.
        let has = |kind: u8, name: &str| keys.iter().any(|k| k.kind == kind && k.name == name);
        assert!(has(1, "gallery"), "{keys:?}");
        assert!(has(1, "setup"), "{keys:?}");
        assert!(has(3, "Archive to Series"), "{keys:?}");
        assert!(!has(1, "Archive to Series"), "{keys:?}");
        // Discord identifies a command by type and name: no two may collide.
        let mut unique = keys.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), keys.len(), "{keys:?}");
    }

    #[test]
    fn a_command_key_reads_the_type_the_builder_sends() {
        let slash = serenity::CreateCommand::new("ping").description("x");
        assert_eq!(
            command_key(&slash).unwrap(),
            CommandKey {
                kind: 1,
                name: "ping".to_owned()
            }
        );
        let menu = serenity::CreateCommand::new("Who is this").kind(serenity::CommandType::User);
        assert_eq!(
            command_key(&menu).unwrap(),
            CommandKey {
                kind: 2,
                name: "Who is this".to_owned()
            }
        );
    }

    /// The bot user as the gateway describes it. `shown` is the display
    /// name, where Discord gives the bot one.
    fn bot(username: &str, shown: Option<&str>) -> serenity::CurrentUser {
        serenity::json::from_value(serenity::json::json!({
            "id": "901", "username": username, "global_name": shown, "bot": true,
        }))
        .unwrap()
    }

    #[test]
    fn the_app_is_named_after_the_bot_from_the_moment_it_connects() {
        // The scheduler has held its end since before the gateway connected.
        let (names, mut scheduler) = tokio::sync::watch::channel(String::new());
        assert_eq!(*scheduler.borrow_and_update(), "");

        let name = AppName::of(names, &bot("leaf-dev", None));
        assert_eq!(name.get(), "leaf-dev");
        assert!(scheduler.has_changed().unwrap());
        assert_eq!(*scheduler.borrow_and_update(), "leaf-dev");

        // Said again at every reconnect: nothing to wake anyone for.
        name.rename(&bot("leaf-dev", None));
        assert!(!scheduler.has_changed().unwrap());

        // The bot was renamed in the Developer Portal.
        name.rename(&bot("Daily Art Bot", None));
        assert!(scheduler.has_changed().unwrap());
        assert_eq!(*scheduler.borrow_and_update(), "Daily Art Bot");
        assert_eq!(name.get(), "Daily Art Bot");
    }

    #[test]
    fn the_app_s_name_is_the_one_discord_shows_for_the_bot() {
        let (names, _scheduler) = tokio::sync::watch::channel(String::new());
        // A display name, where the bot has one, is what Discord shows; the
        // username under it is not.
        let name = AppName::of(names, &bot("daily_art_bot", Some("Daily Art")));
        assert_eq!(name.get(), "Daily Art");
        name.rename(&bot("daily_art_bot", None));
        assert_eq!(name.get(), "daily_art_bot");
        name.rename(&bot("daily_art_bot", Some("Daily Art Bot")));
        assert_eq!(name.get(), "Daily Art Bot");
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

        // No answer at all is its own case: it is retried, and the owner is
        // pointed at the network rather than at Discord.
        let silent = anyhow::Error::new(NoAnswer);
        assert!(!is_permanent(&silent));
        assert!(failure_text(&silent).contains("firewall"));
        assert_ne!(failure_text(&silent), failure_text(&passing));

        // A rate limit is an answer: nothing on this machine is in the way.
        let limited = anyhow::Error::new(ToldToWait).context("gateway connection failed");
        assert!(!is_permanent(&limited));
        assert_eq!(failure_text(&limited), TOLD_TO_WAIT);
        assert!(!TOLD_TO_WAIT.contains("firewall"));
        assert!(!TOLD_TO_WAIT.contains("network"));

        // Shown verbatim on the setup page: a sentence, ending in a period.
        for text in [
            failure_text(&rejected),
            failure_text(&passing),
            failure_text(&silent),
            failure_text(&limited),
            NOT_CONNECTED_YET,
            STOPPED,
            REGISTRATION_FAILED,
        ] {
            assert!(text.ends_with('.'), "{text}");
        }
    }

    /// A client that is never built: nothing answers.
    fn never_built() -> std::future::Pending<Result<u8, serenity::Error>> {
        std::future::pending()
    }

    /// Longer than any test waits. Whatever is given this much patience is
    /// expected to end for another reason.
    const FOR_EVER: Duration = Duration::from_hours(1);

    #[tokio::test]
    async fn building_the_client_is_given_up_on_when_nothing_answers() {
        let (_stop, shutdown) = tokio::sync::watch::channel(false);
        let patience = Duration::from_millis(20);

        let limits = RateLimits::default();
        let silent = build_within(never_built(), patience, &limits, shutdown.clone())
            .await
            .unwrap_err();
        assert!(silent.is::<NoAnswer>(), "{silent:#}");
        assert_eq!(failure_text(&silent), NO_ANSWER);
        // It may pass, so it is tried again.
        assert!(!is_permanent(&silent));

        // Discord did answer, and said to wait longer than leaf does: the
        // owner is not sent to look at a firewall.
        limits.note();
        let limited = build_within(never_built(), patience, &limits, shutdown.clone())
            .await
            .unwrap_err();
        assert!(limited.is::<ToldToWait>(), "{limited:#}");
        assert_eq!(failure_text(&limited), TOLD_TO_WAIT);
        assert!(!is_permanent(&limited));
    }

    #[tokio::test]
    async fn a_client_built_in_time_is_handed_back() {
        let (_stop, shutdown) = tokio::sync::watch::channel(false);
        let limits = RateLimits::default();

        let built = build_within(async { Ok(7_u8) }, FOR_EVER, &limits, shutdown.clone())
            .await
            .unwrap();
        assert_eq!(built, Some(7));

        // A build that fails is that failure, not a silence.
        let refusing = async { Err::<u8, _>(serenity::Error::Other("refused")) };
        let refused = build_within(refusing, FOR_EVER, &limits, shutdown)
            .await
            .unwrap_err();
        assert!(!refused.is::<NoAnswer>(), "{refused:#}");
        assert!(serenity_error(&refused).is_some(), "{refused:#}");
        assert_eq!(
            failure_text(&refused),
            "The bot couldn't reach Discord. leaf keeps trying."
        );
    }

    #[tokio::test]
    async fn stopping_does_not_wait_for_discord_to_answer() {
        let limit = Duration::from_secs(5);

        // Raised before the build began.
        let (_stop, shutdown) = tokio::sync::watch::channel(true);
        let limits = RateLimits::default();
        let stopped = tokio::time::timeout(
            limit,
            build_within(never_built(), FOR_EVER, &limits, shutdown),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(stopped, None);

        // Raised while it waits.
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let waiting = tokio::spawn(async move {
            build_within(never_built(), FOR_EVER, &RateLimits::default(), shutdown).await
        });
        stop.send(true).unwrap();
        let stopped = tokio::time::timeout(limit, waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(stopped, None);
    }

    /// Held by every test that writes [`leaf_core::status`]: it is one
    /// value for the whole process, and tests run side by side.
    static STATUS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    /// What `GET /api/status` would show now: the state and the notice.
    fn shown() -> (GatewayState, Option<String>) {
        (status::gateway(), status::notice())
    }

    #[tokio::test]
    async fn the_last_failure_stands_beside_starting_until_the_bot_is_online() {
        let _status = STATUS.lock().await;
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        // What was shown as each attempt began, and once the second was up.
        let began = Mutex::new(Vec::new());
        let online = Mutex::new(None);
        let (began_in, online_in, stop_in) = (&began, &online, &stop);
        let connect = || {
            let attempt = {
                let mut began = began_in.lock().unwrap();
                began.push(shown());
                began.len()
            };
            async move {
                match attempt {
                    // Nothing answers.
                    1 => Err(anyhow::Error::new(NoAnswer)),
                    // The gateway says the bot is connected, and later the
                    // connection ends by itself.
                    2 => {
                        Connected::default().now();
                        *online_in.lock().unwrap() = Some(shown());
                        Ok(())
                    }
                    // leaf is stopped during the third.
                    _ => {
                        stop_in.send(true).unwrap();
                        Ok(())
                    }
                }
            }
        };

        let supervising = keep_connected(connect, shutdown, Duration::from_millis(1));
        tokio::time::timeout(Duration::from_secs(5), supervising)
            .await
            .unwrap();

        assert_eq!(
            began.into_inner().unwrap(),
            [
                (GatewayState::Starting, None),
                (GatewayState::Starting, Some(NO_ANSWER.to_owned())),
                (GatewayState::Starting, Some(STOPPED.to_owned())),
            ]
        );
        assert_eq!(
            online.into_inner().unwrap(),
            Some((GatewayState::Online, None))
        );
    }

    #[tokio::test]
    async fn a_rejected_token_is_not_tried_again() {
        let _status = STATUS.lock().await;
        let (_stop, shutdown) = tokio::sync::watch::channel(false);
        let attempts = Mutex::new(0_u32);
        let attempts_in = &attempts;
        let connect = || {
            *attempts_in.lock().unwrap() += 1;
            async {
                Err(anyhow::Error::new(serenity::Error::Gateway(
                    serenity::GatewayError::InvalidAuthentication,
                )))
            }
        };

        // It returns by itself, with nobody stopping it.
        let supervising = keep_connected(connect, shutdown, Duration::from_millis(1));
        tokio::time::timeout(Duration::from_secs(5), supervising)
            .await
            .unwrap();

        assert_eq!(attempts.into_inner().unwrap(), 1);
    }

    #[tokio::test]
    async fn a_connection_that_is_slow_to_come_up_says_so_beside_starting() {
        let _status = STATUS.lock().await;
        status::set_gateway(GatewayState::Starting);
        status::set_notice(None);
        let patience = Duration::from_millis(5);

        // The wait runs out and the gateway has said nothing: the state is
        // still "starting", now with a reason.
        let connected = Connected::default();
        connected.or_say_so_after(patience).await;
        assert_eq!(
            shown(),
            (GatewayState::Starting, Some(NOT_CONNECTED_YET.to_owned()))
        );

        // Then it connects after all.
        connected.now();
        assert_eq!(shown(), (GatewayState::Online, None));

        // A connection that came up in time: the wait running out changes
        // nothing.
        let prompt = Connected::default();
        prompt.now();
        prompt.or_say_so_after(patience).await;
        assert_eq!(shown(), (GatewayState::Online, None));
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
