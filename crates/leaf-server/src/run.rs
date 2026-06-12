//! Run-mode router.
//!
//! Grows into the REST API, media proxy, and embedded-app static hosting in
//! Phases 14+; for now it serves health and a placeholder landing page so
//! the boot transition is observable end to end.

use axum::Router;
use axum::response::Html;
use axum::routing::get;

/// Builds the run-mode router.
pub fn router() -> Router {
    Router::new()
        .route("/", get(placeholder))
        .route("/healthz", get(|| async { "ok" }))
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt as _;

    use super::*;

    #[tokio::test]
    async fn health_and_placeholder_respond() {
        let app = router();
        let resp = app
            .clone()
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let resp = app
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
