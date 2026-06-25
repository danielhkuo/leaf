//! Setup mode: the one-time bootstrap flow that collects Tier-1 secrets.
//!
//! Served when no `leaf.conf` exists (or `--reconfigure` was passed). The
//! page is gated by a one-time setup code printed to the logs — Discord
//! OAuth cannot protect this page because the bot is, by definition, not
//! configured yet. After `MAX_ATTEMPTS` wrong codes the flow locks until
//! the process restarts.

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use leaf_core::config::{R2Config, Tier1Config};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, watch};

/// Wrong-code attempts allowed before the flow locks until restart.
pub const MAX_ATTEMPTS: u32 = 10;

/// Alphabet for setup codes: unambiguous uppercase + digits (no 0/O/1/I).
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generates a setup code of the form `XXXX-XXXX`.
#[must_use]
pub fn generate_code() -> String {
    use rand::Rng as _;
    let mut rng = rand::rng();
    let mut pick = |n: usize| -> String {
        (0..n)
            .map(|_| {
                let i = rng.random_range(0..CODE_ALPHABET.len());
                char::from(*CODE_ALPHABET.get(i).unwrap_or(&b'X'))
            })
            .collect()
    };
    let (a, b) = (pick(4), pick(4));
    format!("{a}-{b}")
}

/// Normalizes user input for comparison against the generated code.
fn normalize_code(raw: &str) -> String {
    raw.trim().to_ascii_uppercase().replace('-', "")
}

/// Validates credentials against the live services. Implemented for real
/// by [`crate::validate::LiveValidator`]; tests substitute mocks.
pub trait CredentialValidator: Send + Sync + 'static {
    /// Checks the bot token and the OAuth client id/secret pair.
    fn validate_discord(
        &self,
        token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> impl Future<Output = Result<(), String>> + Send;

    /// Checks the R2 credentials with a canary put/get/delete.
    fn validate_r2(&self, r2: &R2Config) -> impl Future<Output = Result<(), String>> + Send;
}

struct Shared {
    code: String,
    config_path: PathBuf,
    /// Wrong-attempt counter; `None` once completed (further submits 410).
    attempts: Mutex<Option<u32>>,
    done_tx: watch::Sender<bool>,
}

/// State for the setup router; cheap to clone.
pub struct SetupApp<V> {
    shared: Arc<Shared>,
    validator: Arc<V>,
}

impl<V> Clone for SetupApp<V> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
            validator: Arc::clone(&self.validator),
        }
    }
}

/// Everything `main` needs to run setup mode.
pub struct SetupMode<V> {
    /// The router to serve.
    pub router: Router,
    /// The code the operator must enter (print it to the logs).
    pub code: String,
    /// Resolves to `true` when configuration completed successfully.
    pub done_rx: watch::Receiver<bool>,
    _marker: std::marker::PhantomData<V>,
}

/// Builds setup mode: router + one-time code + completion signal.
pub fn setup_mode<V: CredentialValidator>(config_path: PathBuf, validator: V) -> SetupMode<V> {
    let code = generate_code();
    let (done_tx, done_rx) = watch::channel(false);
    let app = SetupApp {
        shared: Arc::new(Shared {
            code: normalize_code(&code),
            config_path,
            attempts: Mutex::new(Some(0)),
            done_tx,
        }),
        validator: Arc::new(validator),
    };

    let router = Router::new()
        .route("/", get(|| async { Redirect::temporary("/setup") }))
        .route("/setup", get(page))
        .route("/setup/fonts/{file}", get(font))
        .route("/setup/api/verify-code", post(verify_code::<V>))
        .route("/setup/api/submit", post(submit::<V>))
        .with_state(app);

    SetupMode {
        router,
        code,
        done_rx,
        _marker: std::marker::PhantomData,
    }
}

async fn page() -> Html<&'static str> {
    Html(include_str!("setup_page.html"))
}

/// Self-hosted display + body fonts (OFL, vendored under `src/fonts/`) that the
/// setup page references, so the bootstrap UI matches the gallery's look while
/// staying fully offline. Embedded in the binary; served by exact filename.
const FRAUNCES_WOFF2: &[u8] = include_bytes!("fonts/fraunces-latin-wght.woff2");
const DM_SANS_WOFF2: &[u8] = include_bytes!("fonts/dm-sans-latin-wght.woff2");

