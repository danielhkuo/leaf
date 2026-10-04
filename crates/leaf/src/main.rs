//! leaf composition root: one process, two-state boot.
//!
//! The HTTP server always starts first. With no Tier-1 config present (or
//! `--reconfigure`), it serves the setup flow and only transitions to run
//! mode — gateway and all — once credentials are validated and written.
//! See PLAN.md § First-run setup and docs/phases.md Phase 3.
//!
//! `leaf doctor` is the one other thing the binary does: it checks a
//! configured install and exits, starting neither (see [`doctor`]).

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context as _;
use leaf_core::config::{CONFIG_FILE_NAME, Tier1Config};
use tracing::info;

mod doctor;

/// The first argument that turns the run into `leaf doctor`.
const DOCTOR_COMMAND: &str = "doctor";

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[tokio::main]
async fn main() -> anyhow::Result<ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data_dir = PathBuf::from(env_or("DATA_DIR", "./data"));

    if let Some((command, doctor_args)) = args.split_first()
        && command == DOCTOR_COMMAND
    {
        // The findings own stdout (`--json` is read by programs), so the
        // log goes to stderr, and by default only says what went wrong.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(
                tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                    .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
            )
            .init();
        let env = doctor::Env {
            data_dir,
            dev_guild: dev_guild(std::env::var("DEV_GUILD_ID").ok().as_deref()),
            now_unix: leaf_server::api::auth::now_unix(),
        };
        return Ok(doctor::main(doctor_args, env).await);
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    let config_path = data_dir.join(CONFIG_FILE_NAME);

    let bind: SocketAddr = env_or("BIND_ADDR", "0.0.0.0:3777")
        .parse()
        .context("BIND_ADDR must be a socket address like 0.0.0.0:3777")?;

    let reconfigure = args.iter().any(|a| a == "--reconfigure");

    let config = match Tier1Config::load(&config_path)? {
        Some(cfg) if !reconfigure => cfg,
        existing => {
            if existing.is_some() {
                info!("--reconfigure: entering setup mode over existing config");
            }
            if run_setup_mode(config_path.clone(), bind).await? == SetupEnd::Stopped {
                // An ordinary stop before setup was finished: nothing failed.
                info!("stopped before setup was completed; nothing was changed");
                return Ok(ExitCode::SUCCESS);
            }
            Tier1Config::load(&config_path)?
                .context("setup completed but config failed to load back")?
        }
    };

    run_mode(&data_dir, bind, config).await?;
    Ok(ExitCode::SUCCESS)
}

/// How setup mode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetupEnd {
    /// The configuration was written: run mode is next.
    Configured,
    /// leaf was told to stop first (Ctrl-C, `docker stop`).
    Stopped,
}

/// Serves the setup flow until configuration is written or leaf is told to
/// stop, and says which.
async fn run_setup_mode(config_path: PathBuf, bind: SocketAddr) -> anyhow::Result<SetupEnd> {
    let validator = leaf_server::validate::LiveValidator::new().context("building HTTP client")?;
    let setup = leaf_server::setup::setup_mode(config_path, validator);

    info!("no configuration found — starting in setup mode");
    info!(
        "→ open http://localhost:{}/setup (or your mapped host)",
        bind.port()
    );
    info!("→ setup code: {}", setup.code);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;

    let mut done = setup.done_rx.clone();
    let finished = setup.done_rx.clone();
    axum::serve(listener, setup.router)
        .with_graceful_shutdown(async move {
            // Completes when setup succeeds (or the process is told to stop).
            let configured = async move {
                while !*done.borrow_and_update() {
                    if done.changed().await.is_err() {
                        return;
                    }
                }
            };
            tokio::select! {
                () = configured => info!("setup complete — transitioning to run mode"),
                () = shutdown_signal() => info!("shutdown requested during setup"),
            }
        })
        .await
        .context("setup server failed")?;
    // Read after the server has drained: a submit that was being answered
    // when the stop arrived still counts.
    Ok(if *finished.borrow() {
        SetupEnd::Configured
    } else {
        SetupEnd::Stopped
    })
}

