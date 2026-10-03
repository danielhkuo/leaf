//! Setup mode: the one-time bootstrap flow that collects Tier-1 secrets.
//!
//! Served when no `leaf.conf` exists (or `--reconfigure` was passed). The
//! page is gated by a one-time setup code printed to the logs — Discord
//! OAuth cannot protect this page because the bot is, by definition, not
//! configured yet. After `MAX_ATTEMPTS` wrong codes the flow locks until
//! the process restarts.
//!
//! A submit reports every problem it can find in one answer, each on the
//! field it concerns: every empty or malformed field, then whatever Discord
//! and R2 say about the credentials of each section that is complete.

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

/// Bot permissions the invite link asks for: View Channels (1 << 10), Send
/// Messages (1 << 11), Embed Links (1 << 14), Attach Files (1 << 15), Add
/// Reactions (1 << 6) and Read Message History (1 << 16), as in
/// guide/02-discord.md § 4.
const INVITE_PERMISSIONS: u64 =
    (1 << 10) | (1 << 11) | (1 << 14) | (1 << 15) | (1 << 6) | (1 << 16);

/// Shown when the application ID is not a Discord id. The page's own check
/// uses the same sentence.
const CLIENT_ID_NOT_A_NUMBER: &str = "The application ID is a long number, such as \
    1234567890123456789. Copy it from OAuth2 → Client information.";
const PUBLIC_URL_INVALID: &str =
    "This isn't a web address. Enter it like https://leaf.example.com.";
const PUBLIC_URL_NOT_HTTPS: &str = "Discord needs an HTTPS address (https://…). Plain http:// \
    only works for localhost.";
const ENDPOINT_NOT_HTTPS: &str = "Enter the endpoint starting with https://, such as \
    https://<account-id>.r2.cloudflarestorage.com.";

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
///
/// Both methods report problems on the field they concern (see [`Field`]),
/// as sentences ready for the page, and finish within about 30 seconds.
pub trait CredentialValidator: Send + Sync + 'static {
    /// Checks the bot token, that it belongs to the application `client_id`,
    /// and the OAuth client id/secret pair. Returns every problem found;
    /// empty means all three are good.
    fn validate_discord(
        &self,
        token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> impl Future<Output = Vec<FieldError>> + Send;

    /// Checks the R2 credentials with a canary put/get/delete.
    fn validate_r2(&self, r2: &R2Config) -> impl Future<Output = Result<(), FieldError>> + Send;
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

/// The page holds no data: never let a stale copy outlive an upgrade.
async fn page() -> impl IntoResponse {
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(include_str!("setup_page.html")),
    )
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

/// Where the page shows an error. Serialized as the input's `name`, which is
/// also its error slot's `data-for`; the order is the page's, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
    /// The one-time code (shown at the code boxes).
    SetupCode,
    /// Bot token.
    DiscordToken,
    /// Application (client) id.
    ClientId,
    /// OAuth client secret.
    ClientSecret,
    /// Public URL.
    PublicUrl,
    /// R2 S3 endpoint.
    R2Endpoint,
    /// R2 bucket.
    R2Bucket,
    /// R2 access key id.
    R2AccessKeyId,
    /// R2 secret access key.
    R2SecretAccessKey,
    /// Not about one field (Discord or R2 unreachable, the config could not
    /// be written): shown next to the submit button.
    Form,
}

impl Field {
    const fn is_discord(self) -> bool {
        matches!(
            self,
            Self::DiscordToken | Self::ClientId | Self::ClientSecret
        )
    }

    const fn is_r2(self) -> bool {
        matches!(
            self,
            Self::R2Endpoint | Self::R2Bucket | Self::R2AccessKeyId | Self::R2SecretAccessKey
        )
    }
}

/// One field-scoped validation error.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FieldError {
    /// Where the page shows it.
    pub field: Field,
    /// A plain sentence that says what to change.
    pub message: String,
}

impl FieldError {
    /// An error for `field`.
    pub fn new(field: Field, message: impl Into<String>) -> Self {
        Self {
            field,
            message: message.into(),
        }
    }
}

/// Submit outcome.
#[derive(Debug, Serialize)]
pub struct SubmitResponse {
    /// True when config was validated and written.
    pub ok: bool,
    /// Field errors when `ok` is false, in page order.
    pub errors: Vec<FieldError>,
    /// After a successful submit: what is left to do, with the operator's
    /// own values filled in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<NextSteps>,
}

