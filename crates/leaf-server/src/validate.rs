//! Live credential validation against Discord and R2, used by setup mode,
//! and the storage round trip `leaf doctor` runs against a configured install
//! ([`LiveValidator::storage_round_trip`]).
//!
//! Storage that is a folder on this machine (a `file://` endpoint, see
//! `leaf_core::media::local_store_dir`) gets the same canary, run in that
//! folder: nothing is asked of the network, and the bucket and keys are not
//! looked at.
//!
//! Every failure comes back on the form field it concerns, as one plain
//! sentence that says what to change, and never echoes a credential. The raw
//! cause goes to the log instead, with the R2 keys masked: an S3 error body is
//! XML that means nothing on a form.
//!
//! Every check is bounded. The two Discord calls run side by side under the
//! HTTP client's timeout; the R2 canary has one retry, an overall deadline of
//! 25 seconds, and 5 more to remove its test object when a step failed. The
//! setup page tells the operator to expect an answer within about half a
//! minute, and these limits (30 seconds at most) keep that true.

use std::ops::Range;
use std::time::Duration;

use leaf_core::config::R2Config;
use leaf_core::media::LocalStoreError;
use object_store::aws::{AmazonS3, AmazonS3Builder};
use object_store::path::Path as ObjectPath;
use object_store::{ClientOptions, ObjectStore, RetryConfig};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::setup::{CredentialValidator, FOLDER_NOT_ABSOLUTE, Field, FieldError, clip};

/// Base URL of Discord's REST API (also what `leaf doctor` asks).
pub const DISCORD_API: &str = "https://discord.com/api/v10";
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
/// Limit for removing the canary once a step has failed or run out of time:
/// by then the deadline above may have passed, and the object must still go.
/// With the deadline this is the half minute the setup page promises.
const R2_CLEANUP_TIMEOUT: Duration = Duration::from_secs(5);
/// Object the setup canary writes and removes again.
const CANARY_KEY: &str = "leaf-setup-canary";
/// Object `leaf doctor` writes and removes again. Archived media lives under
/// `g/…`, so this name can never be a day's file.
pub const DOCTOR_CANARY_KEY: &str = "leaf-doctor-canary";
/// What a canary stores.
const CANARY_BODY: &[u8] = b"leaf storage check";
/// The bytes of [`CANARY_BODY`] a ranged read asks for, and what they are.
const CANARY_PART_RANGE: Range<u64> = 5..12;
const CANARY_PART: &[u8] = b"storage";

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
const R2_WRONG_BYTES: &str = "This endpoint returned different bytes than leaf stored in the test \
    file. Check it's the S3 endpoint from R2 → Overview and that nothing between leaf and R2 \
    rewrites the files.";
const RANGE_UNSUPPORTED: &str = "This endpoint stored and returned the test file but would not \
    return a part of it. leaf reads files in parts to play video, so it needs storage that \
    answers byte-range requests, as R2 does.";

// ---- copy: a folder on this machine ----

const FOLDER_NOT_CREATED: &str = "leaf can't create this folder. Check the path is right, that \
    no file already has that name, and that the user leaf runs as may create folders there.";
const FOLDER_NOT_OPENED: &str = "leaf can't open this folder. Check the user leaf runs as may \
    read it.";
const FOLDER_NOT_WRITABLE: &str = "leaf can't save files in this folder. Give the user leaf runs \
    as permission to write there, and check the disk isn't full or read-only.";
const FOLDER_NOT_READABLE: &str = "leaf saved a test file in this folder but couldn't read it \
    back. Check the user leaf runs as may read files there.";
const FOLDER_WRONG_BYTES: &str = "leaf saved a test file in this folder and read back something \
    else. Check the disk, or choose another folder.";
const FOLDER_NOT_DELETABLE: &str = "leaf can save files in this folder but can't remove them. \
    Give the user leaf runs as full access to the folder: leaf deletes files when a post is \
    undone.";
const FOLDER_TIMED_OUT: &str = "This folder didn't answer within 25 seconds. If it is on a \
    network drive, check the drive is connected, then try again.";

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
    /// Limit for removing the canary's object once a step has failed.
    r2_cleanup: Duration,
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
            r2_cleanup: R2_CLEANUP_TIMEOUT,
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

    /// The storage check `leaf doctor` runs against a configured install:
    /// writes one small object under [`DOCTOR_CANARY_KEY`], reads it back,
    /// reads a byte range of it (what a video player asks for) and deletes
    /// it. Once the write has succeeded, or may have (it got no answer), the
    /// object is deleted whatever became of the other steps. Same time
    /// limits as the setup canary.
    pub async fn storage_round_trip(&self, r2: &R2Config) -> Result<(), StorageFailure> {
        let canary = Canary {
            key: DOCTOR_CANARY_KEY,
            ranged: true,
            deadline: self.r2_deadline,
            cleanup: self.r2_cleanup,
        };
        self.run_canary(r2, canary, "doctor").await
    }

    /// Runs `canary` against the bucket (or the folder) `r2` names and puts
    /// a failure into words. `who` (setup or doctor) labels the log line
    /// that carries the raw cause.
    async fn run_canary(
        &self,
        r2: &R2Config,
        canary: Canary,
        who: &'static str,
    ) -> Result<(), StorageFailure> {
        if leaf_core::media::is_local_endpoint(&r2.endpoint) {
            return folder_canary(&r2.endpoint, canary, who).await;
        }
        let unusable = |error: FieldError| StorageFailure {
            step: None,
            error,
            left_behind: false,
        };
        if let Err(error) = addressable(r2, canary.key, self.r2_allow_http) {
            tracing::warn!(
                field = ?error.field,
                "{who}: R2 settings cannot be made into a request"
            );
            return Err(unusable(error));
        }
        let store = self.r2_store(r2).map_err(|e| {
            tracing::warn!(
                error = %mask_keys(&e.to_string(), r2),
                "{who}: R2 settings refused before any request"
            );
            unusable(FieldError::new(Field::R2Endpoint, ENDPOINT_UNUSABLE))
        })?;
        run(&store, canary).await.map_err(|failure| {
            let error = match &failure.error {
                CanaryError::Store(e) => {
                    tracing::warn!(
                        step = failure.step.as_str(),
                        error = %mask_keys(&e.to_string(), r2),
                        "{who}: R2 check failed"
                    );
                    r2_failure(failure.step, e, &r2.bucket)
                }
                CanaryError::WrongBytes => {
                    tracing::warn!(
                        step = failure.step.as_str(),
                        "{who}: R2 returned other bytes than were stored"
                    );
                    FieldError::new(Field::R2Endpoint, R2_WRONG_BYTES)
                }
                CanaryError::TimedOut => {
                    tracing::warn!(
                        step = failure.step.as_str(),
                        deadline_secs = canary.deadline.as_secs(),
                        "{who}: R2 check timed out"
                    );
                    FieldError::new(Field::R2Endpoint, R2_TIMED_OUT)
                }
            };
            if failure.left_behind {
                tracing::warn!(
                    key = canary.key,
                    step = failure.step.as_str(),
                    "{who}: the test object could not be removed and may still be in the bucket"
                );
            }
            StorageFailure {
                step: Some(failure.step),
                error,
                left_behind: failure.left_behind,
            }
        })
    }
}

