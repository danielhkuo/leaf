//! Live credential validation against Discord and R2, used by setup mode.
//!
//! Every failure comes back on the form field it concerns, as one plain
//! sentence that says what to change, and never echoes a credential. The raw
//! cause goes to the log instead, with the R2 keys masked: an S3 error body is
//! XML that means nothing on a form.
//!
//! Every check is bounded. The two Discord calls run side by side under the
//! HTTP client's timeout; the R2 canary has one retry and an overall
//! deadline. The setup page tells the operator to expect an answer within
//! about half a minute, and these limits keep that true.

use std::time::Duration;

use leaf_core::config::R2Config;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ClientOptions, ObjectStore as _, RetryConfig};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::setup::{CredentialValidator, Field, FieldError, clip};

const DISCORD_API: &str = "https://discord.com/api/v10";
/// Limit for each Discord call (the page's copy says 10 seconds).
const DISCORD_TIMEOUT: Duration = Duration::from_secs(10);
/// Limit for each R2 request, and for opening its connection.
const R2_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const R2_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// One retry for a dropped connection or a 5xx. `object_store`'s default (ten
/// retries within three minutes) keeps the page waiting about a minute on an
/// endpoint that resolves but never answers.
const R2_MAX_RETRIES: usize = 1;
const R2_RETRY_WINDOW: Duration = Duration::from_secs(10);
/// Deadline for the whole canary: write, read back, delete. `R2_TIMED_OUT`
/// names it.
const R2_DEADLINE: Duration = Duration::from_secs(25);
/// Object the canary writes and removes again.
const CANARY_KEY: &str = "leaf-setup-canary";

// ---- copy: Discord ----

const TOKEN_REJECTED: &str = "Discord didn't accept this bot token. Copy it again from Bot → \
    Reset Token in the Developer Portal (the public key and client secret won't work here).";
const TOKEN_MALFORMED: &str = "This isn't a bot token: it contains spaces or characters a token \
    never has. Copy it again from Bot → Reset Token.";
const SECRET_REJECTED: &str = "Discord didn't accept this client secret. In the Developer Portal, \
    open OAuth2, select Reset Secret and paste the new secret here.";
const PAIR_REJECTED: &str = "Discord didn't accept this application ID and client secret \
    together. Copy both from OAuth2 in the same application; if the ID is right, reset the \
    secret.";
const DISCORD_UNREACHABLE: &str = "leaf couldn't reach Discord to check these details. Check \
    this server can reach discord.com, then try again.";
const DISCORD_TIMED_OUT: &str = "Discord didn't answer within 10 seconds. Try again in a minute.";

// ---- copy: R2 ----

const BUCKET_NAME_INVALID: &str = "R2 doesn't accept this bucket name. Bucket names use only \
    lowercase letters, numbers and hyphens.";
const ACCESS_KEY_UNKNOWN: &str = "R2 doesn't recognise this access key ID. Copy it again from \
    your R2 API token, or create a new token.";
const ACCESS_KEY_WRONG_LENGTH: &str = "This isn't an R2 access key ID (those are 32 characters). \
    Copy the Access Key ID from your R2 API token, not the token value or the secret access key.";
const SECRET_KEY_REJECTED: &str = "R2 didn't accept this secret access key for this access key \
    ID. Copy it again (R2 shows it only once), or create a new API token.";
const KEYS_REJECTED: &str = "R2 didn't accept these keys. Check the access key ID and secret \
    access key come from the same R2 API token.";
const ENDPOINT_UNREACHABLE: &str = "leaf couldn't connect to this endpoint. Use the S3 endpoint \
    from R2 → Overview, such as https://<account-id>.r2.cloudflarestorage.com.";
const ENDPOINT_TIMED_OUT: &str = "This endpoint didn't answer in time. Check it's the S3 \
    endpoint from R2 → Overview, then try again.";
const ENDPOINT_NOT_S3: &str = "This address answered, but not as R2's storage API. Use the S3 \
    endpoint from R2 → Overview, such as https://<account-id>.r2.cloudflarestorage.com.";
