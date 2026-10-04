//! The real leaf API with everything outside the process replaced, for the
//! browser suites: no Discord, no R2, no bot.
//!
//! ```text
//! cargo run -p leaf-server --example e2e_server
//! ```
//!
//! What runs is `leaf_server::run::router` over a real [`ApiState`]: the
//! production routes, the production policy and the production media proxy.
//! What is replaced is only what `ApiState` already lets a caller choose:
//!
//! - the database is a fresh `SQLite` file in a temp dir, opened with
//!   `leaf_core::db::connect` (so the real migrations run) and filled through
//!   the repositories (see [`seed`]);
//! - object storage is `object_store`'s `InMemory`, holding the small real
//!   files under `fixtures/` at the keys the media pipeline uses;
//! - Discord is [`discord::StubDiscord`];
//! - the signing key is random, made at start and kept until exit.
//!
//! # Environment
//!
//! - `E2E_PORT`: the port to listen on, on 127.0.0.1 only. Default 3799;
//!   `0` takes a free one (the ready line says which).
//! - `STATIC_DIR`: the built Activity (`npm run build` in `activity/`).
//!   Default `activity/dist`, read by the real router. Without a build the
//!   API still works and `/` is the placeholder page.
//! - `LOG_LEVEL`: a `tracing` filter, default `info`.
//!
//! The server logs `e2e server ready on http://127.0.0.1:<port>` once it
//! accepts connections, and stops on SIGINT or SIGTERM.
//!
//! # One server, one test at a time
//!
//! A process has one world and one stub Discord. A reset, a change of the
//! stub's behaviour or an added day is seen by every client, so tests that
//! share a server interfere unless they run one after another: in
//! Playwright, a project with `workers: 1` and `fullyParallel: false`, each
//! test starting with `POST /__e2e/reset`. To run in parallel, start one
//! server per worker with `E2E_PORT=0` and read its port from the ready line.
//!
//! Reset between tests, not in the middle of one. A request still on its way
//! through the old world can leave what Discord answered it in a cache the
//! reset has just emptied. (Calls the stub is holding cannot: a reset fails
//! them.)
//!
//! # Booting the Activity
//!
//! The Activity starts only inside something that speaks Discord's embedded
//! app protocol, so a suite frames it in a host page of its own. What that
//! takes:
//!
//! - The Activity built with `VITE_DISCORD_CLIENT_ID` set to
//!   [`seed::CLIENT_ID`]. Off `<id>.discordsays.com` that is where it learns
//!   its client id.
//! - A host page on the server's own origin (in Playwright, `page.route`
//!   fulfilling a path the server does not have): the SDK drops every
//!   message that does not come from its own origin or one of Discord's.
//! - The Activity framed at `/?frame_id=…&instance_id=…&platform=desktop`
//!   (or `mobile`) `&guild_id=<guild>&channel_id=<channel>`.
//! - The host answering the frame's messages. To `[0, …]` (the handshake):
//!   `[1, {cmd: "DISPATCH", evt: "READY", data: {v: 1, config: {…}}, nonce:
//!   null}]`. To every `[1, {cmd, nonce, args, evt}]`: `[1, {cmd, data, evt:
//!   null, nonce}]`, where `data` has to pass the SDK's schema for the
//!   command or the SDK logs a validation error to the console. `AUTHORIZE`:
//!   `{code: "code-<persona>"}`. `AUTHENTICATE`: `{access_token` (as sent)`,
//!   user: {id, username, discriminator, global_name, avatar, public_flags},
//!   scopes, expires, application: {id, name, description}}`. `SUBSCRIBE`:
//!   `{evt}`, the message's own. `CAPTURE_LOG`: `null`.
//!
//! Serving the stock build from `<id>.discordsays.com` mapped to 127.0.0.1
//! (`--host-resolver-rules`) is not a shortcut in Playwright's Chromium: a
//! page there is refused the loopback address
//! (`net::ERR_BLOCKED_BY_LOCAL_NETWORK_ACCESS_CHECKS`) unless the browser is
//! also started with `--disable-features=LocalNetworkAccessChecks` and
//! `--unsafely-treat-insecure-origin-as-secure=<origin>`.
//!
//! # Signing in
//!
//! `POST /api/token` with `{"code": "code-<persona>"}` signs in as that
//! persona; the access token it hands back is `access-<persona>`. Personas:
//! `creator`, `viewer`, `admin`, `outsider`, `newcomer`, `patron`.
//!
//! # Control routes
//!
//! These exist only here, never in leaf. Bodies and answers are JSON; a
//! refusal is `{"error": "<sentence>"}`. A persona is named by its key; a
//! series by its key (`long`, `sprout`, `role_gated`, `creator_only`,
//! `revoked`, `reminder`) or its numeric id.
//!
//! - `GET /__e2e/state`: the manifest of the seed. Origin, client id,
//!   timezone, a clock to fix the browser at, and every guild (with its
//!   channels, its roles and its settings as they were stored), persona and
//!   series with its ids.
//! - `POST /__e2e/reset`: a fresh database and store with the seed, the stub
//!   Discord answering normally with nothing recorded, and every cached
//!   Discord answer and collected launch intent forgotten. Calls the stub
//!   was holding fail. Series ids are the same after every reset. Sessions
//!   minted earlier stay valid (the key does not change). Answers the
//!   manifest.
//! - `POST /__e2e/caches/clear`: forgets every cached Discord answer
//!   (membership, channels, roles, guild names, creator names) and collected
//!   launch intents, without touching the data. Answers 204.
//! - `GET /__e2e/discord`: `{mode, delay_ms, only, calls, held}`. `calls`
//!   counts each kind of call made to the stub since the last reset; `held`
//!   is how many calls it is holding at this moment.
//! - `POST /__e2e/discord` `{"mode": "ok" | "down" | "slow", "delay_ms"?:
//!   5000, "only"?: [..]}`: how the stub answers from now on. `down` fails
//!   calls as an unreachable Discord does. `slow` holds a call until
//!   `delay_ms` have passed or this route is posted to again, whichever is
//!   first; the call is then answered as the behaviour at that moment says
//!   (`ok` answers it, `down` fails it, `slow` holds it again). So a state
//!   that shows while leaf waits on Discord needs no timing: go slow with a
//!   delay longer than the test, wait for `held` to rise, look, then post
//!   the mode to go on with. `only` limits the mode to some calls
//!   (`exchange_code`, `current_user`, `guild_member`, `guild_channels`,
//!   `guild_roles`, `guild_summary`, `managed_guilds`, `send_message`).
//!   Answers cached before the switch are still served; clear the caches to
//!   have the next request ask again. Answers what the `GET` does (its
//!   `held` can still count calls this post is letting go of).
//! - `GET /__e2e/discord/messages`: `[{channel_id, content}]`, every log line
//!   leaf posted since the last reset, oldest first.
//! - `POST /__e2e/launch-intent` `{"persona", "series", "day"?, "guild"?}`:
//!   records an "Open gallery" press, as the bot does before it launches the
//!   Activity. `guild` (an id) defaults to the series' own; under any other
//!   the press is not there to collect in the series' guild.
//! - `POST /__e2e/series/{series}/days` `{"day"?, "media"?, "caption"?,
//!   "posted_at"?}` (the body may be left out): archives one more day, as the
//!   bot would while a gallery is open. `day` defaults to the next number,
//!   `posted_at` to a day after the newest post, `media` to `image`; the
//!   others are `images` (three), `mp4`, `webm`, `missing` (a row with no
//!   file) and `none`. As after the bot's own archive, a sprout that now has
//!   the days its guild asks for is published. Answers `{series_id, day,
//!   posted_at, local_date, state, promoted}`, `state` being the series'
//!   afterwards. 409 if the day is archived already, or the series is
//!   revoked (the bot archives nothing into one).
//! - `DELETE /__e2e/series/{series}/days/{day}`: removes an archived day and
//!   its files, as the bot does when the post is deleted in chat. A gallery
//!   that loaded the series earlier still lists the day. Answers 204; 404 if
//!   the day is not archived.
//! - `POST /__e2e/session` `{"persona", "kind"?, "age_secs"?, "ttl_secs"?}`:
//!   mints a gallery session in the shape of `POST /api/token`'s answer
//!   (`{token, access_token, expires_in}`, plus `user_id`, `auth_at`, `exp`),
//!   so a suite can hand it to the Activity in place of the real exchange.
//!   `kind` is `fresh` (default), `expiring` (lapses in 5 s), `expired`
//!   (lapsed a minute ago) or `capped` (valid for 5 minutes, but past the
//!   7-day cap, so it cannot be renewed). `age_secs` (how long ago the
//!   sign-in was, never negative) and `ttl_secs` (how long from now the
//!   token stays valid) override what the kind sets.
//! - `POST /__e2e/admin-session` `{"persona", "ttl_secs"?}`: mints an admin
//!   panel token for the guilds the persona manages (`{token, user_id,
//!   guild_ids, expires_in}`). Open `/admin#token=<token>` with it.
//! - `GET /__e2e/admin/login?persona=<persona>`: stands in for Discord's
//!   consent screen. Redirects to the real `/admin/callback` with that
//!   persona's code and a `state` the server accepts, which ends on
//!   `/admin#token=…` or `/admin#error=…`. `?code=<code>` in place of the
//!   persona sends that code as it is (one the stub refuses, say); it may
//!   hold letters, digits, `-` and `_`.
//! - `POST /__e2e/setup` `{"discord"?: "ok" | "refuse_token", "r2"?: "ok" |
//!   "refuse_bucket"}` (the body may be left out): opens setup mode. From
//!   then until the next reset, `/setup` and everything under it are leaf's
//!   real setup page and submit route, saving to a data directory of their
//!   own in a temp dir; every other address stays the configured leaf.
//!   Posting again starts over with a new code and an empty directory.
//!   Nothing is asked of Discord or R2: `discord` scripts what Discord says
//!   about the credentials (`refuse_token` is one error on the bot token)
//!   and `r2` what an S3 endpoint's check says (`refuse_bucket` is one error
//!   on the bucket). A folder on this machine is not scripted: the real
//!   check runs against the disk. Answers `{code, data_dir}`: the setup code
//!   leaf would print to its log, and the data directory (the page suggests
//!   `<data_dir>/media` as the folder).
//! - `GET /__e2e/setup`: the data directory as it stands. `{saved, raw,
//!   entries}`: `saved` is `leaf.conf` as leaf loads it (`{client_id,
//!   public_url, r2: {endpoint, bucket, access_key_id, secret_access_key}}`)
//!   or `null` before a submit has saved one, `raw` the file's text (or
//!   `null`), `entries` the names of everything in the directory. 404 when
//!   no setup mode is open.
//! - `GET /__e2e/elsewhere`: a redirect to another machine, answered as
//!   leaf's own `/admin/login` answers with one to discord.com. It points at
//!   `http://leaf-e2e.invalid/landed`, a name that never resolves. A browser
//!   follows a redirect where no request interception sees it, so this is
//!   what a suite checks its browser against: the navigation has to fail
//!   before the name is even looked up.