/// Serves a vendored woff2 by exact name (no path traversal); 404 otherwise.
async fn font(Path(file): Path<String>) -> Response {
    let body: &'static [u8] = match file.as_str() {
        "fraunces-latin-wght.woff2" => FRAUNCES_WOFF2,
        "dm-sans-latin-wght.woff2" => DM_SANS_WOFF2,
        _ => return StatusCode::NOT_FOUND.into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        body,
    )
        .into_response()
}

/// The submit payload (field names mirror the form).
#[derive(Debug, Deserialize)]
pub struct SubmitRequest {
    /// One-time code from the logs.
    pub setup_code: String,
    /// Discord bot token.
    pub discord_token: String,
    /// Discord application id.
    pub client_id: String,
    /// Discord OAuth client secret.
    pub client_secret: String,
    /// Public origin for the embedded app.
    pub public_url: String,
    /// R2 endpoint URL.
    pub r2_endpoint: String,
    /// R2 bucket.
    pub r2_bucket: String,
    /// R2 access key id.
    pub r2_access_key_id: String,
    /// R2 secret access key.
    pub r2_secret_access_key: String,
}

/// One field-scoped validation error.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FieldError {
    /// Form field name.
    pub field: &'static str,
    /// Human-readable message.
    pub message: String,
}

/// Submit outcome.
#[derive(Debug, Serialize)]
pub struct SubmitResponse {
    /// True when config was validated and written.
    pub ok: bool,
    /// Field errors when `ok` is false.
    pub errors: Vec<FieldError>,
}

fn err(field: &'static str, message: impl Into<String>) -> FieldError {
    FieldError {
        field,
        message: message.into(),
    }
}

fn fail(status: StatusCode, error: FieldError) -> (StatusCode, Json<SubmitResponse>) {
    (
        status,
        Json(SubmitResponse {
            ok: false,
            errors: vec![error],
        }),
    )
}

/// Builds the candidate config from the (trimmed) form fields.
fn config_from(req: &SubmitRequest) -> Tier1Config {
    Tier1Config {
        discord_token: req.discord_token.trim().to_owned(),
        client_id: req.client_id.trim().to_owned(),
        client_secret: req.client_secret.trim().to_owned(),
        public_url: req.public_url.trim().trim_end_matches('/').to_owned(),
        r2: R2Config {
            endpoint: req.r2_endpoint.trim().to_owned(),
            bucket: req.r2_bucket.trim().to_owned(),
            access_key_id: req.r2_access_key_id.trim().to_owned(),
            secret_access_key: req.r2_secret_access_key.trim().to_owned(),
        },
    }
}

/// Checks the setup code against the shared attempt budget. Single source
/// of the gate semantics for both `verify-code` and `submit`: completed →
/// 410, locked → 429, wrong → 401 and one attempt consumed.
fn gate_code(
    attempts: &mut Option<u32>,
    expected: &str,
    raw: &str,
) -> Result<(), (StatusCode, Json<SubmitResponse>)> {
    let Some(count) = attempts.as_mut() else {
        return Err(fail(
            StatusCode::GONE,
            err("setup_code", "setup already completed"),
        ));
    };
    if *count >= MAX_ATTEMPTS {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            err(
                "setup_code",
                "too many attempts; restart leaf to get a new code",
            ),
        ));
    }
    if normalize_code(raw) != expected {
        *count += 1;
        let remaining = MAX_ATTEMPTS - *count;
        tracing::warn!(remaining, "setup: wrong code entered");
        return Err(fail(
            StatusCode::UNAUTHORIZED,
            err(
                "setup_code",
                format!("wrong code ({remaining} attempts left)"),
            ),
        ));
    }
    Ok(())
}

/// Payload for the pre-flight code check.
#[derive(Debug, Deserialize)]
struct VerifyRequest {
    setup_code: String,
}

/// Pre-flight code check so the UI can reveal the credential form only
/// after a valid code. Consumes attempts on failure exactly like `submit`;
/// success reserves nothing (the code is re-checked at submit).
async fn verify_code<V: CredentialValidator>(
    State(app): State<SetupApp<V>>,
    Json(req): Json<VerifyRequest>,
) -> (StatusCode, Json<SubmitResponse>) {
    let mut attempts = app.shared.attempts.lock().await;
    match gate_code(&mut attempts, &app.shared.code, &req.setup_code) {
        Ok(()) => (
            StatusCode::OK,
            Json(SubmitResponse {
                ok: true,
                errors: Vec::new(),
            }),
        ),
        Err(resp) => resp,
    }
}

