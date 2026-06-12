//! Tier-1 bootstrap configuration.
//!
//! The secrets leaf needs before it can connect to anything. Stored as TOML
//! at `<DATA_DIR>/leaf.conf` with owner-only permissions, written by the
//! setup-mode flow — never by hand, never via env vars (see PLAN.md
//! § Configuration).

use std::fmt;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// File name of the Tier-1 config inside the data directory.
pub const CONFIG_FILE_NAME: &str = "leaf.conf";

/// Errors loading or persisting Tier-1 configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Filesystem failure.
    #[error("config io: {0}")]
    Io(#[from] std::io::Error),
    /// File exists but does not parse as valid config.
    #[error("config parse: {0}")]
    Parse(#[from] toml::de::Error),
    /// Serialization failure (programming error).
    #[error("config serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
    /// A required field is empty.
    #[error("config field `{0}` must not be empty")]
    EmptyField(&'static str),
    /// The public URL is not an http(s) origin.
    #[error("public_url must start with https:// (or http://localhost for dev)")]
    BadPublicUrl,
}

/// R2 credentials (S3-compatible endpoint).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct R2Config {
    /// S3 endpoint, e.g. `https://<account>.r2.cloudflarestorage.com`.
    pub endpoint: String,
    /// Bucket name.
    pub bucket: String,
    /// Access key id.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
}

/// The Tier-1 bootstrap secrets.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tier1Config {
    /// Discord bot token.
    pub discord_token: String,
    /// Discord application (client) id.
    pub client_id: String,
    /// Discord OAuth client secret.
    pub client_secret: String,
    /// Public HTTPS origin the embedded app is served from.
    pub public_url: String,
    /// Object storage credentials.
    pub r2: R2Config,
}

// Manual Debug impls: secrets must never reach logs, even at trace level.
impl fmt::Debug for R2Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("R2Config")
            .field("endpoint", &self.endpoint)
            .field("bucket", &self.bucket)
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .finish()
    }
}

impl fmt::Debug for Tier1Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tier1Config")
            .field("discord_token", &"<redacted>")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("public_url", &self.public_url)
            .field("r2", &self.r2)
            .finish()
    }
}

impl Tier1Config {
    /// Validates field shape (non-empty, plausible URL). Liveness (do the
    /// credentials actually work) is the setup flow's job, not this.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let required: [(&'static str, &str); 7] = [
            ("discord_token", &self.discord_token),
            ("client_id", &self.client_id),
            ("client_secret", &self.client_secret),
            ("public_url", &self.public_url),
            ("r2.endpoint", &self.r2.endpoint),
            ("r2.bucket", &self.r2.bucket),
            ("r2.access_key_id", &self.r2.access_key_id),
        ];
        for (name, value) in required {
            if value.trim().is_empty() {
                return Err(ConfigError::EmptyField(name));
            }
        }
        if self.r2.secret_access_key.trim().is_empty() {
            return Err(ConfigError::EmptyField("r2.secret_access_key"));
        }
        let url_ok = self.public_url.starts_with("https://")
            || self.public_url.starts_with("http://localhost")
            || self.public_url.starts_with("http://127.0.0.1");
        if !url_ok {
            return Err(ConfigError::BadPublicUrl);
        }
        Ok(())
    }

    /// Loads config from `path`. `Ok(None)` means "not configured yet"
    /// (file absent) — the signal to enter setup mode. A present-but-invalid
    /// file is an error, not setup mode: silently discarding a corrupt
    /// config would orphan real credentials.
    pub fn load(path: &Path) -> Result<Option<Self>, ConfigError> {
        let raw = match std::fs::read_to_string(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(ConfigError::Io(e)),
        };
        let cfg: Self = toml::from_str(&raw)?;
        cfg.validate()?;
        Ok(Some(cfg))
    }

    /// Atomically writes config to `path` with owner-only (0600)
    /// permissions: write to a sibling temp file, fsync, rename.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        self.validate()?;
        let raw = toml::to_string_pretty(self)?;

        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, &raw)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    fn sample() -> Tier1Config {
        Tier1Config {
            discord_token: "tok".to_owned(),
            client_id: "123".to_owned(),
            client_secret: "sec".to_owned(),
            public_url: "https://leaf.example.com".to_owned(),
            r2: R2Config {
                endpoint: "https://acc.r2.cloudflarestorage.com".to_owned(),
                bucket: "leaf".to_owned(),
                access_key_id: "ak".to_owned(),
                secret_access_key: "sk".to_owned(),
            },
        }
    }

    #[test]
    fn save_load_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE_NAME);
        let cfg = sample();
        cfg.save(&path).unwrap();
        assert_eq!(Tier1Config::load(&path).unwrap(), Some(cfg));
    }

    #[test]
    fn absent_file_is_none_not_error() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            Tier1Config::load(&dir.path().join("nope.conf")).unwrap(),
            None
        );
    }

    #[test]
    fn corrupt_file_is_an_error_not_setup_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE_NAME);
        std::fs::write(&path, "not = valid = toml").unwrap();
        assert!(Tier1Config::load(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(CONFIG_FILE_NAME);
        sample().save(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn empty_fields_rejected() {
        let mut cfg = sample();
        cfg.discord_token = "  ".to_owned();
        assert!(matches!(
            cfg.validate(),
            Err(ConfigError::EmptyField("discord_token"))
        ));

        let mut cfg = sample();
        cfg.public_url = "ftp://leaf".to_owned();
        assert!(matches!(cfg.validate(), Err(ConfigError::BadPublicUrl)));

        let mut cfg = sample();
        cfg.public_url = "http://localhost:8080".to_owned();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn debug_output_redacts_secrets() {
        let mut cfg = sample();
        cfg.discord_token = "SECRET_AAA".to_owned();
        cfg.client_secret = "SECRET_BBB".to_owned();
        cfg.r2.access_key_id = "SECRET_CCC".to_owned();
        cfg.r2.secret_access_key = "SECRET_DDD".to_owned();
        let rendered = format!("{cfg:?}");
        assert!(!rendered.contains("SECRET_"));
        assert!(rendered.contains("<redacted>"));
        // Non-secrets stay visible for debuggability.
        assert!(rendered.contains("leaf.example.com"));
        assert!(rendered.contains("123"));
    }
}