const ENDPOINT_UNUSABLE: &str = "leaf can't use this endpoint. Enter it like \
    https://<account-id>.r2.cloudflarestorage.com.";
const R2_TIMED_OUT: &str = "R2 didn't finish the check within 25 seconds. Check the endpoint, \
    then try again in a minute.";
const R2_UNEXPECTED: &str = "R2 refused the test upload for a reason leaf doesn't recognise. \
    Check the endpoint, bucket and keys; leaf's logs have the details (docker compose logs \
    leaf).";

/// Validator that talks to the real services.
#[derive(Debug, Clone)]
pub struct LiveValidator {
    http: reqwest::Client,
    /// Discord REST base URL; a local stand-in in tests.
    discord_api: String,
    /// Accept a plain-HTTP R2 endpoint. Run mode refuses one, so only tests
    /// (talking to a local stand-in) turn this on.
    r2_allow_http: bool,
    /// Deadline for the R2 canary.
    r2_deadline: Duration,
}

impl LiveValidator {
    /// Builds the validator (constructs its HTTP client).
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(DISCORD_TIMEOUT)
                .build()?,
            discord_api: DISCORD_API.to_owned(),
            r2_allow_http: false,
            r2_deadline: R2_DEADLINE,
        })
    }

    /// The application the bot token belongs to. `Ok(None)` when Discord
    /// accepted the token but its answer could not be read: the client
    /// id/secret check then decides on its own.
    async fn token_application(&self, token: &str) -> Result<Option<Application>, FieldError> {
        let resp = self
            .http
            .get(format!("{}/applications/@me", self.discord_api))
            .header(reqwest::header::AUTHORIZATION, format!("Bot {token}"))
            .send()
            .await
            .map_err(|e| {
                // A builder error here can only be the header: the token
                // holds a character no header (and no token) can carry.
                if e.is_builder() {
                    FieldError::new(Field::DiscordToken, TOKEN_MALFORMED)
                } else {
                    discord_send_failure(&e)
                }
            })?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(FieldError::new(Field::DiscordToken, TOKEN_REJECTED));
        }
        if !status.is_success() {
            return Err(discord_unavailable(status));
        }
        match resp.json::<Application>().await {
            Ok(app) => Ok(Some(app)),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "setup: unreadable answer from GET /applications/@me; \
                     skipping the application ID comparison"
                );
                Ok(None)
            }
        }
    }

    /// The client-credentials grant succeeds only for a valid id/secret pair.
    async fn check_pair(&self, client_id: &str, client_secret: &str) -> Result<(), PairFailure> {
        let resp = self
            .http
            .post(format!("{}/oauth2/token", self.discord_api))
            .basic_auth(client_id, Some(client_secret))
            .form(&[("grant_type", "client_credentials"), ("scope", "identify")])
            .send()
            .await
            .map_err(|e| PairFailure::Other(discord_send_failure(&e)))?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else if status == StatusCode::BAD_REQUEST || status == StatusCode::UNAUTHORIZED {
            Err(PairFailure::Rejected)
        } else {
            Err(PairFailure::Other(discord_unavailable(status)))
        }
    }

    /// The store the canary runs against: run mode's settings (see
    /// `leaf_core::media::r2_store`) with tighter time limits.
    fn r2_store(&self, r2: &R2Config) -> Result<AmazonS3, object_store::Error> {
        AmazonS3Builder::new()
            .with_endpoint(&r2.endpoint)
            .with_bucket_name(&r2.bucket)
            .with_access_key_id(&r2.access_key_id)
            .with_secret_access_key(&r2.secret_access_key)
            .with_region("auto")
            .with_client_options(
                ClientOptions::new()
                    .with_timeout(R2_REQUEST_TIMEOUT)
                    .with_connect_timeout(R2_CONNECT_TIMEOUT)
                    .with_allow_http(self.r2_allow_http),
            )
            .with_retry(RetryConfig {
                max_retries: R2_MAX_RETRIES,
                retry_timeout: R2_RETRY_WINDOW,
                ..RetryConfig::default()
            })
            .build()
    }
}