/// Why [`LiveValidator::storage_round_trip`] failed.
#[derive(Debug, PartialEq, Eq)]
pub struct StorageFailure {
    /// The step that failed; `None` when the settings were refused before
    /// any request was made (for a folder: before anything was written).
    pub step: Option<Step>,
    /// The setting at fault, and one plain sentence that says what to change.
    /// For a folder the setting is always [`Field::StorageFolder`].
    pub error: FieldError,
    /// The test object could not be removed afterwards and may still be in
    /// the bucket (or folder) under the canary's key. When `step` is anything but the
    /// write, it was written and is there. When `step` is the write, the
    /// write got no answer, so whether the object was stored is not known.
    pub left_behind: bool,
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
        let canary = Canary {
            key: CANARY_KEY,
            ranged: false,
            deadline: self.r2_deadline,
            cleanup: self.r2_cleanup,
        };
        self.run_canary(r2, canary, "setup")
            .await
            .map_err(|failure| failure.error)
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

// ---- a folder on this machine ----

/// The canary against a folder on this machine: the same write, read and
/// delete, in the store run mode builds for that folder
/// (`leaf_core::media::local_store`), which creates the folder when it is
/// not there. No request leaves the machine.
async fn folder_canary(
    endpoint: &str,
    canary: Canary,
    who: &'static str,
) -> Result<(), StorageFailure> {
    let refused = |message: &str| StorageFailure {
        step: None,
        error: FieldError::new(Field::StorageFolder, message),
        left_behind: false,
    };
    let Some(dir) = leaf_core::media::local_store_dir(endpoint) else {
        tracing::warn!("{who}: the storage folder is not given by its full path");
        return Err(refused(FOLDER_NOT_ABSOLUTE));
    };
    // Opening the folder may create it. That is file work, so it runs off
    // the runtime, and under the canary's deadline: a folder on a network
    // drive that has gone away can hang.
    let started = tokio::time::Instant::now();
    let opening = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || leaf_core::media::local_store(&dir)
    });
    let store = match tokio::time::timeout_at(started + canary.deadline, opening).await {
        Ok(Ok(Ok(store))) => store,
        Ok(Ok(Err(e))) => {
            tracing::warn!(error = %e, "{who}: the storage folder cannot be used");
            return Err(refused(match e {
                LocalStoreError::Create { .. } => FOLDER_NOT_CREATED,
                LocalStoreError::Open { .. } => FOLDER_NOT_OPENED,
                LocalStoreError::NotAbsolute => FOLDER_NOT_ABSOLUTE,
            }));
        }
        Ok(Err(e)) => {
            tracing::warn!(
                folder = %dir.display(),
                error = %e,
                "{who}: opening the storage folder did not finish"
            );
            return Err(refused(FOLDER_NOT_OPENED));
        }
        Err(_elapsed) => {
            tracing::warn!(
                folder = %dir.display(),
                deadline_secs = canary.deadline.as_secs(),
                "{who}: opening the storage folder timed out"
            );
            return Err(refused(FOLDER_TIMED_OUT));
        }
    };
    let canary = Canary {
        deadline: canary.deadline.saturating_sub(started.elapsed()),
        ..canary
    };
    run(&store, canary).await.map_err(|failure| {
        let cause = match &failure.error {
            CanaryError::Store(e) => e.to_string(),
            CanaryError::WrongBytes => "other bytes than were stored came back".to_owned(),
            CanaryError::TimedOut => "the deadline passed".to_owned(),
        };
        tracing::warn!(
            step = failure.step.as_str(),
            folder = %dir.display(),
            cause,
            "{who}: storage folder check failed"
        );
        if failure.left_behind {
            tracing::warn!(
                key = canary.key,
                folder = %dir.display(),
                "{who}: the test file could not be removed and may still be in the folder"
            );
        }
        StorageFailure {
            step: Some(failure.step),
            error: FieldError::new(
                Field::StorageFolder,
                folder_failure(failure.step, &failure.error),
            ),
            left_behind: failure.left_behind,
        }
    })
}

/// What to say about a canary step a folder failed.
const fn folder_failure(step: Step, error: &CanaryError) -> &'static str {
    match (error, step) {
        (CanaryError::TimedOut, _) => FOLDER_TIMED_OUT,
        (CanaryError::WrongBytes, _) => FOLDER_WRONG_BYTES,
        (CanaryError::Store(_), Step::Write) => FOLDER_NOT_WRITABLE,
        (CanaryError::Store(_), Step::Read | Step::ReadRange) => FOLDER_NOT_READABLE,
        (CanaryError::Store(_), Step::Delete) => FOLDER_NOT_DELETABLE,
    }
}

// ---- R2 ----

/// Checks that the S3 client can address `key` in this bucket at this
/// endpoint, and names the setting at fault when it cannot.
///
/// The client builds its request URL as `<endpoint>/<bucket>/<key>` and
/// panics on one that is not a URL (`object_store` 0.12: a space in the
/// bucket name is enough). Such settings are refused here, in words, with
/// the parser the client uses.
fn addressable(r2: &R2Config, key: &str, allow_http: bool) -> Result<(), FieldError> {
    let endpoint = r2.endpoint.trim_end_matches('/');
    let endpoint_ok = endpoint.parse::<axum::http::Uri>().is_ok()
        && reqwest::Url::parse(endpoint).is_ok_and(|url| {
            url.host_str().is_some()
                && (url.scheme() == "https" || (allow_http && url.scheme() == "http"))
        });
    if !endpoint_ok {
        return Err(FieldError::new(Field::R2Endpoint, ENDPOINT_UNUSABLE));
    }
    let request = format!("{endpoint}/{}/{key}", r2.bucket);
    if r2.bucket.contains('/') || request.parse::<axum::http::Uri>().is_err() {
        return Err(FieldError::new(Field::R2Bucket, BUCKET_NAME_INVALID));
    }
    Ok(())
}

/// The canary's steps, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Storing the test object.
    Write,
    /// Reading all of it back.
    Read,
    /// Reading a byte range of it.
    ReadRange,
    /// Removing it.
    Delete,
}

