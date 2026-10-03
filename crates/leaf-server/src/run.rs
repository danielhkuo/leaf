//! Run-mode router: API, health, and the embedded gallery's static assets.
//!
//! The Svelte app is built to `activity/dist` and served from `STATIC_DIR`
//! (default `activity/dist`); when that build is absent — e.g. a backend-only
//! `cargo run` — a placeholder page stands in at `/`.
//!
//! Three kinds of path never fall through to the gallery's `index.html`:
//! `/api/…` (unknown routes answer in the API's JSON error shape),
//! `/assets/…` (a missing file is a plain 404, never HTML with a year-long
//! cache lifetime), and the setup flow's addresses, which only exist in
//! setup mode.

use std::path::{Path, PathBuf};

use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{any, get};
use axum::{Json, Router, middleware};
use serde::Serialize;
use tower_http::services::{ServeDir, ServeFile};

use crate::api::auth::DiscordApi;
use crate::api::error::ApiError;
use crate::api::state::ApiState;

/// Where Vite writes its content-hashed files: this directory under the
/// static directory, served at `/assets`.
const ASSETS_DIR: &str = "assets";

/// A hashed asset's name changes whenever its bytes do, so a cached copy is
/// good for as long as anything still asks for it.
const CACHE_FOREVER: &str = "public, max-age=31536000, immutable";

/// The HTML shell names the current hashed assets, so a cached copy has to be
/// checked against the server before each use (a cheap 304 when unchanged).
/// Without this a browser may reuse an old shell for days after an upgrade
/// and ask for assets that no longer exist.
const REVALIDATE: &str = "no-cache";

/// Builds the run-mode router: API + health, with the gallery (or a
/// placeholder) mounted underneath as the fallback.
pub fn router<D: DiscordApi>(state: ApiState<D>) -> Router {
    let api = service_routes().merge(crate::api::router(state));

    with_frontend(api, &static_dir()).layer(tower_http::trace::TraceLayer::new_for_http())
}

/// The routes that need no API state: health, runtime status, and the JSON
/// answers for addresses that must not be served the gallery's HTML.
fn service_routes() -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/api/status", get(status))
        // Real API routes are more specific than these and win; what is left
        // is a path this server does not have.
        .route("/api", any(api_not_found))
        .route("/api/", any(api_not_found))
        .route("/api/{*rest}", any(api_not_found))
        .route("/setup/api/{*rest}", any(setup_finished))
}

/// Body of `GET /api/status`.
#[derive(Debug, Serialize)]
struct StatusBody {
    /// `starting`, `online` or `error`.
    gateway: &'static str,
    /// What went wrong, when `gateway` is `error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    /// A problem that leaves the bot connected but needs attention (for
    /// example a command list Discord has not accepted), as a sentence.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
}

/// `GET /api/status` — where the bot's connection to Discord stands.
///
/// `/healthz` only says the HTTP server is up, which stays true while the
/// bot is offline; the setup page and the admin panel read this to say so.
/// Open to anyone who can reach the server: the setup page has no session
/// yet, and the detail is a sentence the bot wrote for display (see
/// `leaf_core::status`). Never cached, so a poll always sees the present.
async fn status() -> impl IntoResponse {
    let state = leaf_core::status::gateway();
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(StatusBody {
            gateway: state.as_str(),
            detail: state.detail().map(ToOwned::to_owned),
            notice: leaf_core::status::notice(),
        }),
    )
}

/// Any `/api/…` path no route claims. Left to the gallery fallback it would
/// be answered with `index.html` and a 200, which a client built for a newer
/// or older server cannot tell from a real answer.
async fn api_not_found() -> ApiError {
    ApiError::NotFound
}

/// `/setup/api/…` once leaf is configured. A setup tab left open from before
/// keeps posting here; this answers in the setup API's own shape (see
/// `setup::SubmitResponse`) so the page shows the sentence instead of
/// failing to read the gallery's HTML.
async fn setup_finished() -> impl IntoResponse {
    (
        StatusCode::GONE,
        Json(serde_json::json!({
            "ok": false,
            "errors": [{
                "field": "setup_code",
                "message": "leaf is already set up, so there is nothing to enter here. \
                            Open /admin to manage it, or restart leaf with --reconfigure \
                            to enter new credentials.",
            }],
        })),
    )
}