impl CredentialValidator for LiveValidator {
    async fn validate_discord(
        &self,
        token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Vec<FieldError> {
        // Both checks always run, so a bad token does not hide a bad secret.
        let (token_app, pair) = tokio::join!(
            self.token_application(token),
            self.check_pair(client_id, client_secret)
        );
        discord_findings(token_app, pair, client_id)
    }

    async fn validate_r2(&self, r2: &R2Config) -> Result<(), FieldError> {
        let store = self.r2_store(r2).map_err(|e| {
            tracing::warn!(
                error = %mask_keys(&e.to_string(), r2),
                "setup: R2 settings refused before any request"
            );
            FieldError::new(Field::R2Endpoint, ENDPOINT_UNUSABLE)
        })?;
        match tokio::time::timeout(self.r2_deadline, canary(&store)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err((step, e))) => {
                tracing::warn!(
                    step = step.as_str(),
                    error = %mask_keys(&e.to_string(), r2),
                    "setup: R2 check failed"
                );
                Err(r2_failure(step, &e, &r2.bucket))
            }
            Err(_elapsed) => {
                tracing::warn!(
                    deadline_secs = self.r2_deadline.as_secs(),
                    "setup: R2 check timed out"
                );
                Err(FieldError::new(Field::R2Endpoint, R2_TIMED_OUT))
            }
        }
    }
}

// ---- Discord ----

/// The part of `GET /applications/@me` the comparison needs. This is the
/// application id, which for some older applications differs from the bot
/// user's id that `GET /users/@me` returns.
#[derive(Debug, Deserialize)]
struct Application {
    id: String,
    #[serde(default)]
    name: String,
}

/// Why the client id/secret pair could not be confirmed.
#[derive(Debug)]
enum PairFailure {
    /// Discord refused the pair.
    Rejected,
    /// Discord was unreachable or failed on its side.
    Other(FieldError),
}

/// Turns the two Discord checks into form errors.
///
/// A token from another application goes on whichever value is the odd one
/// out. When Discord accepted the ID and secret together, that is the token.
/// Otherwise it is reported on the application ID, and the pair failure that
/// usually comes with it is left out: it follows from the same mix-up, and a
/// second error would send the operator off to reset a client secret that
/// may be fine. Likewise the secret alone is blamed only once the token has
/// confirmed the ID.
fn discord_findings(
    token_app: Result<Option<Application>, FieldError>,
    pair: Result<(), PairFailure>,
    client_id: &str,
) -> Vec<FieldError> {
    let mut errors = Vec::new();
    let same_app = match token_app {
        Ok(Some(app)) => {
            let same = same_application(&app.id, client_id);
            if same == Some(false) {
                errors.push(if pair.is_ok() {
                    token_mismatch(&app, client_id)
                } else {
                    id_mismatch(&app)
                });
            }
            same
        }
        Ok(None) => None,
        Err(e) => {
            errors.push(e);
            None
        }
    };
    let pair_error = match pair {
        Ok(()) => None,
        Err(PairFailure::Rejected) => match same_app {
            Some(false) => None,
            Some(true) => Some(FieldError::new(Field::ClientSecret, SECRET_REJECTED)),
            None => Some(FieldError::new(Field::ClientSecret, PAIR_REJECTED)),
        },
        Err(PairFailure::Other(e)) => Some(e),
    };
    // Both calls failing the same way (Discord unreachable) is one problem.
    if let Some(e) = pair_error
        && !errors.contains(&e)
    {
        errors.push(e);
    }
    errors
}

/// Whether the token's application is the one `client_id` names; `None`
/// when either id is not a number, so nothing can be concluded.
fn same_application(app_id: &str, client_id: &str) -> Option<bool> {
    let app = app_id.trim().parse::<u64>().ok()?;
    let client = client_id.trim().parse::<u64>().ok()?;
    Some(app == client)
}

