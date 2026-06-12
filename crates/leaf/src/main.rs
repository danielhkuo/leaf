//! leaf composition root: one process, two-state boot.
//!
//! The HTTP server always starts first. With no Tier-1 config present (or
//! `--reconfigure`), it serves the setup flow and only transitions to run
//! mode — gateway and all — once credentials are validated and written.
//! See PLAN.md § First-run setup and docs/phases.md Phase 3.

use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context as _;
use leaf_core::config::{CONFIG_FILE_NAME, Tier1Config};
use tracing::info;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LOG_LEVEL")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let data_dir = PathBuf::from(env_or("DATA_DIR", "./data"));
    std::fs::create_dir_all(&data_dir)
        .with_context(|| format!("creating data dir {}", data_dir.display()))?;
    let config_path = data_dir.join(CONFIG_FILE_NAME);

    let bind: SocketAddr = env_or("BIND_ADDR", "0.0.0.0:8080")
        .parse()
        .context("BIND_ADDR must be a socket address like 0.0.0.0:8080")?;

    let reconfigure = std::env::args().any(|a| a == "--reconfigure");

    let config = match Tier1Config::load(&config_path)? {
        Some(cfg) if !reconfigure => cfg,
        existing => {
            if existing.is_some() {
                info!("--reconfigure: entering setup mode over existing config");
            }
            run_setup_mode(config_path.clone(), bind).await?;
            Tier1Config::load(&config_path)?
                .context("setup completed but config failed to load back")?
        }
    };

    run_mode(&data_dir, bind, config).await
}

/// Serves the setup flow until configuration is written, then returns.
async fn run_setup_mode(config_path: PathBuf, bind: SocketAddr) -> anyhow::Result<()> {
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
    Ok(())
}

/// Normal operation: database, HTTP server, and (from Phase 4) the bot.
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

    // Gateway connection lands in Phase 4; config is validated and held.
    info!(client_id = %config.client_id, "run mode (gateway connection arrives in Phase 4)");

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    info!(%bind, "serving");

    axum::serve(listener, leaf_server::run::router())
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("server failed")?;

    pool.close().await;
    info!("shut down cleanly");
    Ok(())
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
