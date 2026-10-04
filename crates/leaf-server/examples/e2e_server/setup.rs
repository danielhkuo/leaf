//! Setup mode for the browser suites: leaf's real setup page and its real
//! submit route (`leaf_server::setup::setup_mode`), over a data directory of
//! its own in a temp dir, with nothing asked of Discord or R2.
//!
//! What is replaced is the validator alone, and only where the real one
//! would use the network:
//!
//! - Discord's checks are answered from a script (`ok`, or the bot token
//!   refused);
//! - an S3 endpoint's check is answered from a script (`ok`, or the bucket
//!   refused);
//! - a folder on this machine is checked by the real
//!   [`LiveValidator`]: its canary writes, reads and deletes on this disk
//!   and makes no request.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use axum::Router;
use leaf_core::config::{CONFIG_FILE_NAME, R2Config, Tier1Config};
use leaf_server::setup::{CredentialValidator, Field, FieldError, setup_mode};
use leaf_server::validate::LiveValidator;
use serde::Deserialize;
use serde_json::{Value, json};

/// What the stand-in for Discord says about the bot token.
const TOKEN_REFUSED: &str = "The stand-in for Discord refuses this bot token.";
/// What the stand-in for R2 says about the bucket.
const BUCKET_REFUSED: &str = "The stand-in for R2 has no such bucket.";

/// How the stand-in for Discord answers a submit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscordAnswer {
    /// Token, application id and secret are all accepted.
    #[default]
    Ok,
    /// One error, on the bot token.
    RefuseToken,
}

/// How the stand-in for R2 answers a submit with an S3 endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum R2Answer {
    /// The bucket takes the canary.
    #[default]
    Ok,
    /// One error, on the bucket.
    RefuseBucket,
}

/// The body of `POST /__e2e/setup`; every part may be left out.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Script {
    /// What Discord says. Default `ok`.
    #[serde(default)]
    pub discord: DiscordAnswer,
    /// What R2 says. Default `ok`. A folder is never scripted.
    #[serde(default)]
    pub r2: R2Answer,
}

/// The validator setup mode runs here: see the module's doc comment.
struct Validator {
    script: Script,
    /// For a folder on this machine only: nothing else is ever handed to it.
    folders: LiveValidator,
}

impl CredentialValidator for Validator {
    async fn validate_discord(&self, _: &str, _: &str, _: &str) -> Vec<FieldError> {
        match self.script.discord {
            DiscordAnswer::Ok => Vec::new(),
            DiscordAnswer::RefuseToken => {
                vec![FieldError::new(Field::DiscordToken, TOKEN_REFUSED)]
            }
        }
    }

    async fn validate_r2(&self, r2: &R2Config) -> Result<(), FieldError> {
        if leaf_core::media::is_local_endpoint(&r2.endpoint) {
            return self.folders.validate_r2(r2).await;
        }
        match self.script.r2 {
            R2Answer::Ok => Ok(()),
            R2Answer::RefuseBucket => Err(FieldError::new(Field::R2Bucket, BUCKET_REFUSED)),
        }
    }
}

/// One run of setup mode: the page, its code, and the data directory its
/// submit saves `leaf.conf` to.
pub struct Stage {
    /// `leaf_server::setup`'s own router: `/setup` and everything under it.
    pub router: Router,
    /// The setup code, as leaf would print it to its log.
    code: String,
    /// The data directory; removed with the stage.
    dir: tempfile::TempDir,
}

impl Stage {
    /// A setup mode nobody has submitted to yet, answering as `script` says.
    pub fn start(script: Script) -> anyhow::Result<Self> {
        let dir = tempfile::tempdir().context("creating the temp dir for setup mode")?;
        let folders = LiveValidator::new().context("building the storage validator")?;
        let mode = setup_mode(
            dir.path().join(CONFIG_FILE_NAME),
            Validator { script, folders },
        );
        Ok(Self {
            router: mode.router,
            code: mode.code,
            dir,
        })
    }

    fn config_path(&self) -> PathBuf {
        self.dir.path().join(CONFIG_FILE_NAME)
    }

    /// What `POST /__e2e/setup` answers: what an operator has before the
    /// page is opened.
    pub fn opening(&self) -> Value {
        json!({
            "code": self.code,
            "data_dir": self.dir.path(),
        })
    }

    /// What `GET /__e2e/setup` answers: the data directory as it stands.
    pub fn status(&self) -> anyhow::Result<Value> {
        let path = self.config_path();
        let saved = Tier1Config::load(&path).context("loading the saved leaf.conf")?;
        let raw = if saved.is_some() {
            Some(std::fs::read_to_string(&path).context("reading the saved leaf.conf")?)
        } else {
            None
        };
        Ok(json!({
            "saved": saved.map(|cfg| json!({
                "client_id": cfg.client_id,
                "public_url": cfg.public_url,
                "r2": {
                    "endpoint": cfg.r2.endpoint,
                    "bucket": cfg.r2.bucket,
                    "access_key_id": cfg.r2.access_key_id,
                    "secret_access_key": cfg.r2.secret_access_key,
                },
            })),
            "raw": raw,
            "entries": entries(self.dir.path())?,
        }))
    }
}

/// The names of everything directly inside `dir`, sorted.
fn entries(dir: &Path) -> anyhow::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).context("listing the data directory")? {
        let entry = entry.context("listing the data directory")?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}