mod control;
mod discord;
mod seed;
mod setup;
#[cfg(test)]
mod tests;

use std::future::IntoFuture as _;
use std::io::IsTerminal as _;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use anyhow::Context as _;
use axum::Router;
use axum::extract::{Request, State};
use axum::response::Response;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo};
use leaf_server::api::auth::SessionKey;
use leaf_server::api::state::{self, ApiState};
use object_store::ObjectStore;
use object_store::memory::InMemory;
use tower::ServiceExt as _;

use crate::discord::StubDiscord;
use crate::seed::{Seeded, SeededSeries};

/// The port used when `E2E_PORT` is not set.
const DEFAULT_PORT: u16 = 3799;

/// How long requests still open at shutdown get to finish. A call the stub
/// is stalling would otherwise hold the process up for its whole delay.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(2);

/// One seeded copy of everything a reset replaces: the database, the store,
/// the per-state caches, and the real router built over them.
pub struct World {
    /// What the real routes run on; the control routes reach the
    /// repositories and the store through it.
    pub state: ApiState<StubDiscord>,
    /// What the seed stored, before any test changed it.
    seeded: Seeded,
    /// `leaf_server::run::router` over `state`.
    router: Router,
    /// Holds the database file; removed with the world. Declared last so the
    /// pool in `state` is dropped first.
    _dir: tempfile::TempDir,
}