impl SubmitResponse {
    const fn success(next: Option<NextSteps>) -> Self {
        Self {
            ok: true,
            errors: Vec::new(),
            next,
        }
    }

    const fn failure(errors: Vec<FieldError>) -> Self {
        Self {
            ok: false,
            errors,
            next: None,
        }
    }
}

/// The success screen's checklist values, derived from the saved config.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct NextSteps {
    /// Adds the bot to a server with the commands and the permissions leaf
    /// uses.
    pub invite_url: String,
    /// This application in the Developer Portal.
    pub portal_url: String,
    /// The OAuth redirects to register: the gallery's token exchange and
    /// the admin login.
    pub redirects: Vec<String>,
    /// Activities → URL Mappings target for prefix `/`: the public host,
    /// without scheme or slash.
    pub url_mapping_target: String,
    /// The admin panel on the public origin, where its login returns.
    pub admin_url: String,
    /// The public URL is a localhost address, which Discord cannot reach.
    pub local_only: bool,
}

/// The checklist for a validated config. `public_url` is already a bare
/// origin (see [`public_origin`]).
fn next_steps(cfg: &Tier1Config) -> NextSteps {
    let origin = cfg.public_url.as_str();
    let host = origin.split_once("://").map_or(origin, |(_, host)| host);
    NextSteps {
        invite_url: format!(
            "https://discord.com/oauth2/authorize?client_id={}&permissions={INVITE_PERMISSIONS}\
             &integration_type=0&scope=bot+applications.commands",
            cfg.client_id
        ),
        portal_url: format!(
            "https://discord.com/developers/applications/{}",
            cfg.client_id
        ),
        // Must match admin.rs `admin_redirect_uri` and the gallery's default
        // redirect (the bare public URL).
        redirects: vec![origin.to_owned(), format!("{origin}/admin/callback")],
        url_mapping_target: host.to_owned(),
        admin_url: format!("{origin}/admin"),
        local_only: is_local_host(host.split(':').next().unwrap_or(host)),
    }
}

fn is_local_host(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1")
}

/// What an empty field says. The page's own check before sending uses the
/// same sentences.
const fn missing_message(field: Field) -> &'static str {
    match field {
        Field::SetupCode => "Enter the setup code from the logs.",
        Field::DiscordToken => "Enter the bot token.",
        Field::ClientId => "Enter the application ID.",
        Field::ClientSecret => "Enter the client secret.",
        Field::PublicUrl => "Enter the public URL, such as https://leaf.example.com.",
        Field::R2Endpoint => "Enter the S3 endpoint.",
        Field::R2Bucket => "Enter the bucket name.",
        Field::R2AccessKeyId => "Enter the access key ID.",
        Field::R2SecretAccessKey => "Enter the secret access key.",
        Field::Form => "Fill in every field.",
    }
}

/// Checks the public URL and returns it as a bare origin
/// (`https://leaf.example.com`): the form the OAuth redirects and the URL
/// mapping are built from. Accepts a subset of what
/// [`Tier1Config::validate`] accepts, so a passing value always saves.
fn public_origin(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(missing_message(Field::PublicUrl).to_owned());
    }
    if !raw.contains("://") {
        // Suggest the fix with their own host (and without any path). A
        // local leaf serves plain HTTP, so localhost gets http://.
        let host = raw.split(['/', '?', '#']).next().unwrap_or(raw);
        let name = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
        let scheme = if is_local_host(&name) {
            "http"
        } else {
            "https"
        };
        return Err(format!(
            "Start the address with {scheme}://, for example {scheme}://{}.",
            clip(host, 80)
        ));
    }
    let Ok(url) = reqwest::Url::parse(raw) else {
        return Err(PUBLIC_URL_INVALID.to_owned());
    };
    let Some(host) = url.host_str() else {
        return Err(PUBLIC_URL_INVALID.to_owned());
    };
    match url.scheme() {
        "https" => {}
        "http" if is_local_host(host) => {}
        "http" => return Err(PUBLIC_URL_NOT_HTTPS.to_owned()),
        _ => return Err(PUBLIC_URL_INVALID.to_owned()),
    }
    let origin = url.origin().ascii_serialization();
    let extra = url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some();
    if extra {
        return Err(format!("Enter only the address, without a path: {origin}"));
    }
    Ok(origin)
}

