//! Run-mode router: API, health, and the embedded gallery's static assets.
//!
//! The Svelte app is built to `activity/dist` and served from `STATIC_DIR`
//! (default `activity/dist`); when that build is absent — e.g. a backend-only
//! `cargo run` — a placeholder page stands in at `/`.

use std::path::{Path, PathBuf};

use axum::Router;
use axum::response::Html;
use axum::routing::get;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::auth::DiscordApi;
use crate::api::state::ApiState;

/// Builds the run-mode router: API + health, with the gallery (or a
/// placeholder) mounted underneath as the fallback.
pub fn router<D: DiscordApi>(state: ApiState<D>) -> Router {
    let api = Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .merge(crate::api::router(state));

    with_frontend(api, &static_dir()).layer(tower_http::trace::TraceLayer::new_for_http())
}

/// The directory the built gallery is served from.
fn static_dir() -> PathBuf {
    std::env::var_os("STATIC_DIR").map_or_else(|| PathBuf::from("activity/dist"), PathBuf::from)
}

/// Mounts the built SPA beneath `api` when present, else a placeholder at `/`.
fn with_frontend(api: Router, dir: &Path) -> Router {
    let index = dir.join("index.html");
    if index.is_file() {
        // Serve hashed assets directly; fall back to index.html so client-side
        // routes the server doesn't know about still load the app.
        let serve = ServeDir::new(dir).fallback(ServeFile::new(index));
        api.fallback_service(serve)
    } else {
        api.route("/", get(placeholder))
    }
}

async fn placeholder() -> Html<&'static str> {
    Html(
        "<!doctype html><meta charset=utf-8><title>leaf</title>\
         <body style=\"background:#000;color:#b2b6bd;font:16px system-ui;\
         display:grid;place-items:center;min-height:100vh\">\
         <div style=\"text-align:center\"><div style=\"font-size:3rem\">🍃</div>\
         leaf is running. The gallery build isn’t mounted here \
         (set STATIC_DIR or run the Vite dev server).</div>",
    )
}
