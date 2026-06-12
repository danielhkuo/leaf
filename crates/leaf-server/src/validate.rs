//! Live credential validation against Discord and R2, used by setup mode.
//!
//! Errors are returned as operator-facing strings (they render on the
//! setup page), so they name the failing service and likely cause without
//! ever echoing the credential itself.

use leaf_core::config::R2Config;
use object_store::ObjectStore as _;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectPath;

use crate::setup::CredentialValidator;

const DISCORD_API: &str = "https://discord.com/api/v10";
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Validator that talks to the real services.
#[derive(Debug, Clone)]
pub struct LiveValidator {
    http: reqwest::Client,
}

impl LiveValidator {
    /// Builds the validator (constructs its HTTP client).
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            http: reqwest::Client::builder().timeout(TIMEOUT).build()?,
        })
    }
}

impl CredentialValidator for LiveValidator {
    async fn validate_discord(
        &self,
        token: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<(), String> {
        // 1. Bot token: GET /users/@me must succeed.
        let resp = self
            .http
            .get(format!("{DISCORD_API}/users/@me"))
            .header("Authorization", format!("Bot {token}"))
            .send()
            .await
            .map_err(|e| format!("could not reach Discord: {e}"))?;
        match resp.status() {
            s if s.is_success() => {}
            reqwest::StatusCode::UNAUTHORIZED => {
                return Err("Discord rejected the bot token".to_owned());
            }
            s => return Err(format!("Discord returned {s} for the bot token check")),
        }

        // 2. Client id/secret: the client-credentials grant succeeds only
        //    for a valid pair.
        let resp = self
            .http
            .post(format!("{DISCORD_API}/oauth2/token"))
            .basic_auth(client_id, Some(client_secret))
            .form(&[("grant_type", "client_credentials"), ("scope", "identify")])
            .send()
            .await
            .map_err(|e| format!("could not reach Discord OAuth: {e}"))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err("Discord rejected the client ID / client secret pair".to_owned())
        }
    }

    async fn validate_r2(&self, r2: &R2Config) -> Result<(), String> {
        let store = AmazonS3Builder::new()
            .with_endpoint(&r2.endpoint)
            .with_bucket_name(&r2.bucket)
            .with_access_key_id(&r2.access_key_id)
            .with_secret_access_key(&r2.secret_access_key)
            .with_region("auto")
            .build()
            .map_err(|e| format!("invalid R2 settings: {e}"))?;

        let canary = ObjectPath::from("leaf-setup-canary");
        store
            .put(&canary, bytes::Bytes::from_static(b"leaf").into())
            .await
            .map_err(|e| format!("R2 write failed (check endpoint/bucket/keys): {e}"))?;
        store
            .get(&canary)
            .await
            .map_err(|e| format!("R2 read-back failed: {e}"))?;
        store
            .delete(&canary)
            .await
            .map_err(|e| format!("R2 delete failed (key needs delete permission): {e}"))?;
        Ok(())
    }
}