/// Whether the R2 endpoint is an `https://` URL with a host. Run mode's
/// store refuses plain HTTP, so anything else could never work.
fn endpoint_ok(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some())
}

/// Checks the shape of every field at once and builds the config to
/// validate live: values trimmed, the public URL reduced to its origin.
fn check_shape(req: &SubmitRequest) -> (Tier1Config, Vec<FieldError>) {
    let mut errors = Vec::new();
    let mut take = |field: Field, raw: &str| -> String {
        let value = raw.trim();
        if value.is_empty() {
            errors.push(FieldError::new(field, missing_message(field)));
        }
        value.to_owned()
    };
    let discord_token = take(Field::DiscordToken, &req.discord_token);
    let client_id = take(Field::ClientId, &req.client_id);
    let client_secret = take(Field::ClientSecret, &req.client_secret);
    let endpoint = take(Field::R2Endpoint, &req.r2_endpoint);
    let bucket = take(Field::R2Bucket, &req.r2_bucket);
    let access_key_id = take(Field::R2AccessKeyId, &req.r2_access_key_id);
    let secret_access_key = take(Field::R2SecretAccessKey, &req.r2_secret_access_key);

    // Discord ids are u64 snowflakes; anything else cannot name an app.
    let snowflake =
        client_id.bytes().all(|b| b.is_ascii_digit()) && client_id.parse::<u64>().is_ok();
    if !client_id.is_empty() && !snowflake {
        errors.push(FieldError::new(Field::ClientId, CLIENT_ID_NOT_A_NUMBER));
    }
    let public_url = public_origin(&req.public_url).unwrap_or_else(|message| {
        errors.push(FieldError::new(Field::PublicUrl, message));
        req.public_url.trim().to_owned()
    });
    if !endpoint.is_empty() && !endpoint_ok(&endpoint) {
        errors.push(FieldError::new(Field::R2Endpoint, ENDPOINT_NOT_HTTPS));
    }

    let cfg = Tier1Config {
        discord_token,
        client_id,
        client_secret,
        public_url,
        r2: R2Config {
            endpoint,
            bucket,
            access_key_id,
            secret_access_key,
        },
    };
    (cfg, errors)
}