impl Step {
    /// The step as a word for a log line or a sentence.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Write => "write",
            Self::Read => "read",
            Self::ReadRange => "ranged read",
            Self::Delete => "delete",
        }
    }
}

/// One canary run: where it writes, how much it reads, how long it may take.
#[derive(Debug, Clone, Copy)]
struct Canary {
    /// Key of the object written and removed.
    key: &'static str,
    /// Also read a byte range of the object.
    ranged: bool,
    /// Limit for the write and the reads together; the delete that follows
    /// them unharmed shares it.
    deadline: Duration,
    /// Limit for removing the object once a step has failed.
    cleanup: Duration,
}

/// What stopped a canary step.
#[derive(Debug)]
enum CanaryError {
    /// The store refused the step, or could not be reached.
    Store(object_store::Error),
    /// The store answered, with other bytes than were written.
    WrongBytes,
    /// The deadline passed during the step.
    TimedOut,
}

/// A canary that did not finish.
#[derive(Debug)]
struct CanaryFailure {
    /// The first step that failed.
    step: Step,
    error: CanaryError,
    /// The object was written and is still in the bucket.
    left_behind: bool,
}

/// Runs one step under the deadline.
async fn timed<T>(
    deadline: tokio::time::Instant,
    step: Step,
    work: impl Future<Output = object_store::Result<T>>,
) -> Result<T, (Step, CanaryError)> {
    match tokio::time::timeout_at(deadline, work).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(e)) => Err((step, CanaryError::Store(e))),
        Err(_elapsed) => Err((step, CanaryError::TimedOut)),
    }
}

/// What the canary asks of a store: the four requests it makes. Every
/// [`ObjectStore`] answers them; the tests also hand it stores that lose an
/// answer or never give one, which no real store does on demand.
trait CanaryStore: Sync {
    /// Stores `body` at `path`.
    fn put(
        &self,
        path: &ObjectPath,
        body: bytes::Bytes,
    ) -> impl Future<Output = object_store::Result<()>> + Send;

    /// Reads all of the object at `path`.
    fn get(
        &self,
        path: &ObjectPath,
    ) -> impl Future<Output = object_store::Result<bytes::Bytes>> + Send;

    /// Reads the bytes in `range` of the object at `path`.
    fn get_range(
        &self,
        path: &ObjectPath,
        range: Range<u64>,
    ) -> impl Future<Output = object_store::Result<bytes::Bytes>> + Send;

    /// Removes the object at `path`.
    fn delete(&self, path: &ObjectPath) -> impl Future<Output = object_store::Result<()>> + Send;
}

impl<S: ObjectStore> CanaryStore for S {
    async fn put(&self, path: &ObjectPath, body: bytes::Bytes) -> object_store::Result<()> {
        ObjectStore::put(self, path, body.into()).await.map(drop)
    }

    async fn get(&self, path: &ObjectPath) -> object_store::Result<bytes::Bytes> {
        ObjectStore::get(self, path).await?.bytes().await
    }

    async fn get_range(
        &self,
        path: &ObjectPath,
        range: Range<u64>,
    ) -> object_store::Result<bytes::Bytes> {
        ObjectStore::get_range(self, path, range).await
    }

    async fn delete(&self, path: &ObjectPath) -> object_store::Result<()> {
        ObjectStore::delete(self, path).await
    }
}

/// Writes, reads back and deletes one small object: proves the endpoint,
/// bucket and keys, and that the token may delete (leaf removes media).
///
/// Once the write has succeeded the object is deleted whatever became of
/// the reads, so a failed check leaves nothing in the bucket. A failed read
/// is still what is reported; the delete then only decides `left_behind`.
/// The same goes for a write that failed without an answer (see
/// [`may_have_landed`]): the store may hold the object all the same.
async fn run<S: CanaryStore + ?Sized>(store: &S, canary: Canary) -> Result<(), CanaryFailure> {
    let path = ObjectPath::from(canary.key);
    let deadline = tokio::time::Instant::now() + canary.deadline;
    let body = bytes::Bytes::from_static(CANARY_BODY);
    if let Err((step, error)) = timed(deadline, Step::Write, store.put(&path, body)).await {
        let left_behind = may_have_landed(&error) && !removed(store, &path, canary.cleanup).await;
        return Err(CanaryFailure {
            step,
            error,
            left_behind,
        });
    }
    match read_back(store, &path, canary.ranged, deadline).await {
        Ok(()) => timed(deadline, Step::Delete, store.delete(&path))
            .await
            .map_err(|(step, error)| CanaryFailure {
                step,
                error,
                left_behind: true,
            }),
        Err((step, error)) => Err(CanaryFailure {
            step,
            error,
            left_behind: !removed(store, &path, canary.cleanup).await,
        }),
    }
}

/// Removes the canary's object after a step has failed, within a limit of
/// its own, and says whether it is gone. A store that answers that there is
/// no such object has none to remove.
async fn removed<S: CanaryStore + ?Sized>(store: &S, path: &ObjectPath, limit: Duration) -> bool {
    let until = tokio::time::Instant::now() + limit;
    match timed(until, Step::Delete, store.delete(path)).await {
        Ok(()) | Err((_, CanaryError::Store(object_store::Error::NotFound { .. }))) => true,
        Err(_) => false,
    }
}

/// Whether a write that failed may have been stored all the same.
///
/// It may when nothing came back that says otherwise: the deadline passed,
/// the connection was lost once it was made, or the answer was a failure on
/// the far side (a 5xx, as a proxy gives when it loses the store's answer)
/// and not a refusal. It cannot when the store refused the write (401, 403,
/// 404 and the other 4xx answers) or no connection was ever made.
fn may_have_landed(error: &CanaryError) -> bool {
    match error {
        CanaryError::TimedOut => true,
        CanaryError::Store(e @ object_store::Error::Generic { .. }) => match transport_failure(e) {
            Some(_) => !http_causes(e).any(reqwest::Error::is_connect),
            None => answered_status(&e.to_string()).is_some_and(|status| status >= 500),
        },
        // The store's other errors are its refusals, and bytes that differ
        // come from a read.
        CanaryError::Store(_) | CanaryError::WrongBytes => false,
    }
}