/// Maps a shape-validation error to the form field it belongs to.
fn shape_error_field(e: &leaf_core::config::ConfigError) -> &'static str {
    match e {
        leaf_core::config::ConfigError::EmptyField(name) => match *name {
            "discord_token" => "discord_token",
            "client_id" => "client_id",
            "client_secret" => "client_secret",
            "public_url" => "public_url",
            "r2.endpoint" => "r2_endpoint",
            "r2.bucket" => "r2_bucket",
            "r2.access_key_id" => "r2_access_key_id",
            _ => "r2_secret_access_key",
        },
        _ => "public_url",
    }
}

#[allow(
    clippy::significant_drop_tightening,
    reason = "attempts guard intentionally spans validation: serializes concurrent submits"
)]
async fn submit<V: CredentialValidator>(
    State(app): State<SetupApp<V>>,
    Json(req): Json<SubmitRequest>,
) -> (StatusCode, Json<SubmitResponse>) {
    let mut attempts = app.shared.attempts.lock().await;

    if let Err(resp) = gate_code(&mut attempts, &app.shared.code, &req.setup_code) {
        return resp;
    }

    let cfg = config_from(&req);

    // Shape validation first: cheap, precise field errors.
    if let Err(e) = cfg.validate() {
        return fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            err(shape_error_field(&e), e.to_string()),
        );
    }

    // Liveness validation: hit Discord and R2 for real.
    let mut errors = Vec::new();
    if let Err(msg) = app
        .validator
        .validate_discord(&cfg.discord_token, &cfg.client_id, &cfg.client_secret)
        .await
    {
        errors.push(err("discord_token", msg));
    }
    if let Err(msg) = app.validator.validate_r2(&cfg.r2).await {
        errors.push(err("r2_endpoint", msg));
    }
    if !errors.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(SubmitResponse { ok: false, errors }),
        );
    }

    if let Err(e) = cfg.save(&app.shared.config_path) {
        tracing::error!(error = %e, "setup: failed to persist config");
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            err("setup_code", format!("could not write config: {e}")),
        );
    }

    *attempts = None; // single-use: no further submissions
    tracing::info!("setup: configuration validated and written");
    let _send_result = app.shared.done_tx.send(true);

    (
        StatusCode::OK,
        Json(SubmitResponse {
            ok: true,
            errors: Vec::new(),
        }),
    )
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use std::sync::atomic::{AtomicU32, Ordering};

    use axum::body::Body;
    use axum::http::{Request, header};
    use tower::ServiceExt as _;

    use super::*;

    /// Mock validator with scriptable outcomes and call counting.
    struct Mock {
        discord_ok: bool,
        r2_ok: bool,
        discord_calls: AtomicU32,
    }

    impl Mock {
        const fn ok() -> Self {
            Self {
                discord_ok: true,
                r2_ok: true,
                discord_calls: AtomicU32::new(0),
            }
        }
    }

    impl CredentialValidator for Arc<Mock> {
        async fn validate_discord(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            self.discord_calls.fetch_add(1, Ordering::SeqCst);
            if self.discord_ok {
                Ok(())
            } else {
                Err("bad token".to_owned())
            }
        }

        async fn validate_r2(&self, _: &R2Config) -> Result<(), String> {
            if self.r2_ok {
                Ok(())
            } else {
                Err("bucket unreachable".to_owned())
            }
        }
    }

    fn body_json(code: &str) -> String {
        serde_json::json!({
            "setup_code": code,
            "discord_token": "tok",
            "client_id": "123",
            "client_secret": "sec",
            "public_url": "https://leaf.example.com",
            "r2_endpoint": "https://acc.r2.cloudflarestorage.com",
            "r2_bucket": "leaf",
            "r2_access_key_id": "ak",
            "r2_secret_access_key": "sk",
        })
        .to_string()
    }

    async fn post_json(
        router: &Router,
        path: &str,
        body: String,
    ) -> (StatusCode, serde_json::Value) {
        let resp = router
            .clone()
            .oneshot(
                Request::post(path)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    async fn post_submit(router: &Router, body: String) -> (StatusCode, serde_json::Value) {
        post_json(router, "/setup/api/submit", body).await
    }

    async fn post_verify(router: &Router, code: &str) -> (StatusCode, serde_json::Value) {
        post_json(
            router,
            "/setup/api/verify-code",
            serde_json::json!({ "setup_code": code }).to_string(),
        )
        .await
    }

    fn fixture(mock: Arc<Mock>) -> (tempfile::TempDir, SetupMode<Arc<Mock>>, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("leaf.conf");
        let mode = setup_mode(path.clone(), mock);
        (dir, mode, path)
    }

    #[tokio::test]
    async fn verify_code_gates_the_form() {
        let (_dir, mode, path) = fixture(Arc::new(Mock::ok()));

        // Right code verifies, reserves nothing, and submit still works.
        let (status, json) = post_verify(&mode.router, &mode.code).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
        let (status, _) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(path.exists());

        // After completion the verify endpoint reports GONE too.
        let (status, _) = post_verify(&mode.router, &mode.code).await;
        assert_eq!(status, StatusCode::GONE);
    }

    #[tokio::test]
    async fn verify_attempts_share_the_submit_budget() {
        let (_dir, mode, path) = fixture(Arc::new(Mock::ok()));

        for _ in 0..MAX_ATTEMPTS {
            let (status, _) = post_verify(&mode.router, "WRONG-CODE").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        // Budget exhausted via verify → submit is locked even with the
        // correct code: verify cannot be used as a free brute-force oracle.
        let (status, _) = post_verify(&mode.router, &mode.code).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        let (status, _) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(!path.exists());
    }

    #[test]
    fn generated_codes_have_expected_shape() {
        for _ in 0..100 {
            let code = generate_code();
            assert_eq!(code.len(), 9);
            assert!(
                code.chars()
                    .all(|c| c == '-' || CODE_ALPHABET.contains(&(c as u8)))
            );
            assert!(!code.contains('0') && !code.contains('O'));
        }
    }

    #[tokio::test]
    async fn happy_path_writes_config_and_signals_done() {
        let (_dir, mode, path) = fixture(Arc::new(Mock::ok()));
        let (status, json) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
        assert!(path.exists());
        assert!(*mode.done_rx.borrow());

        // Lowercase + dashes-stripped code also accepted (normalization).
        let (_dir, mode, _path) = fixture(Arc::new(Mock::ok()));
        let lowered = mode.code.to_ascii_lowercase().replace('-', "");
        let (status, _) = post_submit(&mode.router, body_json(&lowered)).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn wrong_code_rejected_then_locked_after_max_attempts() {
        let mock = Arc::new(Mock::ok());
        let (_dir, mode, path) = fixture(Arc::clone(&mock));

        for _ in 0..MAX_ATTEMPTS {
            let (status, _) = post_submit(&mode.router, body_json("WRONG-CODE")).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        // Locked now — even the *correct* code is refused.
        let (status, json) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert!(
            json["errors"][0]["message"]
                .as_str()
                .unwrap()
                .contains("restart")
        );
        assert!(!path.exists());
        // The validator was never reached with a bad code.
        assert_eq!(mock.discord_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn validator_failures_become_field_errors_and_nothing_is_written() {
        let mock = Arc::new(Mock {
            discord_ok: false,
            r2_ok: false,
            discord_calls: AtomicU32::new(0),
        });
        let (_dir, mode, path) = fixture(mock);
        let (status, json) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["errors"].as_array().unwrap().len(), 2);
        assert!(!path.exists());
        assert!(!*mode.done_rx.borrow());
    }

    #[tokio::test]
    async fn shape_errors_short_circuit_before_live_validation() {
        let mock = Arc::new(Mock::ok());
        let (_dir, mode, _path) = fixture(Arc::clone(&mock));
        let mut body: serde_json::Value = serde_json::from_str(&body_json(&mode.code)).unwrap();
        body["discord_token"] = serde_json::Value::String(String::new());
        let (status, json) = post_submit(&mode.router, body.to_string()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(json["errors"][0]["field"], "discord_token");
        assert_eq!(mock.discord_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn completed_setup_refuses_further_submissions() {
        let (_dir, mode, _path) = fixture(Arc::new(Mock::ok()));
        let (status, _) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::GONE);
    }

    #[tokio::test]
    async fn root_redirects_and_page_serves() {
        let (_dir, mode, _path) = fixture(Arc::new(Mock::ok()));
        let resp = mode
            .router
            .clone()
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert!(resp.status().is_redirection());

        let resp = mode
            .router
            .clone()
            .oneshot(Request::get("/setup").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