/// The token's application, for a message: its name when it has one.
fn other_application(app: &Application) -> String {
    let name = app.name.trim();
    let id = clip(app.id.trim(), 24);
    if name.is_empty() {
        format!("another application (ID {id})")
    } else {
        format!("another application, “{}” (ID {id})", clip(name, 60))
    }
}

/// The token and the ID disagree, and the ID and secret did not pair up
/// either: the ID is the likeliest odd one out.
fn id_mismatch(app: &Application) -> FieldError {
    FieldError::new(
        Field::ClientId,
        format!(
            "This bot token belongs to {}. The bot token, application ID and client secret \
             must all come from the same application.",
            other_application(app)
        ),
    )
}

/// The ID and secret belong together, so the token is the odd one out.
fn token_mismatch(app: &Application, client_id: &str) -> FieldError {
    let client_id = clip(client_id.trim(), 24);
    FieldError::new(
        Field::DiscordToken,
        format!(
            "This bot token belongs to {}, but the application ID and client secret are for \
             application {client_id}. Copy the bot token from application {client_id} \
             (Bot → Reset Token).",
            other_application(app)
        ),
    )
}

/// A Discord call that got no HTTP answer.
fn discord_send_failure(e: &reqwest::Error) -> FieldError {
    tracing::warn!(error = %e, "setup: Discord request failed");
    let message = if e.is_timeout() {
        DISCORD_TIMED_OUT
    } else {
        DISCORD_UNREACHABLE
    };
    FieldError::new(Field::Form, message)
}

/// An answer that says nothing about the credentials (429, 5xx, ...).
fn discord_unavailable(status: StatusCode) -> FieldError {
    tracing::warn!(%status, "setup: Discord could not check the credentials");
    FieldError::new(
        Field::Form,
        format!(
            "Discord couldn't check these details just now (HTTP {}). Try again in a minute.",
            status.as_u16()
        ),
    )
}

// ---- R2 ----

/// The canary's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Write,
    Read,
    Delete,
}

impl Step {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Read => "read",
            Self::Delete => "delete",
        }
    }
}

/// Writes, reads back and deletes one small object: proves the endpoint,
/// bucket and keys, and that the token may delete (leaf removes media).
async fn canary(store: &AmazonS3) -> Result<(), (Step, object_store::Error)> {
    let path = ObjectPath::from(CANARY_KEY);
    store
        .put(&path, bytes::Bytes::from_static(b"leaf").into())
        .await
        .map_err(|e| (Step::Write, e))?;
    store.get(&path).await.map_err(|e| (Step::Read, e))?;
    store.delete(&path).await.map_err(|e| (Step::Delete, e))?;
    Ok(())
}

/// Maps a failed canary step to the field at fault.
///
/// `object_store` sorts HTTP answers into variants (404 `NotFound`, 403
/// `PermissionDenied`, 401 `Unauthenticated`) and keeps S3's error body in
/// the message; the body's `<Code>`, when there is one, is the most specific
/// signal. Anything without an HTTP answer is the endpoint's.
fn r2_failure(step: Step, e: &object_store::Error, bucket: &str) -> FieldError {
    let text = e.to_string();
    let code = s3_error_code(&text);
    let bucket = clip(bucket, 63);
    match code {
        Some("NoSuchBucket") => return bucket_missing(&bucket),
        Some("InvalidBucketName") => return FieldError::new(Field::R2Bucket, BUCKET_NAME_INVALID),
        Some("InvalidAccessKeyId") => {
            return FieldError::new(Field::R2AccessKeyId, ACCESS_KEY_UNKNOWN);
        }
        // R2's answer to an access key ID of the wrong length: "Credential
        // access key has length 40, should be 32".
        Some("InvalidArgument") if text.contains("Credential access key") => {
            return FieldError::new(Field::R2AccessKeyId, ACCESS_KEY_WRONG_LENGTH);
        }
        Some("SignatureDoesNotMatch") => {
            return FieldError::new(Field::R2SecretAccessKey, SECRET_KEY_REJECTED);
        }
        _ => {}
    }
    match e {
        // A 404 with no S3 error code did not come from an S3 API at all.
        object_store::Error::NotFound { .. } if code.is_none() => {
            FieldError::new(Field::R2Endpoint, ENDPOINT_NOT_S3)
        }
        object_store::Error::NotFound { .. } if step == Step::Write => bucket_missing(&bucket),
        object_store::Error::Unauthenticated { .. } => {
            FieldError::new(Field::R2AccessKeyId, KEYS_REJECTED)
        }
        object_store::Error::PermissionDenied { .. } => permission_denied(step, &bucket),
        _ => match transport_failure(e) {
            Some(Transport::TimedOut) => FieldError::new(Field::R2Endpoint, ENDPOINT_TIMED_OUT),
            Some(Transport::Unreachable) => {
                FieldError::new(Field::R2Endpoint, ENDPOINT_UNREACHABLE)
            }
            None => FieldError::new(Field::Form, R2_UNEXPECTED),
        },
    }
}