/// Reads the canary back, whole and (when `ranged`) in part, and compares
/// what comes back with what was written.
async fn read_back<S: CanaryStore + ?Sized>(
    store: &S,
    path: &ObjectPath,
    ranged: bool,
    deadline: tokio::time::Instant,
) -> Result<(), (Step, CanaryError)> {
    let whole = timed(deadline, Step::Read, store.get(path)).await?;
    if whole.as_ref() != CANARY_BODY {
        return Err((Step::Read, CanaryError::WrongBytes));
    }
    if ranged {
        let part = timed(
            deadline,
            Step::ReadRange,
            store.get_range(path, CANARY_PART_RANGE),
        )
        .await?;
        if part.as_ref() != CANARY_PART {
            return Err((Step::ReadRange, CanaryError::WrongBytes));
        }
    }
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
            // The whole file came back a moment ago: what this endpoint
            // refuses is the part.
            None if step == Step::ReadRange => {
                FieldError::new(Field::R2Endpoint, RANGE_UNSUPPORTED)
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
        Step::Read | Step::ReadRange => FieldError::new(
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

/// The HTTP client's own errors in the cause chain of `e`.
fn http_causes(e: &object_store::Error) -> impl Iterator<Item = &reqwest::Error> {
    let first: &(dyn std::error::Error + 'static) = e;
    std::iter::successors(Some(first), |err| err.source())
        .filter_map(|err| err.downcast_ref::<reqwest::Error>())
}

/// How the request failed, when the HTTP client says no answer arrived.
fn transport_failure(e: &object_store::Error) -> Option<Transport> {
    http_causes(e).find_map(|http| {
        if http.is_timeout() {
            Some(Transport::TimedOut)
        } else if http.is_connect() || http.is_request() {
            Some(Transport::Unreachable)
        } else {
            None
        }
    })
}

/// The HTTP status the S3 client quotes for an answer it took as a failure
/// ("… non-2xx status code: 502 Bad Gateway: …").
fn answered_status(text: &str) -> Option<u16> {
    const MARK: &str = "status code: ";
    let rest = text.get(text.find(MARK)? + MARK.len()..)?;
    rest.get(..3)?.parse().ok()
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

    /// How the S3 stand-in answers a read (a `GET`, whole or ranged).
    #[derive(Clone, Copy)]
    enum Read {
        /// As S3 does: the stored object, or the part of it asked for.
        Stored,
        /// With this status and S3 XML error body.
        Fails(u16, &'static str),
        /// With these bytes instead of the stored ones.
        Bytes(&'static [u8]),
        /// With the whole object and a 200, whatever range was asked for.
        IgnoresRange,
    }

    /// What the S3 stand-in does with each kind of request. `put` and
    /// `delete` are (status, body); bodies are S3 XML error documents.
    #[derive(Clone, Copy)]
    struct S3 {
        put: (u16, &'static str),
        /// The object is stored even when `put` answers a failure, as when
        /// something between leaf and the store loses the store's answer.
        put_lands: bool,
        get: Read,
        range: Read,
        delete: (u16, &'static str),
    }

    impl S3 {
        /// A store where everything works.
        const WORKING: Self = Self {
            put: (200, ""),
            put_lands: false,
            get: Read::Stored,
            range: Read::Stored,
            delete: (204, ""),
        };
    }

    /// What reached the stand-in, and the object it holds.
    #[derive(Default)]
    struct S3State {
        puts: AtomicU32,
        gets: AtomicU32,
        ranges: AtomicU32,
        deletes: AtomicU32,
        /// Key and bytes of the stored object. A successful PUT stores it,
        /// a successful DELETE removes it.
        object: std::sync::Mutex<Option<(String, bytes::Bytes)>>,
    }

    impl S3State {
        fn stored(&self) -> Option<(String, bytes::Bytes)> {
            self.object.lock().unwrap().clone()
        }

        /// (PUTs, whole GETs, ranged GETs, DELETEs) received.
        fn calls(&self) -> (u32, u32, u32, u32) {
            (
                self.puts.load(Ordering::SeqCst),
                self.gets.load(Ordering::SeqCst),
                self.ranges.load(Ordering::SeqCst),
                self.deletes.load(Ordering::SeqCst),
            )
        }
    }

    /// `bytes=a-b` as the half-open span it selects.
    fn requested_range(headers: &HeaderMap) -> Option<Range<usize>> {
        let spec = headers.get(header::RANGE)?.to_str().ok()?;
        let (first, last) = spec.strip_prefix("bytes=")?.split_once('-')?;
        Some(first.parse().ok()?..last.parse::<usize>().ok()? + 1)
    }

    fn object_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ETAG, "\"e1\"".parse().unwrap());
        headers.insert(
            header::LAST_MODIFIED,
            "Wed, 21 Oct 2015 07:28:00 GMT".parse().unwrap(),
        );
        headers
    }

    /// S3 stand-in holding at most one object.
    async fn s3(behaviour: S3, state: Arc<S3State>) -> Stub {
        let router = Router::new().route(
            "/{*key}",
            any(
                move |method: Method,
                      uri: axum::http::Uri,
                      headers: HeaderMap,
                      body: bytes::Bytes| async move {
                    let refuse = |(status, body): (u16, &'static str)| -> Response {
                        (StatusCode::from_u16(status).unwrap(), body).into_response()
                    };
                    let key = uri.path().to_owned();
                    match method {
                        Method::PUT => {
                            state.puts.fetch_add(1, Ordering::SeqCst);
                            let refused = behaviour.put.0 >= 300;
                            if !refused || behaviour.put_lands {
                                *state.object.lock().unwrap() = Some((key, body));
                            }
                            if refused {
                                return refuse(behaviour.put);
                            }
                            (StatusCode::OK, object_headers()).into_response()
                        }
                        Method::DELETE => {
                            state.deletes.fetch_add(1, Ordering::SeqCst);
                            if behaviour.delete.0 >= 300 {
                                return refuse(behaviour.delete);
                            }
                            *state.object.lock().unwrap() = None;
                            StatusCode::NO_CONTENT.into_response()
                        }
                        _ => {
                            let range = requested_range(&headers);
                            let read = if range.is_some() {
                                state.ranges.fetch_add(1, Ordering::SeqCst);
                                behaviour.range
                            } else {
                                state.gets.fetch_add(1, Ordering::SeqCst);
                                behaviour.get
                            };
                            let stored = state.stored().filter(|(at, _)| *at == key);
                            let Some((_, stored)) = stored else {
                                return refuse((404, leak(s3_error("NoSuchKey"))));
                            };
                            let object = match read {
                                Read::Fails(status, body) => return refuse((status, body)),
                                Read::IgnoresRange => {
                                    return (StatusCode::OK, object_headers(), stored)
                                        .into_response();
                                }
                                Read::Bytes(other) => bytes::Bytes::from_static(other),
                                Read::Stored => stored,
                            };
                            let Some(range) = range else {
                                return (StatusCode::OK, object_headers(), object).into_response();
                            };
                            let mut headers = object_headers();
                            headers.insert(
                                header::CONTENT_RANGE,
                                format!("bytes {}-{}/{}", range.start, range.end - 1, object.len())
                                    .parse()
                                    .unwrap(),
                            );
                            (StatusCode::PARTIAL_CONTENT, headers, object.slice(range))
                                .into_response()
                        }
                    }
                },
            ),
        );
        serve(router).await
    }

    /// S3 stand-in that answers PUT with `put` and DELETE with `delete`,
    /// and reads as S3 does.
    async fn r2(put: (u16, &'static str), delete: (u16, &'static str)) -> Stub {
        let behaviour = S3 {
            put,
            delete,
            ..S3::WORKING
        };
        s3(behaviour, Arc::default()).await
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
        // A write that got no answer is followed by an attempt to remove
        // the object; here nothing would answer that either.
        let v = LiveValidator {
            r2_deadline: Duration::from_millis(200),
            r2_cleanup: Duration::ZERO,
            ..validator("http://unused")
        };
        let err = v
            .validate_r2(&r2_config(&format!("http://{addr}")))
            .await
            .unwrap_err();
        hold.abort();
        assert_eq!(err, FieldError::new(Field::R2Endpoint, R2_TIMED_OUT));
    }

    // ---- the round trip `leaf doctor` runs ----

    /// Runs the doctor's round trip against a stand-in that behaves as
    /// told; returns the outcome and what the stand-in saw.
    async fn round_trip(behaviour: S3) -> (Result<(), StorageFailure>, Arc<S3State>) {
        let state = Arc::new(S3State::default());
        let stub = s3(behaviour, Arc::clone(&state)).await;
        let v = validator("http://unused");
        let outcome = v.storage_round_trip(&r2_config(&stub.base)).await;
        (outcome, state)
    }

    #[test]
    fn the_ranged_read_asks_for_a_real_part_of_the_canary() {
        let range = usize::try_from(CANARY_PART_RANGE.start).unwrap()
            ..usize::try_from(CANARY_PART_RANGE.end).unwrap();
        assert_eq!(&CANARY_BODY[range], CANARY_PART);
        // A part, not the whole thing: a store that ignores the range must
        // not pass by accident.
        assert!(CANARY_PART.len() < CANARY_BODY.len());
    }

    #[tokio::test]
    async fn round_trip_writes_reads_reads_a_part_and_deletes() {
        let (outcome, state) = round_trip(S3::WORKING).await;
        assert_eq!(outcome, Ok(()));
        assert_eq!(state.calls(), (1, 1, 1, 1));
        assert_eq!(state.stored(), None, "the canary is gone afterwards");
    }

    #[tokio::test]
    async fn round_trip_uses_its_own_throwaway_key() {
        // Held at the delete, so the object can be looked at.
        let behaviour = S3 {
            delete: (403, leak(s3_error("AccessDenied"))),
            ..S3::WORKING
        };
        let (outcome, state) = round_trip(behaviour).await;
        let (key, bytes) = state.stored().unwrap();
        assert_eq!(key, format!("/leaf/{DOCTOR_CANARY_KEY}"));
        assert_eq!(bytes.as_ref(), CANARY_BODY);
        // Nothing leaf archives lives outside `g/`.
        assert!(!DOCTOR_CANARY_KEY.starts_with("g/"));

        // The delete is the failure, and the object is reported as left.
        let failure = outcome.unwrap_err();
        assert_eq!(failure.step, Some(Step::Delete));
        assert!(failure.left_behind);
        assert_eq!(failure.error.field, Field::R2AccessKeyId);
        assert!(failure.error.message.contains("not delete"), "{failure:?}");
    }

    #[tokio::test]
    async fn the_canary_is_removed_when_a_read_fails() {
        // (what the stand-in does, step blamed, field blamed)
        let cases = [
            (
                S3 {
                    get: Read::Fails(403, leak(s3_error("AccessDenied"))),
                    ..S3::WORKING
                },
                Step::Read,
                Field::R2AccessKeyId,
            ),
            (
                S3 {
                    get: Read::Bytes(b"something else"),
                    ..S3::WORKING
                },
                Step::Read,
                Field::R2Endpoint,
            ),
            (
                S3 {
                    range: Read::Fails(403, leak(s3_error("AccessDenied"))),
                    ..S3::WORKING
                },
                Step::ReadRange,
                Field::R2AccessKeyId,
            ),
            (
                S3 {
                    range: Read::IgnoresRange,
                    ..S3::WORKING
                },
                Step::ReadRange,
                Field::R2Endpoint,
            ),
            (
                S3 {
                    range: Read::Bytes(b"leaf STORAGE check"),
                    ..S3::WORKING
                },
                Step::ReadRange,
                Field::R2Endpoint,
            ),
        ];
        for (behaviour, step, field) in cases {
            let (outcome, state) = round_trip(behaviour).await;
            let failure = outcome.unwrap_err();
            assert_eq!(failure.step, Some(step), "{failure:?}");
            assert_eq!(failure.error.field, field, "{failure:?}");
            assert!(!failure.left_behind, "{failure:?}");
            let (_, _, _, deletes) = state.calls();
            assert_eq!(deletes, 1, "{failure:?}");
            assert_eq!(state.stored(), None, "{failure:?}");
            // One plain sentence: no XML, no request details, no keys.
            assert!(!failure.error.message.contains("</"), "{failure:?}");
            assert!(!failure.error.message.contains("127.0.0.1"), "{failure:?}");
            assert!(!failure.error.message.contains("AKID"), "{failure:?}");
            assert!(!failure.error.message.contains("SECRET"), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_store_without_byte_ranges_is_named() {
        let behaviour = S3 {
            range: Read::IgnoresRange,
            ..S3::WORKING
        };
        let (outcome, _state) = round_trip(behaviour).await;
        assert_eq!(outcome.unwrap_err().error.message, RANGE_UNSUPPORTED);

        let behaviour = S3 {
            get: Read::Bytes(b"something else"),
            ..S3::WORKING
        };
        let (outcome, _state) = round_trip(behaviour).await;
        assert_eq!(outcome.unwrap_err().error.message, R2_WRONG_BYTES);
    }

    #[tokio::test]
    async fn a_failed_read_is_reported_even_when_the_cleanup_fails_too() {
        let behaviour = S3 {
            get: Read::Fails(403, leak(s3_error("AccessDenied"))),
            delete: (403, leak(s3_error("AccessDenied"))),
            ..S3::WORKING
        };
        let (outcome, state) = round_trip(behaviour).await;
        let failure = outcome.unwrap_err();
        assert_eq!(failure.step, Some(Step::Read));
        assert!(failure.left_behind);
        assert!(state.stored().is_some());
    }

    #[tokio::test]
    async fn a_refused_write_leaves_nothing_to_remove() {
        // S3's refusals, typed by the client (403, 404) or not (400): the
        // store has said the object is not there, so nothing more is asked.
        for put in [
            (403, s3_error("AccessDenied")),
            (404, s3_error("NoSuchBucket")),
            (400, s3_error("InvalidArgument")),
        ] {
            let behaviour = S3 {
                put: (put.0, leak(put.1)),
                ..S3::WORKING
            };
            let (outcome, state) = round_trip(behaviour).await;
            let failure = outcome.unwrap_err();
            assert_eq!(failure.step, Some(Step::Write), "{failure:?}");
            assert!(!failure.left_behind, "{failure:?}");
            assert_eq!(state.calls(), (1, 0, 0, 0), "{failure:?}");
        }
    }

    #[tokio::test]
    async fn a_write_whose_answer_was_lost_is_removed_all_the_same() {
        // The store took the object, and what came back was a 502 from
        // something in between: no refusal, so the object may be there.
        let behaviour = S3 {
            put: (502, "<html>Bad gateway</html>"),
            put_lands: true,
            ..S3::WORKING
        };
        let (outcome, state) = round_trip(behaviour).await;
        let failure = outcome.unwrap_err();
        assert_eq!(failure.step, Some(Step::Write));
        let (_, gets, ranges, deletes) = state.calls();
        assert_eq!((gets, ranges, deletes), (0, 0, 1));
        assert_eq!(state.stored(), None);
        assert!(!failure.left_behind);

        // When it cannot be removed either, that is reported.
        let behaviour = S3 {
            delete: (403, leak(s3_error("AccessDenied"))),
            ..behaviour
        };
        let (outcome, state) = round_trip(behaviour).await;
        let failure = outcome.unwrap_err();
        assert_eq!(failure.step, Some(Step::Write));
        assert!(failure.left_behind);
        assert!(state.stored().is_some());

        // A store that answers that there is no such object has none left.
        let behaviour = S3 {
            put_lands: false,
            delete: (404, leak(s3_error("NoSuchKey"))),
            ..behaviour
        };
        let (outcome, state) = round_trip(behaviour).await;
        let failure = outcome.unwrap_err();
        assert_eq!(failure.step, Some(Step::Write));
        assert_eq!(state.calls().3, 1);
        assert!(!failure.left_behind);
    }

    #[tokio::test]
    async fn a_write_cut_off_on_the_way_is_followed_by_a_removal() {
        // Takes each request and hangs up without a word: the write may
        // have arrived. The method of every request is kept.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let record = Arc::clone(&seen);
        let hang_up = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let mut head = Vec::new();
                let mut buffer = [0_u8; 1024];
                while !head.windows(4).any(|end| end == b"\r\n\r\n") {
                    if socket.readable().await.is_err() {
                        break;
                    }
                    match socket.try_read(&mut buffer) {
                        Ok(0) => break,
                        Ok(read) => head.extend_from_slice(&buffer[..read]),
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                        Err(_) => break,
                    }
                }
                let head = String::from_utf8_lossy(&head);
                let method = head.split_whitespace().next().unwrap_or_default();
                record.lock().unwrap().push(method.to_owned());
            }
        });
        let v = validator("http://unused");
        let failure = v
            .storage_round_trip(&r2_config(&format!("http://{addr}")))
            .await
            .unwrap_err();
        hang_up.abort();
        assert_eq!(failure.step, Some(Step::Write));
        assert_eq!(
            failure.error,
            FieldError::new(Field::R2Endpoint, ENDPOINT_UNREACHABLE)
        );
        let seen = seen.lock().unwrap().clone();
        assert!(seen.iter().any(|method| method == "DELETE"), "{seen:?}");
        // The removal got no answer either, so the object may be there.
        assert!(failure.left_behind);

        // An endpoint no connection could be made to has taken nothing.
        let failure = v
            .storage_round_trip(&r2_config(&closed_port().await))
            .await
            .unwrap_err();
        assert_eq!(failure.step, Some(Step::Write));
        assert!(!failure.left_behind);
    }

    // ---- steps that never answer ----

    /// A store for the canary whose steps can be made to never answer,
    /// which the HTTP stand-in could only do by letting real time pass.
    #[derive(Default)]
    struct Fickle {
        /// What it holds.
        inner: object_store::memory::InMemory,
        /// A write is stored, and its answer never comes.
        put_goes_unanswered: bool,
        get_hangs: bool,
        delete_hangs: bool,
        deletes: AtomicU32,
    }

    impl Fickle {
        async fn holds_nothing(&self) -> bool {
            let listing = self.inner.list_with_delimiter(None).await.unwrap();
            listing.objects.is_empty()
        }
    }

    impl CanaryStore for Fickle {
        async fn put(&self, path: &ObjectPath, body: bytes::Bytes) -> object_store::Result<()> {
            CanaryStore::put(&self.inner, path, body).await?;
            if self.put_goes_unanswered {
                return std::future::pending().await;
            }
            Ok(())
        }

        async fn get(&self, path: &ObjectPath) -> object_store::Result<bytes::Bytes> {
            if self.get_hangs {
                return std::future::pending().await;
            }
            CanaryStore::get(&self.inner, path).await
        }

        async fn get_range(
            &self,
            path: &ObjectPath,
            range: Range<u64>,
        ) -> object_store::Result<bytes::Bytes> {
            CanaryStore::get_range(&self.inner, path, range).await
        }

        async fn delete(&self, path: &ObjectPath) -> object_store::Result<()> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            if self.delete_hangs {
                return std::future::pending().await;
            }
            CanaryStore::delete(&self.inner, path).await
        }
    }

    /// A canary with no time at all: a step passes only if its answer is
    /// there the moment it is asked for. `Fickle` answers at once or never,
    /// so no clock decides a test that uses this.
    const NO_TIME: Canary = Canary {
        key: DOCTOR_CANARY_KEY,
        ranged: true,
        deadline: Duration::ZERO,
        cleanup: Duration::ZERO,
    };

    #[tokio::test]
    async fn fickle_is_an_ordinary_store_until_told_otherwise() {
        let store = Fickle::default();
        run(&store, NO_TIME).await.unwrap();
        assert_eq!(store.deletes.load(Ordering::SeqCst), 1);
        assert!(store.holds_nothing().await);
    }

    #[tokio::test]
    async fn the_canary_is_removed_when_a_read_runs_out_of_time() {
        let store = Fickle {
            get_hangs: true,
            ..Fickle::default()
        };
        let failure = run(&store, NO_TIME).await.unwrap_err();
        assert_eq!(failure.step, Step::Read);
        assert!(
            matches!(failure.error, CanaryError::TimedOut),
            "{failure:?}"
        );
        // The deadline had passed; the delete still went out and worked.
        assert_eq!(store.deletes.load(Ordering::SeqCst), 1);
        assert!(!failure.left_behind);
        assert!(store.holds_nothing().await);
    }

    #[tokio::test]
    async fn the_canary_is_removed_when_its_write_runs_out_of_time() {
        // Stored, and never answered: the deadline is what ends the write.
        let store = Fickle {
            put_goes_unanswered: true,
            ..Fickle::default()
        };
        let failure = run(&store, NO_TIME).await.unwrap_err();
        assert_eq!(failure.step, Step::Write);
        assert!(
            matches!(failure.error, CanaryError::TimedOut),
            "{failure:?}"
        );
        assert_eq!(store.deletes.load(Ordering::SeqCst), 1);
        assert!(!failure.left_behind);
        assert!(store.holds_nothing().await);
    }

    #[tokio::test]
    async fn an_object_that_cannot_be_removed_in_time_is_reported() {
        for (store, step) in [
            (
                Fickle {
                    get_hangs: true,
                    delete_hangs: true,
                    ..Fickle::default()
                },
                Step::Read,
            ),
            (
                Fickle {
                    put_goes_unanswered: true,
                    delete_hangs: true,
                    ..Fickle::default()
                },
                Step::Write,
            ),
        ] {
            let failure = run(&store, NO_TIME).await.unwrap_err();
            assert_eq!(failure.step, step);
            assert_eq!(store.deletes.load(Ordering::SeqCst), 1, "{step:?}");
            assert!(failure.left_behind, "{step:?}");
            assert!(!store.holds_nothing().await, "{step:?}");
        }
    }

    #[test]
    fn the_status_of_a_failed_answer_is_read_from_the_clients_message() {
        let said = |answer: &str| {
            answered_status(&format!(
                "Generic S3 error: Error performing PUT http://e/b/k in 12ms, after 1 retries, \
                 max_retries: 1, retry_timeout: 10s  - Server returned non-2xx status code: \
                 {answer}"
            ))
        };
        assert_eq!(said("502 Bad Gateway: <html>"), Some(502));
        assert_eq!(said("400 Bad Request: "), Some(400));
        assert_eq!(said("50"), None);
        assert_eq!(said("teapot"), None);
        assert_eq!(answered_status("error sending request"), None);
        assert_eq!(answered_status(""), None);
    }

    #[tokio::test]
    async fn an_unusable_endpoint_fails_before_any_step() {
        let v = validator("http://unused");
        let failure = v
            .storage_round_trip(&r2_config("not a url"))
            .await
            .unwrap_err();
        assert_eq!(failure.step, None);
        assert_eq!(
            failure.error,
            FieldError::new(Field::R2Endpoint, ENDPOINT_UNUSABLE)
        );
    }

    #[tokio::test]
    async fn settings_the_s3_client_cannot_address_are_refused_in_words() {
        // The S3 client panics on a request URL that is not one, so each of
        // these must be answered before it is asked: no request arrives.
        let state = Arc::new(S3State::default());
        let stub = s3(S3::WORKING, Arc::clone(&state)).await;
        let v = validator("http://unused");
        for bucket in ["my bucket", "leaf\tphotos", "a/b", "caf\u{e9} <1>"] {
            let cfg = R2Config {
                bucket: bucket.to_owned(),
                ..r2_config(&stub.base)
            };
            let expected = FieldError::new(Field::R2Bucket, BUCKET_NAME_INVALID);
            assert_eq!(v.validate_r2(&cfg).await, Err(expected), "{bucket:?}");
            let failure = v.storage_round_trip(&cfg).await.unwrap_err();
            assert_eq!(failure.step, None, "{bucket:?}");
            assert_eq!(failure.error.field, Field::R2Bucket, "{bucket:?}");
        }
        for endpoint in [
            "not a url",
            "acc.r2.cloudflarestorage.com",
            "ftp://acc.example",
        ] {
            let cfg = r2_config(endpoint);
            let expected = FieldError::new(Field::R2Endpoint, ENDPOINT_UNUSABLE);
            assert_eq!(v.validate_r2(&cfg).await, Err(expected), "{endpoint:?}");
        }
        assert_eq!(state.calls(), (0, 0, 0, 0));

        // Plain HTTP is for the stand-ins only: the real validator, like
        // run mode's store, refuses it.
        let live = LiveValidator::new().unwrap();
        assert_eq!(
            live.validate_r2(&r2_config(&stub.base)).await,
            Err(FieldError::new(Field::R2Endpoint, ENDPOINT_UNUSABLE))
        );
        assert_eq!(state.calls(), (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn the_setup_canary_reads_no_range_and_cleans_up_too() {
        let state = Arc::new(S3State::default());
        let stub = s3(S3::WORKING, Arc::clone(&state)).await;
        let v = validator("http://unused");
        assert_eq!(v.validate_r2(&r2_config(&stub.base)).await, Ok(()));
        assert_eq!(state.calls(), (1, 1, 0, 1));

        // A read the token may not make: the object still goes.
        let state = Arc::new(S3State::default());
        let behaviour = S3 {
            get: Read::Fails(403, leak(s3_error("AccessDenied"))),
            ..S3::WORKING
        };
        let stub = s3(behaviour, Arc::clone(&state)).await;
        let err = v.validate_r2(&r2_config(&stub.base)).await.unwrap_err();
        assert_eq!(err.field, Field::R2AccessKeyId);
        assert_eq!(state.stored(), None);
    }

    #[tokio::test]
    async fn the_canary_runs_over_any_object_store() {
        let store = object_store::memory::InMemory::new();
        let canary = Canary {
            key: DOCTOR_CANARY_KEY,
            ranged: true,
            deadline: R2_DEADLINE,
            cleanup: R2_CLEANUP_TIMEOUT,
        };
        run(&store, canary).await.unwrap();
        // Nothing is left under any key.
        let listing = store.list_with_delimiter(None).await.unwrap();
        assert!(listing.objects.is_empty(), "{listing:?}");
        assert!(listing.common_prefixes.is_empty(), "{listing:?}");
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

    // ---- a folder on this machine ----

    /// Storage settings for the folder at `dir`, as setup saves them: the
    /// endpoint alone.
    fn folder_config(dir: &std::path::Path) -> R2Config {
        R2Config {
            endpoint: leaf_core::media::local_endpoint(dir.to_str().unwrap()).unwrap(),
            bucket: String::new(),
            access_key_id: String::new(),
            secret_access_key: String::new(),
        }
    }

    /// The names in `dir`, sorted.
    fn names_in(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// The validator as setup and the doctor build it. Nothing in it allows
    /// plain HTTP or points at a stand-in: a folder needs no network.
    fn live() -> LiveValidator {
        LiveValidator::new().unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_folder_canary_creates_the_folder_and_leaves_nothing_in_it() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("leaf data").join("media");
        let cfg = folder_config(&dir);

        assert_eq!(live().validate_r2(&cfg).await, Ok(()));
        assert!(dir.is_dir(), "the folder is created");
        assert!(names_in(&dir).is_empty());

        // The doctor's round trip, with its ranged read, in the folder that
        // is there now.
        assert_eq!(live().storage_round_trip(&cfg).await, Ok(()));
        assert!(names_in(&dir).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn the_folder_canary_writes_and_removes_its_own_file_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("media");
        std::fs::create_dir(&dir).unwrap();
        // What an interrupted run would leave, under each canary's key, and
        // a day's file that is none of the canary's business.
        std::fs::write(dir.join(CANARY_KEY), b"stale").unwrap();
        std::fs::write(dir.join(DOCTOR_CANARY_KEY), b"stale").unwrap();
        std::fs::create_dir(dir.join("g")).unwrap();
        std::fs::write(dir.join("g").join("kept"), b"a day").unwrap();

        let cfg = folder_config(&dir);
        assert_eq!(live().validate_r2(&cfg).await, Ok(()));
        assert_eq!(names_in(&dir), ["g", DOCTOR_CANARY_KEY]);
        assert_eq!(live().storage_round_trip(&cfg).await, Ok(()));
        assert_eq!(names_in(&dir), ["g"]);
        assert_eq!(std::fs::read(dir.join("g").join("kept")).unwrap(), b"a day");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_folder_ignores_whatever_the_bucket_and_keys_hold() {
        let tmp = tempfile::tempdir().unwrap();
        // A bucket the S3 client cannot even address, and placeholder keys:
        // a config from before setup offered a folder looks like this.
        let cfg = R2Config {
            bucket: "not a bucket".to_owned(),
            access_key_id: "local".to_owned(),
            secret_access_key: "local".to_owned(),
            ..folder_config(&tmp.path().join("media"))
        };
        assert_eq!(live().validate_r2(&cfg).await, Ok(()));
        assert_eq!(live().storage_round_trip(&cfg).await, Ok(()));
    }

    #[tokio::test]
    async fn a_folder_not_given_by_its_full_path_is_refused_in_words() {
        for endpoint in ["file://media", "file://nas/media", "file://", "file:///"] {
            let cfg = r2_config(endpoint);
            let expected = FieldError::new(Field::StorageFolder, FOLDER_NOT_ABSOLUTE);
            assert_eq!(live().validate_r2(&cfg).await, Err(expected), "{endpoint}");
            let failure = live().storage_round_trip(&cfg).await.unwrap_err();
            assert_eq!(failure.step, None, "{endpoint}");
            assert_eq!(failure.error.field, Field::StorageFolder, "{endpoint}");
            assert!(!failure.left_behind, "{endpoint}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_folder_that_cannot_be_created_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        // A file where a folder on the way should be.
        std::fs::write(tmp.path().join("taken"), b"").unwrap();
        let cfg = folder_config(&tmp.path().join("taken").join("media"));

        let expected = FieldError::new(Field::StorageFolder, FOLDER_NOT_CREATED);
        assert_eq!(live().validate_r2(&cfg).await, Err(expected));
        let failure = live().storage_round_trip(&cfg).await.unwrap_err();
        assert_eq!(failure.step, None);
        assert_eq!(failure.error.message, FOLDER_NOT_CREATED);
        assert_eq!(names_in(tmp.path()), ["taken"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_folder_that_takes_no_write_says_so_at_the_write_step() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("media");
        // A folder where each test file would go: the write cannot land,
        // whoever leaf runs as.
        std::fs::create_dir_all(dir.join(CANARY_KEY)).unwrap();
        std::fs::create_dir_all(dir.join(DOCTOR_CANARY_KEY)).unwrap();
        let cfg = folder_config(&dir);

        let expected = FieldError::new(Field::StorageFolder, FOLDER_NOT_WRITABLE);
        assert_eq!(live().validate_r2(&cfg).await, Err(expected));
        let failure = live().storage_round_trip(&cfg).await.unwrap_err();
        assert_eq!(failure.step, Some(Step::Write));
        assert_eq!(failure.error.message, FOLDER_NOT_WRITABLE);
        assert!(!failure.left_behind);
        // No half-written file stays behind either.
        assert_eq!(names_in(&dir), [DOCTOR_CANARY_KEY, CANARY_KEY]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_only_folder_says_leaf_cannot_save_files_there() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("media");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root writes where the mode says no; there is nothing to see then.
        if std::fs::write(dir.join("probe"), b"").is_ok() {
            return;
        }

        let outcome = live().validate_r2(&folder_config(&dir)).await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            outcome,
            Err(FieldError::new(Field::StorageFolder, FOLDER_NOT_WRITABLE))
        );
        assert!(names_in(&dir).is_empty());
    }

    #[test]
    fn each_folder_failure_says_what_the_folder_would_not_do() {
        let refused = || {
            CanaryError::Store(object_store::Error::Generic {
                store: "LocalFileSystem",
                source: "Permission denied (os error 13)".into(),
            })
        };
        let cases = [
            (Step::Write, refused(), FOLDER_NOT_WRITABLE),
            (Step::Read, refused(), FOLDER_NOT_READABLE),
            (Step::ReadRange, refused(), FOLDER_NOT_READABLE),
            (Step::Delete, refused(), FOLDER_NOT_DELETABLE),
            (Step::Read, CanaryError::WrongBytes, FOLDER_WRONG_BYTES),
            (Step::ReadRange, CanaryError::WrongBytes, FOLDER_WRONG_BYTES),
            (Step::Write, CanaryError::TimedOut, FOLDER_TIMED_OUT),
            (Step::Read, CanaryError::TimedOut, FOLDER_TIMED_OUT),
            (Step::Delete, CanaryError::TimedOut, FOLDER_TIMED_OUT),
        ];
        for (step, error, says) in cases {
            assert_eq!(folder_failure(step, &error), says, "{step:?} {error:?}");
        }
        // The deadline the sentence names is the one the canary has.
        assert!(FOLDER_TIMED_OUT.contains(&format!("{} seconds", R2_DEADLINE.as_secs())));
        // A folder's sentences are about a folder: no bucket, no keys.
        for copy in [
            FOLDER_NOT_ABSOLUTE,
            FOLDER_NOT_CREATED,
            FOLDER_NOT_OPENED,
            FOLDER_NOT_WRITABLE,
            FOLDER_NOT_READABLE,
            FOLDER_WRONG_BYTES,
            FOLDER_NOT_DELETABLE,
            FOLDER_TIMED_OUT,
        ] {
            for word in ["R2", "bucket", "key", "endpoint", "token"] {
                assert!(!copy.contains(word), "{word} in {copy}");
            }
        }
    }
}