/// At most `max` characters of `s`, with an ellipsis when cut.
pub(crate) fn clip(s: &str, max: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

fn fail(status: StatusCode, error: FieldError) -> (StatusCode, Json<SubmitResponse>) {
    (status, Json(SubmitResponse::failure(vec![error])))
}

/// Checks the setup code against the shared attempt budget. Single source
/// of the gate semantics for both `verify-code` and `submit`: completed →
/// 410, locked → 429, wrong → 401 and one attempt consumed.
fn gate_code(
    attempts: &mut Option<u32>,
    expected: &str,
    raw: &str,
) -> Result<(), (StatusCode, FieldError)> {
    let Some(count) = attempts.as_mut() else {
        return Err((
            StatusCode::GONE,
            FieldError::new(
                Field::SetupCode,
                "Setup is already complete. Open /admin to manage leaf.",
            ),
        ));
    };
    if *count >= MAX_ATTEMPTS {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            FieldError::new(
                Field::SetupCode,
                "Too many wrong codes. Restart leaf to get a new one \
                 (docker compose restart leaf), then enter the code from the logs.",
            ),
        ));
    }
    if normalize_code(raw) != expected {
        *count += 1;
        let remaining = MAX_ATTEMPTS - *count;
        tracing::warn!(remaining, "setup: wrong code entered");
        let message = match remaining {
            0 => "That code isn't right, and that was the last try. Restart leaf to get a new \
                  code (docker compose restart leaf)."
                .to_owned(),
            1 => "That code isn't right. Check the logs for the current code (1 try left)."
                .to_owned(),
            n => format!(
                "That code isn't right. Check the logs for the current code ({n} tries left)."
            ),
        };
        return Err((
            StatusCode::UNAUTHORIZED,
            FieldError::new(Field::SetupCode, message),
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
        Ok(()) => (StatusCode::OK, Json(SubmitResponse::success(None))),
        Err((status, error)) => fail(status, error),
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

    if let Err((status, error)) = gate_code(&mut attempts, &app.shared.code, &req.setup_code) {
        return fail(status, error);
    }

    // Shape first: cheap, and covers every field in one answer.
    let (cfg, mut errors) = check_shape(&req);

    // Then the live checks, side by side, for each section whose fields are
    // all present and well formed: a gap in the R2 card does not hold back
    // what Discord has to say, and the reverse.
    let discord_ready = !errors.iter().any(|e| e.field.is_discord());
    let r2_ready = !errors.iter().any(|e| e.field.is_r2());
    let discord = async {
        if discord_ready {
            app.validator
                .validate_discord(&cfg.discord_token, &cfg.client_id, &cfg.client_secret)
                .await
        } else {
            Vec::new()
        }
    };
    let r2 = async {
        if r2_ready {
            app.validator.validate_r2(&cfg.r2).await.err()
        } else {
            None
        }
    };
    let (discord_errors, r2_error) = tokio::join!(discord, r2);
    errors.extend(discord_errors);
    errors.extend(r2_error);
    if !errors.is_empty() {
        // Page order, so the first error is the first one on screen.
        errors.sort_by_key(|e| e.field);
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(SubmitResponse::failure(errors)),
        );
    }

    if let Err(e) = cfg.save(&app.shared.config_path) {
        tracing::error!(error = %e, "setup: failed to persist config");
        return fail(
            StatusCode::INTERNAL_SERVER_ERROR,
            FieldError::new(
                Field::Form,
                "Everything checked out, but leaf couldn't save leaf.conf to its data volume. \
                 Check the volume is mounted and writable, then try again; the logs have the \
                 details (docker compose logs leaf).",
            ),
        );
    }

    *attempts = None; // single-use: no further submissions
    tracing::info!("setup: configuration validated and written");
    let _send_result = app.shared.done_tx.send(true);

    (
        StatusCode::OK,
        Json(SubmitResponse::success(Some(next_steps(&cfg)))),
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
    #[derive(Default)]
    struct Mock {
        discord_errors: Vec<(Field, &'static str)>,
        r2_error: Option<(Field, &'static str)>,
        discord_calls: AtomicU32,
        r2_calls: AtomicU32,
    }

    impl Mock {
        fn ok() -> Self {
            Self::default()
        }
    }

    impl CredentialValidator for Arc<Mock> {
        async fn validate_discord(&self, _: &str, _: &str, _: &str) -> Vec<FieldError> {
            self.discord_calls.fetch_add(1, Ordering::SeqCst);
            self.discord_errors
                .iter()
                .map(|&(field, message)| FieldError::new(field, message))
                .collect()
        }

        async fn validate_r2(&self, _: &R2Config) -> Result<(), FieldError> {
            self.r2_calls.fetch_add(1, Ordering::SeqCst);
            match self.r2_error {
                Some((field, message)) => Err(FieldError::new(field, message)),
                None => Ok(()),
            }
        }
    }

    fn body_value(code: &str) -> serde_json::Value {
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
    }

    fn body_json(code: &str) -> String {
        body_value(code).to_string()
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

    fn fields(json: &serde_json::Value) -> Vec<String> {
        json["errors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["field"].as_str().unwrap().to_owned())
            .collect()
    }

    #[tokio::test]
    async fn verify_code_gates_the_form() {
        let (_dir, mode, path) = fixture(Arc::new(Mock::ok()));

        // Right code verifies, reserves nothing, and submit still works.
        let (status, json) = post_verify(&mode.router, &mode.code).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json["ok"], true);
        assert!(json.get("next").is_none());
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
    async fn success_lists_the_operators_own_next_steps() {
        let (_dir, mode, path) = fixture(Arc::new(Mock::ok()));
        let mut body = body_value(&mode.code);
        body["client_id"] = " 1234567890123456789 ".into();
        body["public_url"] = "HTTPS://Leaf.Example.com/".into();
        let (status, json) = post_submit(&mode.router, body.to_string()).await;
        assert_eq!(status, StatusCode::OK);
        let next = &json["next"];
        assert_eq!(
            next["invite_url"],
            "https://discord.com/oauth2/authorize?client_id=1234567890123456789\
             &permissions=117824&integration_type=0&scope=bot+applications.commands"
        );
        assert_eq!(
            next["portal_url"],
            "https://discord.com/developers/applications/1234567890123456789"
        );
        assert_eq!(
            next["redirects"],
            serde_json::json!([
                "https://leaf.example.com",
                "https://leaf.example.com/admin/callback"
            ])
        );
        assert_eq!(next["url_mapping_target"], "leaf.example.com");
        assert_eq!(next["admin_url"], "https://leaf.example.com/admin");
        assert_eq!(next["local_only"], false);

        // The saved config holds the normalized values the checklist shows.
        let saved = Tier1Config::load(&path).unwrap().unwrap();
        assert_eq!(saved.public_url, "https://leaf.example.com");
        assert_eq!(saved.client_id, "1234567890123456789");
    }

    #[test]
    fn local_public_url_is_flagged_for_the_checklist() {
        let mut cfg = config_with_url("http://localhost:3777");
        let next = next_steps(&cfg);
        assert!(next.local_only);
        assert_eq!(next.url_mapping_target, "localhost:3777");
        assert_eq!(next.admin_url, "http://localhost:3777/admin");

        cfg.public_url = "https://leaf.example.com:8443".to_owned();
        let next = next_steps(&cfg);
        assert!(!next.local_only);
        assert_eq!(next.url_mapping_target, "leaf.example.com:8443");
    }

    fn config_with_url(public_url: &str) -> Tier1Config {
        Tier1Config {
            discord_token: "tok".to_owned(),
            client_id: "123".to_owned(),
            client_secret: "sec".to_owned(),
            public_url: public_url.to_owned(),
            r2: R2Config {
                endpoint: "https://acc.r2.cloudflarestorage.com".to_owned(),
                bucket: "leaf".to_owned(),
                access_key_id: "ak".to_owned(),
                secret_access_key: "sk".to_owned(),
            },
        }
    }

    #[test]
    fn invite_asks_for_the_documented_permissions() {
        // View Channels, Send Messages, Embed Links, Attach Files, Add
        // Reactions, Read Message History.
        assert_eq!(INVITE_PERMISSIONS, 1024 + 2048 + 16384 + 32768 + 64 + 65536);
        assert_eq!(INVITE_PERMISSIONS, 117_824);
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
        assert_eq!(json["errors"][0]["field"], "setup_code");
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
    async fn validator_failures_keep_their_fields_and_nothing_is_written() {
        let mock = Arc::new(Mock {
            discord_errors: vec![
                (Field::ClientSecret, "secret rejected"),
                (Field::DiscordToken, "bad token"),
            ],
            r2_error: Some((Field::R2Bucket, "no such bucket")),
            ..Mock::default()
        });
        let (_dir, mode, path) = fixture(mock);
        let (status, json) = post_submit(&mode.router, body_json(&mode.code)).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        // In page order, whatever order the validators reported them in.
        assert_eq!(
            fields(&json),
            ["discord_token", "client_secret", "r2_bucket"]
        );
        assert_eq!(json["errors"][2]["message"], "no such bucket");
        assert!(json.get("next").is_none());
        assert!(!path.exists());
        assert!(!*mode.done_rx.borrow());
    }

    #[tokio::test]
    async fn every_empty_field_is_reported_at_once_without_live_checks() {
        let mock = Arc::new(Mock::ok());
        let (_dir, mode, path) = fixture(Arc::clone(&mock));
        let mut body = body_value(&mode.code);
        for name in [
            "discord_token",
            "client_id",
            "client_secret",
            "public_url",
            "r2_endpoint",
            "r2_bucket",
            "r2_access_key_id",
            "r2_secret_access_key",
        ] {
            body[name] = "  ".into();
        }
        let (status, json) = post_submit(&mode.router, body.to_string()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            fields(&json),
            [
                "discord_token",
                "client_id",
                "client_secret",
                "public_url",
                "r2_endpoint",
                "r2_bucket",
                "r2_access_key_id",
                "r2_secret_access_key",
            ]
        );
        assert_eq!(json["errors"][0]["message"], "Enter the bot token.");
        assert_eq!(mock.discord_calls.load(Ordering::SeqCst), 0);
        assert_eq!(mock.r2_calls.load(Ordering::SeqCst), 0);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_complete_section_is_checked_while_the_other_has_gaps() {
        let mock = Arc::new(Mock {
            discord_errors: vec![(Field::DiscordToken, "bad token")],
            ..Mock::default()
        });
        let (_dir, mode, _path) = fixture(Arc::clone(&mock));
        let mut body = body_value(&mode.code);
        body["r2_bucket"] = "".into();
        let (status, json) = post_submit(&mode.router, body.to_string()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fields(&json), ["discord_token", "r2_bucket"]);
        assert_eq!(mock.discord_calls.load(Ordering::SeqCst), 1);
        assert_eq!(mock.r2_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn malformed_values_are_named_before_any_live_check() {
        let mock = Arc::new(Mock::ok());
        let (_dir, mode, _path) = fixture(Arc::clone(&mock));
        let mut body = body_value(&mode.code);
        body["client_id"] = "my-app".into();
        body["r2_endpoint"] = "http://acc.r2.cloudflarestorage.com".into();
        body["public_url"] = "https://leaf.example.com/setup".into();
        let (status, json) = post_submit(&mode.router, body.to_string()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fields(&json), ["client_id", "public_url", "r2_endpoint"]);
        assert_eq!(json["errors"][0]["message"], CLIENT_ID_NOT_A_NUMBER);
        assert_eq!(
            json["errors"][1]["message"],
            "Enter only the address, without a path: https://leaf.example.com"
        );
        assert_eq!(mock.discord_calls.load(Ordering::SeqCst), 0);
        assert_eq!(mock.r2_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_bad_public_url_alone_still_gets_the_credentials_checked() {
        let mock = Arc::new(Mock::ok());
        let (_dir, mode, path) = fixture(Arc::clone(&mock));
        let mut body = body_value(&mode.code);
        body["public_url"] = "http://leaf.example.com".into();
        let (status, json) = post_submit(&mode.router, body.to_string()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(fields(&json), ["public_url"]);
        assert_eq!(mock.discord_calls.load(Ordering::SeqCst), 1);
        assert_eq!(mock.r2_calls.load(Ordering::SeqCst), 1);
        assert!(!path.exists());
    }

    #[test]
    fn public_urls_are_reduced_to_an_origin_that_saves() {
        let ok = [
            ("https://leaf.example.com", "https://leaf.example.com"),
            (" https://leaf.example.com/ ", "https://leaf.example.com"),
            ("HTTPS://Leaf.Example.COM:443/", "https://leaf.example.com"),
            (
                "https://leaf.example.com:8443",
                "https://leaf.example.com:8443",
            ),
            ("http://localhost:3777", "http://localhost:3777"),
            ("http://127.0.0.1:3777/", "http://127.0.0.1:3777"),
        ];
        for (raw, origin) in ok {
            assert_eq!(public_origin(raw).as_deref(), Ok(origin), "{raw}");
            // Whatever passes here must also pass the config's own check.
            assert!(config_with_url(origin).validate().is_ok(), "{origin}");
        }

        let refused = [
            ("", "Enter the public URL"),
            ("leaf.example.com", "https://leaf.example.com."),
            (
                "leaf.example.com/setup",
                "for example https://leaf.example.com.",
            ),
            // A local leaf serves plain HTTP.
            ("localhost:3777", "for example http://localhost:3777."),
            ("127.0.0.1:3777/admin", "for example http://127.0.0.1:3777."),
            ("http://leaf.example.com", "HTTPS"),
            ("ftp://leaf.example.com", "isn't a web address"),
            ("https://", "isn't a web address"),
            ("https://leaf.example.com/setup", "without a path"),
            ("https://leaf.example.com/?x=1", "without a path"),
            ("https://user:pw@leaf.example.com", "without a path"),
        ];
        for (raw, says) in refused {
            let message = public_origin(raw).unwrap_err();
            assert!(message.contains(says), "{raw}: {message}");
        }
    }

    #[test]
    fn clip_cuts_on_characters() {
        assert_eq!(clip("leaf", 10), "leaf");
        assert_eq!(clip("🍃🍃🍃", 2), "🍃🍃…");
    }

    #[test]
    fn field_names_match_the_page() {
        let page = include_str!("setup_page.html");
        for field in [
            Field::DiscordToken,
            Field::ClientId,
            Field::ClientSecret,
            Field::PublicUrl,
            Field::R2Endpoint,
            Field::R2Bucket,
            Field::R2AccessKeyId,
            Field::R2SecretAccessKey,
            Field::Form,
        ] {
            let name = serde_json::to_value(field).unwrap();
            let name = name.as_str().unwrap();
            assert!(page.contains(&format!("data-for=\"{name}\"")), "{name}");
            if field != Field::Form {
                assert!(page.contains(&format!("name=\"{name}\"")), "{name}");
                // The page's own empty-field check says the same thing.
                assert!(page.contains(missing_message(field)), "{name}");
            }
        }
        assert!(page.contains(CLIENT_ID_NOT_A_NUMBER));
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
        assert_eq!(
            resp.headers().get(header::CACHE_CONTROL).unwrap(),
            "no-store"
        );
    }
}