impl World {
    async fn build(
        key: &SessionKey,
        discord: &Arc<StubDiscord>,
        origin: &str,
    ) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("creating the temp dir for the database")?;
        let pool = leaf_core::db::connect(&dir.path().join("e2e.db")).await?;
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let seeded = seed::seed(&pool, store.as_ref()).await?;
        let state = ApiState {
            series: SeriesRepo::new(pool.clone()),
            posts: PostRepo::new(pool.clone()),
            guilds: GuildSettingsRepo::new(pool),
            store,
            key: key.clone(),
            discord: Arc::clone(discord),
            membership: state::membership_cache(),
            channels: state::channels_cache(),
            redirect_uri: origin.to_owned(),
            client_id: seed::CLIENT_ID.to_owned(),
        };
        Ok(Self {
            router: leaf_server::run::router(state.clone()),
            state,
            seeded,
            _dir: dir,
        })
    }

    /// The seeded series, in creation order.
    #[must_use]
    pub fn series(&self) -> &[SeededSeries] {
        &self.seeded.series
    }
}

/// What lasts for the whole process, and the current [`World`].
pub struct Harness {
    /// The server's own origin, `http://127.0.0.1:<port>`.
    pub origin: String,
    /// The signing key, made at start. A reset keeps it.
    pub key: SessionKey,
    /// The stand-in for Discord. A reset puts it back to answering.
    pub discord: Arc<StubDiscord>,
    world: RwLock<Arc<World>>,
    /// Setup mode, while a suite has one open (`POST /__e2e/setup`). Without
    /// one the server is a configured leaf, as it is after every reset.
    setup: RwLock<Option<Arc<setup::Stage>>>,
    /// Held for the length of a reset, so two never interleave.
    resetting: tokio::sync::Mutex<()>,
}