fn bucket_missing(bucket: &str) -> FieldError {
    FieldError::new(
        Field::R2Bucket,
        format!(
            "R2 has no bucket named “{bucket}” at this endpoint. Check the spelling (bucket \
             names are lowercase), or create the bucket in the R2 dashboard."
        ),
    )
}

/// A 403 without a more specific code, on the step that got it.
fn permission_denied(step: Step, bucket: &str) -> FieldError {
    match step {
        // The first request. A token scoped to particular buckets gets a 403
        // for a misspelt bucket, not `NoSuchBucket`, so the name (quicker to
        // check) comes first and the token's permission second.
        Step::Write => FieldError::new(
            Field::R2Bucket,
            format!(
                "R2 won't let this API token add files to “{bucket}”. Check the bucket name \
                 matches the bucket in R2 exactly; if it does, give the token Object Read & \
                 Write permission for this bucket."
            ),
        ),
        // The write has proved the bucket name: the token is at fault.
        Step::Read => FieldError::new(
            Field::R2AccessKeyId,
            format!(
                "This R2 API token isn't allowed to read files in “{bucket}”. Give it Object \
                 Read & Write permission for this bucket, or create a token that has it."
            ),
        ),
        Step::Delete => FieldError::new(
            Field::R2AccessKeyId,
            format!(
                "This R2 API token can add files to “{bucket}” but not delete them. leaf needs \
                 Object Read & Write, which includes deleting."
            ),
        ),
    }
}

/// How a request failed when no HTTP answer arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    TimedOut,
    Unreachable,
}

/// Finds the HTTP client's own error in the cause chain, if any.
fn transport_failure(e: &object_store::Error) -> Option<Transport> {
    let mut cause: Option<&(dyn std::error::Error + 'static)> = Some(e);
    while let Some(err) = cause {
        if let Some(http) = err.downcast_ref::<reqwest::Error>() {
            if http.is_timeout() {
                return Some(Transport::TimedOut);
            }
            if http.is_connect() || http.is_request() {
                return Some(Transport::Unreachable);
            }
        }
        cause = err.source();
    }
    None
}

/// The `<Code>` of an S3 XML error body quoted in `text`.
fn s3_error_code(text: &str) -> Option<&str> {
    const OPEN: &str = "<Code>";
    let start = text.find(OPEN)? + OPEN.len();
    let rest = text.get(start..)?;
    let code = rest.get(..rest.find("</Code>")?)?.trim();
    (!code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric())).then_some(code)
}

