//! Run-mode router: the REST API plus health and a placeholder landing
//! page. The Svelte gallery's static assets mount here in a later phase.

use axum::Router;
use axum::response::Html;
use axum::routing::get;

use crate::api::auth::DiscordApi;
use crate::api::state::ApiState;

/// Builds the run-mode router, merging the API routes over `state`.
pub fn router<D: DiscordApi>(state: ApiState<D>) -> Router {
    Router::new()
        .route("/", get(placeholder))
        .route("/healthz", get(|| async { "ok" }))
        .merge(crate::api::router(state))
        .layer(tower_http::trace::TraceLayer::new_for_http())
}

async fn placeholder() -> Html<&'static str> {
    Html(
        "<!doctype html><meta charset=utf-8><title>leaf</title>\
         <body style=\"background:#101410;color:#d8e4d8;font:16px system-ui;\
         display:grid;place-items:center;min-height:100vh\">\
         <div style=\"text-align:center\"><div style=\"font-size:3rem\">🍃</div>\
         leaf is running. The gallery arrives in a later phase.</div>",
    )
}