/// The directory the built gallery is served from.
fn static_dir() -> PathBuf {
    std::env::var_os("STATIC_DIR").map_or_else(|| PathBuf::from("activity/dist"), PathBuf::from)
}

/// Mounts the built SPA beneath `api` when present, else a placeholder at `/`.
fn with_frontend(api: Router, dir: &Path) -> Router {
    let index = dir.join("index.html");
    if index.is_file() {
        // Hashed assets: served as files only (a directory is a 404, not its
        // index.html), and cacheable for good.
        let assets = Router::new()
            .fallback_service(
                ServeDir::new(dir.join(ASSETS_DIR)).append_index_html_on_directories(false),
            )
            .layer(middleware::map_response(cache_forever));
        // Everything else falls back to index.html so client-side routes the
        // server doesn't know about still load the app.
        let shell = Router::new()
            .fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
            .layer(middleware::map_response(revalidate_html));
        // `/setup` is the setup-mode page; once configured, the browser page
        // to send a server owner to is the admin panel. Temporary on purpose:
        // `--reconfigure` serves the real page at this address again.
        api.route("/setup", get(|| async { Redirect::temporary("/admin") }))
            .nest_service("/assets", assets)
            .fallback_service(shell)
    } else {
        // No admin panel without the build, so `/setup` lands on the
        // placeholder instead.
        api.route("/", get(placeholder))
            .route("/setup", get(|| async { Redirect::temporary("/") }))
    }
}

/// Marks a served asset (or the 304 that confirms a cached one) as immutable.
/// Misses and errors are left alone so a 404 is never remembered for a year.
async fn cache_forever(mut response: Response) -> Response {
    if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            HeaderValue::from_static(CACHE_FOREVER),
        );
    }
    response
}