/// Masks the R2 keys wherever an error message repeats them (some S3 error
/// bodies quote the access key id).
fn mask_keys(text: &str, r2: &R2Config) -> String {
    let mut masked = text.to_owned();
    for (secret, mask) in [
        (r2.secret_access_key.as_str(), "<secret access key>"),
        (r2.access_key_id.as_str(), "<access key id>"),
    ] {
        // Very short values would mask unrelated text and are not worth hiding.
        if secret.len() >= 4 {
            masked = masked.replace(secret, mask);
        }
    }
    masked
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic"
    )]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    use axum::Router;
    use axum::http::{HeaderMap, Method, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{any, get, post};

    use super::*;

    /// A local HTTP server for the duration of a test.
    struct Stub {
        base: String,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    async fn serve(app: Router) -> Stub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Stub {
            base: format!("http://{addr}"),
            task,
        }
    }

    /// An address nothing listens on (bound, then released).
    async fn closed_port() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }

    fn validator(discord_api: &str) -> LiveValidator {
        LiveValidator {
            discord_api: discord_api.to_owned(),
            r2_allow_http: true,
            ..LiveValidator::new().unwrap()
        }
    }

    fn r2_config(endpoint: &str) -> R2Config {
        R2Config {
            endpoint: endpoint.to_owned(),
            bucket: "leaf".to_owned(),
            access_key_id: "AKID-1234".to_owned(),
            secret_access_key: "SECRET-5678".to_owned(),
        }
    }

    // ---- Discord ----

    #[derive(Default)]
    struct DiscordCalls {
        app: AtomicU32,
        token: AtomicU32,
    }

    /// Discord stand-in: `GET /applications/@me` answers `app` (status, body)
    /// and `POST /oauth2/token` answers `pair_status`.
    async fn discord(app: (u16, &'static str), pair_status: u16, calls: Arc<DiscordCalls>) -> Stub {
        let app_calls = Arc::clone(&calls);
        let router = Router::new()
            .route(
                "/api/v10/applications/@me",
                get(move || async move {
                    app_calls.app.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::from_u16(app.0).unwrap(), app.1)
                }),
            )
            .route(
                "/api/v10/oauth2/token",
                post(move || async move {
                    calls.token.fetch_add(1, Ordering::SeqCst);
                    (
                        StatusCode::from_u16(pair_status).unwrap(),
                        r#"{"access_token":"x"}"#,
                    )
                }),
            );
        serve(router).await
    }

    const APP_123: &str = r#"{"id":"123","name":"leaf","flags":0}"#;

    async fn check_discord(
        app: (u16, &'static str),
        pair: u16,
        client_id: &str,
    ) -> Vec<FieldError> {
        let calls = Arc::new(DiscordCalls::default());
        let stub = discord(app, pair, Arc::clone(&calls)).await;
        let v = validator(&format!("{}/api/v10", stub.base));
        let errors = v.validate_discord("tok.en.here", client_id, "secret").await;
        // Both checks always run.
        assert_eq!(calls.app.load(Ordering::SeqCst), 1);
        assert_eq!(calls.token.load(Ordering::SeqCst), 1);
        errors
    }

    #[tokio::test]
    async fn discord_accepts_a_matching_token_and_pair() {
        assert!(check_discord((200, APP_123), 200, "123").await.is_empty());
    }

    #[tokio::test]
    async fn token_from_another_application_lands_on_the_odd_one_out() {
        // The ID and secret don't pair up either: the ID is blamed, and the
        // pair failure follows from the same mix-up so it is left out.
        let errors = check_discord((200, APP_123), 401, "456").await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, Field::ClientId);
        assert!(errors[0].message.contains("“leaf” (ID 123)"));

        // The ID and secret belong together: the token is the odd one out,
        // and the message names the application to copy it from.
        let errors = check_discord((200, APP_123), 200, "456").await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, Field::DiscordToken);
        assert!(errors[0].message.contains("“leaf” (ID 123)"));
        assert!(errors[0].message.contains("from application 456"));
    }

    #[tokio::test]
    async fn unnamed_application_is_named_by_its_id() {
        let errors = check_discord((200, r#"{"id":"123"}"#), 401, "456").await;
        assert_eq!(errors.len(), 1);
        assert!(
            errors[0].message.contains("another application (ID 123)."),
            "{errors:?}"
        );
    }

    #[tokio::test]
    async fn bad_token_does_not_hide_a_bad_secret() {
        let errors = check_discord((401, "{}"), 401, "123").await;
        let fields: Vec<_> = errors.iter().map(|e| e.field).collect();
        assert_eq!(fields, [Field::DiscordToken, Field::ClientSecret]);
        // The ID could not be confirmed, so both halves of the pair are named.
        assert_eq!(errors[1].message, PAIR_REJECTED);
    }

    #[tokio::test]
    async fn confirmed_id_puts_the_blame_on_the_secret() {
        let errors = check_discord((200, APP_123), 401, "123").await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, Field::ClientSecret);
        assert_eq!(errors[0].message, SECRET_REJECTED);
    }

    #[tokio::test]
    async fn unreadable_application_answer_skips_the_comparison() {
        assert!(
            check_discord((200, "not json"), 200, "456")
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn discord_outage_is_one_form_level_error() {
        let errors = check_discord((503, "{}"), 503, "123").await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, Field::Form);
        assert!(errors[0].message.contains("HTTP 503"));
    }

    #[tokio::test]
    async fn unreachable_discord_is_one_form_level_error() {
        let v = validator(&format!("{}/api/v10", closed_port().await));
        let errors = v.validate_discord("tok", "123", "secret").await;
        assert_eq!(errors, [FieldError::new(Field::Form, DISCORD_UNREACHABLE)]);
    }

    #[tokio::test]
    async fn token_with_impossible_characters_is_named_without_a_request() {
        let v = validator(&format!("{}/api/v10", closed_port().await));
        let errors = v.validate_discord("tok\nen", "123", "secret").await;
        assert_eq!(
            errors[0],
            FieldError::new(Field::DiscordToken, TOKEN_MALFORMED)
        );
    }

    #[test]
    fn application_ids_compare_as_numbers_only_when_both_parse() {
        assert_eq!(same_application("123", "123"), Some(true));
        assert_eq!(same_application("123", " 0123 "), Some(true));
        assert_eq!(same_application("123", "456"), Some(false));
        assert_eq!(same_application("123", "abc"), None);
        assert_eq!(same_application("", "123"), None);
    }

    // ---- R2 ----

    /// S3 stand-in: answers PUT with `put`, GET with the object, and DELETE
    /// with `delete` (status, body). Bodies are S3 XML error documents.
    async fn r2(put: (u16, &'static str), delete: (u16, &'static str)) -> Stub {
        let router = Router::new().route(
            "/{*key}",
            any(move |method: Method| async move {
                let answer = |(status, body): (u16, &'static str)| -> Response {
                    let mut headers = HeaderMap::new();
                    headers.insert(header::ETAG, "\"e1\"".parse().unwrap());
                    (StatusCode::from_u16(status).unwrap(), headers, body).into_response()
                };
                match method {
                    Method::PUT => answer(put),
                    Method::DELETE => answer(delete),
                    _ => {
                        let mut headers = HeaderMap::new();
                        headers.insert(header::ETAG, "\"e1\"".parse().unwrap());
                        headers.insert(
                            header::LAST_MODIFIED,
                            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
                        );
                        (StatusCode::OK, headers, "leaf").into_response()
                    }
                }
            }),
        );
        serve(router).await
    }

    fn s3_error(code: &str) -> String {
        s3_error_saying(code, "details")
    }

    fn s3_error_saying(code: &str, message: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><Error><Code>{code}</Code>\
             <Message>{message}</Message></Error>"
        )
    }

    fn leak(s: String) -> &'static str {
        Box::leak(s.into_boxed_str())
    }

    async fn check_r2(put: (u16, &'static str), delete: (u16, &'static str)) -> Option<FieldError> {
        let stub = r2(put, delete).await;
        let v = validator("http://unused");
        v.validate_r2(&r2_config(&stub.base)).await.err()
    }

    #[tokio::test]
    async fn r2_canary_passes_against_a_working_store() {
        assert_eq!(check_r2((200, ""), (204, "")).await, None);
    }

    #[tokio::test]
    async fn r2_failures_land_on_the_field_at_fault() {
        // (status, body, field the error must land on), for a failed write.
        let cases = [
            (404, s3_error("NoSuchBucket"), Field::R2Bucket),
            (400, s3_error("InvalidBucketName"), Field::R2Bucket),
            (403, s3_error("InvalidAccessKeyId"), Field::R2AccessKeyId),
            (
                403,
                s3_error("SignatureDoesNotMatch"),
                Field::R2SecretAccessKey,
            ),
            // The token value (40 characters) pasted as the access key ID.
            (
                400,
                s3_error_saying(
                    "InvalidArgument",
                    "Credential access key has length 40, should be 32",
                ),
                Field::R2AccessKeyId,
            ),
            (400, s3_error("InvalidArgument"), Field::Form),
            // A bucket-scoped token answers a misspelt bucket with 403.
            (403, s3_error("AccessDenied"), Field::R2Bucket),
            (401, s3_error("Unauthorized"), Field::R2AccessKeyId),
            (404, "<html>Not here</html>".to_owned(), Field::R2Endpoint),
        ];
        for (status, body, field) in cases {
            let err = check_r2((status, leak(body)), (204, "")).await.unwrap();
            assert_eq!(err.field, field, "{err:?}");
            // One plain sentence: no XML, no request details.
            assert!(!err.message.contains("</"), "{err:?}");
            assert!(!err.message.contains("127.0.0.1"));
        }
    }

    #[tokio::test]
    async fn refused_write_names_the_bucket_then_the_permission() {
        let err = check_r2((403, leak(s3_error("AccessDenied"))), (204, ""))
            .await
            .unwrap();
        assert_eq!(err.field, Field::R2Bucket);
        assert!(err.message.contains("bucket name matches"), "{err:?}");
        assert!(err.message.contains("Object Read & Write"), "{err:?}");
    }

    #[test]
    fn refused_read_or_delete_is_the_tokens_fault() {
        for step in [Step::Read, Step::Delete] {
            let err = permission_denied(step, "leaf");
            assert_eq!(err.field, Field::R2AccessKeyId, "{step:?}");
        }
    }

    #[tokio::test]
    async fn token_without_delete_permission_is_named() {
        let err = check_r2((200, ""), (403, leak(s3_error("AccessDenied"))))
            .await
            .unwrap();
        assert_eq!(err.field, Field::R2AccessKeyId);
        assert!(err.message.contains("not delete"));
    }

    #[tokio::test]
    async fn unexpected_r2_answer_is_form_level() {
        let err = check_r2((500, ""), (204, "")).await.unwrap();
        assert_eq!(err.field, Field::Form);
    }

    #[tokio::test]
    async fn unreachable_endpoint_is_the_endpoints_fault() {
        let v = validator("http://unused");
        let err = v
            .validate_r2(&r2_config(&closed_port().await))
            .await
            .unwrap_err();
        assert_eq!(
            err,
            FieldError::new(Field::R2Endpoint, ENDPOINT_UNREACHABLE)
        );
    }

    #[tokio::test]
    async fn silent_endpoint_hits_the_deadline() {
        // Accepts connections and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let _open = socket;
                    std::future::pending::<()>().await;
                });
            }
        });
        let v = LiveValidator {
            r2_deadline: Duration::from_millis(200),
            ..validator("http://unused")
        };
        let err = v
            .validate_r2(&r2_config(&format!("http://{addr}")))
            .await
            .unwrap_err();
        hold.abort();
        assert_eq!(err, FieldError::new(Field::R2Endpoint, R2_TIMED_OUT));
    }

    #[test]
    fn s3_error_codes_are_read_from_the_message() {
        assert_eq!(
            s3_error_code(&format!("PUT failed: {}", s3_error("NoSuchBucket"))),
            Some("NoSuchBucket")
        );
        assert_eq!(s3_error_code("<Code> </Code>"), None);
        assert_eq!(s3_error_code("<Code>a b</Code>"), None);
        assert_eq!(s3_error_code("<Code>Unclosed"), None);
        assert_eq!(s3_error_code("no xml here"), None);
    }

    #[test]
    fn logged_errors_mask_the_keys() {
        let r2 = r2_config("https://acc.r2.cloudflarestorage.com");
        let masked = mask_keys("key AKID-1234 sig SECRET-5678 end", &r2);
        assert_eq!(masked, "key <access key id> sig <secret access key> end");
    }
}