impl Harness {
    /// Seeds the first world for a server reachable at `origin`.
    pub async fn start(origin: &str) -> anyhow::Result<Arc<Self>> {
        // Any two values nobody can guess: the key only has to be unknown.
        let secret = format!(
            "{:032x}{:032x}",
            rand::random::<u128>(),
            rand::random::<u128>()
        );
        let key = SessionKey::derive(&secret);
        let discord = Arc::new(StubDiscord::new(origin));
        let world = World::build(&key, &discord, origin).await?;
        clear_process_caches();
        Ok(Arc::new(Self {
            origin: origin.to_owned(),
            key,
            discord,
            world: RwLock::new(Arc::new(world)),
            setup: RwLock::new(None),
            resetting: tokio::sync::Mutex::new(()),
        }))
    }

    /// The world requests currently run against.
    pub fn world(&self) -> Arc<World> {
        Arc::clone(&self.world.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Replaces the world with a newly seeded one and puts the stub and the
    /// caches back to their starting state. A request still running against
    /// the old world finishes against it.
    pub async fn reset(&self) -> anyhow::Result<()> {
        let _one_at_a_time = self.resetting.lock().await;
        let fresh = Arc::new(World::build(&self.key, &self.discord, &self.origin).await?);
        *self.world.write().unwrap_or_else(PoisonError::into_inner) = fresh;
        *self.setup.write().unwrap_or_else(PoisonError::into_inner) = None;
        self.discord.reset();
        clear_process_caches();
        Ok(())
    }

    /// Setup mode, when a suite has opened one since the last reset.
    pub fn setup(&self) -> Option<Arc<setup::Stage>> {
        self.setup
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Opens a new setup mode in place of any earlier one: a new code, an
    /// empty data directory, nothing submitted.
    pub fn open_setup(&self, script: setup::Script) -> anyhow::Result<Arc<setup::Stage>> {
        let stage = Arc::new(setup::Stage::start(script)?);
        *self.setup.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::clone(&stage));
        Ok(stage)
    }

    /// Forgets every cached Discord answer and collected launch intent.
    pub fn clear_caches(&self) {
        let world = self.world();
        world.state.membership.invalidate_all();
        world.state.channels.invalidate_all();
        clear_process_caches();
    }

    /// The manifest of the seed as it stands in the current world.
    pub fn manifest(&self) -> serde_json::Value {
        seed::manifest(&self.origin, &self.world().seeded)
    }
}

/// Empties the caches leaf-server keeps for the whole process. They are
/// keyed by guild and user ids, which a reset leaves the same, so what one
/// test cached would otherwise answer for the next.
fn clear_process_caches() {
    state::channel_names_cache().invalidate_all();
    state::roles_cache().invalidate_all();
    state::guild_summary_cache().invalidate_all();
    state::creator_name_cache().invalidate_all();
    state::launch_replay_cache().invalidate_all();
}

/// The whole server: the control routes, and everything else handed to the
/// current world's real router.
pub fn app(harness: Arc<Harness>) -> Router {
    Router::new()
        .nest("/__e2e", control::router())
        .fallback(real_routes)
        .with_state(harness)
}

/// Whether `path` is the setup page or something under it.
fn is_setup_path(path: &str) -> bool {
    path.strip_prefix("/setup")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// Runs a request through `leaf_server::run::router` over the current world,
/// or, for the setup page's own addresses while a setup mode is open,
/// through `leaf_server::setup`'s router.
async fn real_routes(State(harness): State<Arc<Harness>>, request: Request) -> Response {
    let stage = harness
        .setup()
        .filter(|_| is_setup_path(request.uri().path()));
    let router = stage.map_or_else(
        || harness.world().router.clone(),
        |stage| stage.router.clone(),
    );
    match router.oneshot(request).await {
        Ok(response) => response,
        Err(never) => match never {},
    }
}

/// `E2E_PORT`, or the default.
fn port() -> anyhow::Result<u16> {
    std::env::var("E2E_PORT").map_or(Ok(DEFAULT_PORT), |raw| {
        raw.trim()
            .parse()
            .with_context(|| format!("E2E_PORT must be a port number, not {raw:?}"))
    })
}

/// Resolves on SIGINT (Ctrl-C) or SIGTERM, as the real binary's does.
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %e, "ctrl-c handler failed");
        }
    };
    #[cfg(unix)]
    {
        let mut term =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(term) => term,
                Err(e) => {
                    tracing::error!(error = %e, "SIGTERM handler failed");
                    return ctrl_c.await;
                }
            };
        tokio::select! {
            () = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    ctrl_c.await;
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        // Plain text when a test runner is reading the output.
        .with_ansi(std::io::stdout().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port()?))
        .await
        .context("binding the e2e server's port")?;
    let origin = format!("http://{}", listener.local_addr()?);
    let harness = Harness::start(&origin).await?;

    // The same rule `leaf_server::run` applies; said here so a missing build
    // is not mistaken for a broken gallery.
    let static_dir = std::env::var_os("STATIC_DIR")
        .map_or_else(|| PathBuf::from("activity/dist"), PathBuf::from);
    if !static_dir.join("index.html").is_file() {
        tracing::warn!(
            static_dir = %static_dir.display(),
            "no built Activity here: the API works, but / is the placeholder page"
        );
    }

    let stopping = Arc::new(tokio::sync::Notify::new());
    let signalled = Arc::clone(&stopping);
    let serve = axum::serve(listener, app(harness))
        .with_graceful_shutdown(async move {
            shutdown_signal().await;
            signalled.notify_one();
        })
        .into_future();
    let grace_over = async {
        stopping.notified().await;
        tokio::time::sleep(SHUTDOWN_GRACE).await;
    };

    tracing::info!(static_dir = %static_dir.display(), "e2e server ready on {origin}");
    tokio::select! {
        served = serve => served.context("e2e server failed")?,
        () = grace_over => tracing::warn!("requests were still open after the grace period"),
    }
    tracing::info!("e2e server stopped");
    Ok(())
}