/// Normal operation: database, HTTP server, and the gateway bot. A gateway
/// failure (e.g. token revoked after setup) is logged loudly, shown on
/// `GET /api/status` and retried when it may pass, but does not take the
/// HTTP server down; a server failure ends the process.
async fn run_mode(
    data_dir: &std::path::Path,
    bind: SocketAddr,
    config: Tier1Config,
) -> anyhow::Result<()> {
    let db_path = data_dir.join("leaf.db");
    let pool = leaf_core::db::connect(&db_path)
        .await
        .with_context(|| format!("opening database {}", db_path.display()))?;
    info!(db = %db_path.display(), "database ready");

    // One shutdown signal fans out to the server and the gateway.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tokio::spawn(async move {
        shutdown_signal().await;
        let _send = shutdown_tx.send(true);
    });

    let store = leaf_core::media::r2_store(&config.r2).context("opening media storage")?;
    let media =
        leaf_core::media::MediaPipeline::new(store.clone()).context("building media pipeline")?;

    // REST API state shared by the run-mode router.
    let discord = leaf_server::api::discord::LiveDiscord::new(
        &config.client_id,
        &config.client_secret,
        &config.discord_token,
    )
    .context("building Discord API client")?;
    let api_state = leaf_server::api::state::ApiState {
        series: leaf_core::db::SeriesRepo::new(pool.clone()),
        posts: leaf_core::db::PostRepo::new(pool.clone()),
        guilds: leaf_core::db::GuildSettingsRepo::new(pool.clone()),
        store,
        key: leaf_server::api::auth::SessionKey::derive(&config.client_secret),
        discord: std::sync::Arc::new(discord),
        membership: leaf_server::api::state::membership_cache(),
        channels: leaf_server::api::state::channels_cache(),
        redirect_uri: config.public_url.clone(),
        client_id: config.client_id.clone(),
    };

    let bot_cfg = leaf_bot::BotConfig {
        token: config.discord_token.clone(),
        dev_guild: dev_guild_id(),
        public_url: Some(config.public_url.clone()),
    };
    // The bot connects, and reconnects, on its own task: the HTTP server
    // (and setup-mode recovery via --reconfigure) must stay reachable on a
    // dead gateway. Where the connection stands is on `GET /api/status`.
    let bot = tokio::spawn(leaf_bot::supervise(
        bot_cfg,
        pool.clone(),
        media,
        shutdown_rx.clone(),
    ));

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    info!(%bind, "serving");

    let mut server_shutdown = shutdown_rx;
    axum::serve(listener, leaf_server::run::router(api_state))
        .with_graceful_shutdown(async move {
            while !*server_shutdown.borrow_and_update() {
                if server_shutdown.changed().await.is_err() {
                    return;
                }
            }
        })
        .await
        .context("server failed")?;

    // Server is down (shutdown); wait for the gateway task to drain.
    if let Err(e) = bot.await {
        tracing::error!(error = %e, "gateway task panicked");
    }

    pool.close().await;
    info!("shut down cleanly");
    Ok(())
}

/// What `DEV_GUILD_ID` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevGuild {
    /// Unset or empty: commands register globally.
    Unset,
    /// Commands register in this one server.
    Guild(u64),
    /// Set to something that is not a server id: commands register
    /// globally, and that is said rather than done silently.
    NotAnId,
}

/// Reads the value of `DEV_GUILD_ID`.
fn dev_guild(raw: Option<&str>) -> DevGuild {
    let raw = raw.map_or("", str::trim);
    if raw.is_empty() {
        return DevGuild::Unset;
    }
    match raw.parse::<u64>() {
        Ok(id) if id != 0 => DevGuild::Guild(id),
        _ => DevGuild::NotAnId,
    }
}

/// `DEV_GUILD_ID`, when set to a guild id. A value that is not one is
/// reported instead of silently registering commands globally.
fn dev_guild_id() -> Option<u64> {
    match dev_guild(std::env::var("DEV_GUILD_ID").ok().as_deref()) {
        DevGuild::Unset => None,
        DevGuild::Guild(id) => Some(id),
        DevGuild::NotAnId => {
            tracing::warn!("DEV_GUILD_ID is not a server id; registering commands globally");
            None
        }
    }
}

/// Resolves on SIGINT (Ctrl-C) or SIGTERM (docker stop).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_guild_id_is_a_server_id_or_is_said_not_to_be() {
        assert_eq!(dev_guild(None), DevGuild::Unset);
        assert_eq!(dev_guild(Some("")), DevGuild::Unset);
        assert_eq!(dev_guild(Some("  ")), DevGuild::Unset);
        assert_eq!(
            dev_guild(Some(" 900000000000000001 ")),
            DevGuild::Guild(900_000_000_000_000_001)
        );
        assert_eq!(dev_guild(Some("0")), DevGuild::NotAnId);
        assert_eq!(dev_guild(Some("my-server")), DevGuild::NotAnId);
        assert_eq!(dev_guild(Some("-5")), DevGuild::NotAnId);
    }
}