/// Makes browsers check the HTML shell with the server before reusing it.
async fn revalidate_html(mut response: Response) -> Response {
    let is_html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/html"));
    if is_html {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(REVALIDATE));
    }
    response
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; JSON indexing is fine in assertions"
    )]

    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Method, Request};
    use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo};
    use leaf_core::domain::{
        Cadence, DetectionMode, NewMediaAttachment, NewSeries, Post, Privacy, SeriesState,
    };
    use leaf_core::status::{GatewayState, set_gateway};
    use object_store::ObjectStore;
    use object_store::memory::InMemory;
    use tower::ServiceExt;

    use super::*;
    use crate::api::auth::{SessionKey, now_unix};
    use crate::api::discord::LiveDiscord;

    const INDEX_HTML: &str = "<!doctype html><title>leaf</title><div id=app></div>";
    const ASSET_JS: &str = "console.log('leaf')";
    const VIDEO_BYTES: &[u8] = b"0123456789";

    /// A built gallery on disk: `index.html` plus one hashed asset.
    fn dist() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), INDEX_HTML).unwrap();
        let assets = dir.path().join(ASSETS_DIR);
        std::fs::create_dir(&assets).unwrap();
        std::fs::write(assets.join("entry-abc123.js"), ASSET_JS).unwrap();
        dir
    }

    /// The state-free routes over a built gallery.
    fn app(dist: &tempfile::TempDir) -> Router {
        with_frontend(service_routes(), dist.path())
    }

    async fn send(app: &Router, request: Request<Body>) -> Response {
        app.clone().oneshot(request).await.unwrap()
    }

    async fn fetch(app: &Router, path: &str) -> Response {
        send(app, Request::get(path).body(Body::empty()).unwrap()).await
    }

    fn header_of<'a>(response: &'a Response, name: &header::HeaderName) -> Option<&'a str> {
        response.headers().get(name).map(|v| v.to_str().unwrap())
    }

    async fn text(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 16)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    async fn json(response: Response) -> serde_json::Value {
        serde_json::from_str(&text(response).await).unwrap()
    }

    #[tokio::test]
    async fn hashed_assets_are_immutable_even_when_revalidated() {
        let dist = dist();
        let app = app(&dist);

        let response = fetch(&app, "/assets/entry-abc123.js").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, &header::CACHE_CONTROL),
            Some(CACHE_FOREVER)
        );
        let modified = header_of(&response, &header::LAST_MODIFIED)
            .unwrap()
            .to_owned();
        assert_eq!(text(response).await, ASSET_JS);

        let revalidation = Request::get("/assets/entry-abc123.js")
            .header(header::IF_MODIFIED_SINCE, modified)
            .body(Body::empty())
            .unwrap();
        let response = send(&app, revalidation).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            header_of(&response, &header::CACHE_CONTROL),
            Some(CACHE_FOREVER)
        );
    }

    #[tokio::test]
    async fn a_missing_asset_is_a_404_not_the_app_shell() {
        let dist = dist();
        // Vite's output is flat, but a directory that does turn up must not
        // hand out its index.html, or a redirect that drops the prefix.
        let nested = dist.path().join(ASSETS_DIR).join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(nested.join("index.html"), INDEX_HTML).unwrap();
        let app = app(&dist);
        // A chunk from a previous build: the client must see a failure, and
        // nothing may cache it.
        for path in [
            "/assets/entry-old999.js",
            "/assets",
            "/assets/",
            "/assets/nested",
            "/assets/nested/",
        ] {
            let response = fetch(&app, path).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(header_of(&response, &header::CACHE_CONTROL), None, "{path}");
            assert_ne!(text(response).await, INDEX_HTML, "{path}");
        }
    }

    #[tokio::test]
    async fn the_shell_answers_client_routes_and_is_revalidated() {
        let dist = dist();
        let app = app(&dist);
        for path in ["/", "/index.html", "/admin", "/some/client/route"] {
            let response = fetch(&app, path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header_of(&response, &header::CACHE_CONTROL),
                Some(REVALIDATE),
                "{path}"
            );
            assert_eq!(text(response).await, INDEX_HTML, "{path}");
        }
    }

    #[tokio::test]
    async fn unknown_api_paths_answer_json_404_for_any_method() {
        let dist = dist();
        // A stand-in for the real API: one GET route nested as deep as the
        // real ones, to show the catch-all only takes what is left over.
        let api = service_routes().merge(Router::new().route(
            "/api/guilds/{gid}/series",
            get(|| async { "the real route" }),
        ));
        let app = with_frontend(api, dist.path());

        for path in [
            "/api",
            "/api/",
            "/api/nope",
            "/api/guilds/1/nope",
            "/api/guilds/1/series/2/deeper",
            "/api/statuses",
        ] {
            for method in [Method::GET, Method::POST, Method::PATCH] {
                let request = Request::builder()
                    .method(&method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap();
                let response = send(&app, request).await;
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                assert_eq!(
                    json(response).await,
                    serde_json::json!({ "error": "not_found" }),
                    "{method} {path}"
                );
            }
        }

        let response = fetch(&app, "/api/guilds/1/series").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(text(response).await, "the real route");
        // A known path with the wrong method is still a 405, not a 404.
        let post = Request::post("/api/guilds/1/series")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            send(&app, post).await.status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[tokio::test]
    async fn setup_addresses_point_at_the_admin_panel() {
        let dist = dist();
        let app = app(&dist);

        let response = fetch(&app, "/setup").await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(header_of(&response, &header::LOCATION), Some("/admin"));

        // A setup tab left open posts its code; it gets a sentence to show.
        let post = Request::post("/setup/api/verify-code")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"setup_code":"ABCD-EFGH"}"#))
            .unwrap();
        let response = send(&app, post).await;
        assert_eq!(response.status(), StatusCode::GONE);
        let body = json(response).await;
        assert_eq!(body["ok"], false);
        assert_eq!(body["errors"][0]["field"], "setup_code");
        let message = body["errors"][0]["message"].as_str().unwrap();
        assert!(message.contains("already set up"), "{message}");
        assert!(message.contains("/admin"), "{message}");
    }

    #[tokio::test]
    async fn without_a_build_the_placeholder_stands_in() {
        let empty = tempfile::tempdir().unwrap();
        let app = with_frontend(service_routes(), empty.path());

        let response = fetch(&app, "/").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(text(response).await.contains("leaf is running"));

        let response = fetch(&app, "/setup").await;
        assert_eq!(response.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(header_of(&response, &header::LOCATION), Some("/"));

        let response = fetch(&app, "/api/nope").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json(response).await,
            serde_json::json!({ "error": "not_found" })
        );
        assert_eq!(fetch(&app, "/healthz").await.status(), StatusCode::OK);
    }

    // The gateway state is process-global, so every transition is checked in
    // this one test rather than in several that would race each other.
    #[tokio::test]
    async fn status_reports_the_gateway_state() {
        let dist = dist();
        let app = app(&dist);

        set_gateway(GatewayState::Starting);
        let response = fetch(&app, "/api/status").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, &header::CACHE_CONTROL),
            Some("no-store")
        );
        assert_eq!(
            json(response).await,
            serde_json::json!({ "gateway": "starting" })
        );

        set_gateway(GatewayState::Online);
        assert_eq!(
            json(fetch(&app, "/api/status").await).await,
            serde_json::json!({ "gateway": "online" })
        );

        // Connected, with something the owner should know about.
        leaf_core::status::set_notice(Some("Discord has not accepted the list.".to_owned()));
        assert_eq!(
            json(fetch(&app, "/api/status").await).await,
            serde_json::json!({
                "gateway": "online",
                "notice": "Discord has not accepted the list.",
            })
        );
        leaf_core::status::set_notice(None);

        set_gateway(GatewayState::Error(
            "Discord rejected the bot token.".to_owned(),
        ));
        assert_eq!(
            json(fetch(&app, "/api/status").await).await,
            serde_json::json!({
                "gateway": "error",
                "detail": "Discord rejected the bot token.",
            })
        );
        set_gateway(GatewayState::Starting);
    }

    /// The whole run-mode router over a seeded database and store: one
    /// public series whose Day 1 holds the "video" `att1`.
    async fn full_router() -> (Router, SessionKey) {
        let dir = tempfile::tempdir().unwrap();
        let pool = leaf_core::db::connect(&dir.path().join("t.db"))
            .await
            .unwrap();
        // Keep the temp DB file alive for the rest of the test process.
        std::mem::forget(dir);

        let guilds = GuildSettingsRepo::new(pool.clone());
        guilds.ensure_exists("g1").await.unwrap();
        let series = SeriesRepo::new(pool.clone());
        let posts = PostRepo::new(pool);
        let created = series
            .create(
                &NewSeries {
                    guild_id: "g1".to_owned(),
                    creator_id: "creator1".to_owned(),
                    name: "public".to_owned(),
                    description: String::new(),
                    channels: vec!["c1".to_owned()],
                    cadence: Cadence::Daily,
                    detection_mode: DetectionMode::ContextMenu,
                    privacy: Privacy::Public,
                    privacy_role_id: None,
                    start_day: 1,
                    state: SeriesState::Active,
                },
                0,
            )
            .await
            .unwrap();
        posts
            .insert_with_media(
                &Post {
                    series_id: created.id,
                    day: 1,
                    message_id: "m1".to_owned(),
                    channel_id: "c1".to_owned(),
                    caption: "Day 1".to_owned(),
                    posted_at: 1000,
                    archived_at: 1001,
                },
                &[NewMediaAttachment {
                    attachment_id: "att1".to_owned(),
                    channel_id: "c1".to_owned(),
                    message_id: "m1".to_owned(),
                    content_type: "video/mp4".to_owned(),
                    original_key: Some("k-orig".to_owned()),
                    thumb_key: None,
                    media_missing: false,
                }],
            )
            .await
            .unwrap();

        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        store
            .put(
                &object_store::path::Path::from("k-orig"),
                bytes::Bytes::from_static(VIDEO_BYTES).into(),
            )
            .await
            .unwrap();

        let key = SessionKey::derive("test-secret");
        let state = ApiState {
            series,
            posts,
            guilds,
            store,
            key: key.clone(),
            // Never called: these tests only reach routes that do not talk
            // to Discord.
            discord: Arc::new(LiveDiscord::new("client-123", "secret", "token").unwrap()),
            membership: crate::api::state::membership_cache(),
            channels: crate::api::state::channels_cache(),
            redirect_uri: "https://leaf.test".to_owned(),
            client_id: "client-123".to_owned(),
        };
        (router(state), key)
    }

    #[tokio::test]
    async fn full_router_builds_and_keeps_the_real_api_routes() {
        // Building it is the first assertion: axum panics on two routes that
        // claim the same path, which would otherwise only show at startup.
        let (app, _key) = full_router().await;

        assert_eq!(fetch(&app, "/healthz").await.status(), StatusCode::OK);
        assert_eq!(fetch(&app, "/api/status").await.status(), StatusCode::OK);
        // A real route still answers as itself (401 without a session)…
        assert_eq!(
            fetch(&app, "/api/guilds/g1/series").await.status(),
            StatusCode::UNAUTHORIZED
        );
        // …and what no route claims is the API's own 404.
        let response = fetch(&app, "/api/guilds/g1/no-such-thing").await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json(response).await,
            serde_json::json!({ "error": "not_found" })
        );
    }

    #[tokio::test]
    async fn media_route_answers_a_range_with_206() {
        let (app, key) = full_router().await;
        let exp = now_unix() + 60;
        let sig = key.sign_media("att1", exp);
        let url = format!("/api/media/att1?exp={exp}&sig={sig}");

        let ranged = |method: Method| {
            Request::builder()
                .method(method)
                .uri(&url)
                .header(header::RANGE, "bytes=2-5")
                .body(Body::empty())
                .unwrap()
        };

        let response = send(&app, ranged(Method::GET)).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_of(&response, &header::CONTENT_RANGE),
            Some("bytes 2-5/10")
        );
        assert_eq!(header_of(&response, &header::CONTENT_LENGTH), Some("4"));
        assert_eq!(header_of(&response, &header::ACCEPT_RANGES), Some("bytes"));
        assert_eq!(
            header_of(&response, &header::CONTENT_TYPE),
            Some("video/mp4")
        );
        assert_eq!(text(response).await, "2345");

        // `curl -I -H 'Range: …'`: the same headers, no body.
        let response = send(&app, ranged(Method::HEAD)).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            header_of(&response, &header::CONTENT_RANGE),
            Some("bytes 2-5/10")
        );
        assert_eq!(header_of(&response, &header::CONTENT_LENGTH), Some("4"));
        assert!(text(response).await.is_empty());

        // No range: the whole file, with its length stated.
        let response = fetch(&app, &url).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(header_of(&response, &header::CONTENT_LENGTH), Some("10"));
        assert_eq!(header_of(&response, &header::ACCEPT_RANGES), Some("bytes"));
        assert_eq!(text(response).await.as_bytes(), VIDEO_BYTES);
    }

    /// Over a real socket: the stated length has to be the framing the
    /// server actually uses (no chunked encoding), because players and edge
    /// caches act on it.
    #[tokio::test]
    async fn media_lengths_hold_on_the_wire() {
        let (app, key) = full_router().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await });

        let exp = now_unix() + 60;
        let sig = key.sign_media("att1", exp);
        let url = format!("http://{addr}/api/media/att1?exp={exp}&sig={sig}");
        // Straight to the loopback listener, whatever proxy the environment
        // names.
        let client = reqwest::Client::builder().no_proxy().build().unwrap();

        let part = client
            .get(&url)
            .header("Range", "bytes=2-5")
            .send()
            .await
            .unwrap();
        assert_eq!(part.status().as_u16(), 206);
        assert_eq!(part.headers()["content-length"], "4");
        assert_eq!(part.headers()["content-range"], "bytes 2-5/10");
        assert!(part.headers().get("transfer-encoding").is_none());
        assert_eq!(part.text().await.unwrap(), "2345");

        let whole = client.get(&url).send().await.unwrap();
        assert_eq!(whole.status().as_u16(), 200);
        assert_eq!(whole.headers()["content-length"], "10");
        assert!(whole.headers().get("transfer-encoding").is_none());
        assert_eq!(whole.bytes().await.unwrap().as_ref(), VIDEO_BYTES);

        let head = client
            .head(&url)
            .header("Range", "bytes=0-1")
            .send()
            .await
            .unwrap();
        assert_eq!(head.status().as_u16(), 206);
        assert_eq!(head.headers()["content-length"], "2");
        assert_eq!(head.headers()["accept-ranges"], "bytes");

        server.abort();
    }
}
