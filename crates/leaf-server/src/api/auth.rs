//! API authentication: short-lived HMAC session tokens, and the Discord
//! calls the token exchange and membership checks need (behind a trait so
//! the routes are testable without the network).
//!
//! Flow: the embedded app `authorize`s, POSTs the code to `/api/token`; we
//! exchange it server-side (client secret never leaves the server), resolve
//! the user id, and mint a session token. Subsequent calls carry that token
//! as `Authorization: Bearer …` and never re-hit Discord for identity —
//! only guild membership is re-checked (it can change), via the bot token.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use sha2::{Digest as _, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Default session lifetime: long enough for a gallery sitting, short
/// enough that a leaked token expires on its own.
pub const SESSION_TTL_SECS: i64 = 6 * 3600;

/// HMAC key for session tokens.
///
/// Derived from the OAuth client secret so it is stable across restarts
/// (tokens survive a redeploy) without storing a separate secret. Rotating
/// the client secret invalidates live tokens — acceptable, that is a
/// deliberate credential rotation.
#[derive(Clone)]
pub struct SessionKey(Vec<u8>);

impl SessionKey {
    /// Derives the key from the client secret.
    #[must_use]
    pub fn derive(client_secret: &str) -> Self {
        let mut h = Sha256::new();
        h.update(b"leaf-session-key-v1\0");
        h.update(client_secret.as_bytes());
        Self(h.finalize().to_vec())
    }

    #[allow(
        clippy::expect_used,
        reason = "HMAC-SHA256 accepts a key of any length; this never errors"
    )]
    fn mac(&self, payload: &[u8]) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(payload);
        mac
    }

    /// Mints a token for `user_id` valid for `ttl_secs` from `now_unix`.
    #[must_use]
    pub fn mint(&self, user_id: &str, now_unix: i64, ttl_secs: i64) -> String {
        let payload = format!("{user_id}:{}", now_unix + ttl_secs);
        let sig = self.mac(payload.as_bytes()).finalize().into_bytes();
        format!("{}.{}", B64.encode(payload.as_bytes()), B64.encode(sig))
    }

    /// Verifies a token and returns its `user_id` if the signature is valid
    /// and it has not expired as of `now_unix`.
    pub fn verify(&self, token: &str, now_unix: i64) -> Result<String, AuthError> {
        let (payload_b64, sig_b64) = token.split_once('.').ok_or(AuthError::Malformed)?;
        let payload = B64.decode(payload_b64).map_err(|_| AuthError::Malformed)?;
        let sig = B64.decode(sig_b64).map_err(|_| AuthError::Malformed)?;

        // Constant-time verification via the MAC itself.
        self.mac(&payload)
            .verify_slice(&sig)
            .map_err(|_| AuthError::BadSignature)?;

        let payload = String::from_utf8(payload).map_err(|_| AuthError::Malformed)?;
        let (user_id, exp) = payload.rsplit_once(':').ok_or(AuthError::Malformed)?;
        let exp: i64 = exp.parse().map_err(|_| AuthError::Malformed)?;
        if now_unix >= exp {
            return Err(AuthError::Expired);
        }
        if user_id.is_empty() {
            return Err(AuthError::Malformed);
        }
        Ok(user_id.to_owned())
    }
}

/// Media URLs are signed, not Bearer-authed: the gallery loads them via
/// `<img src>`, which cannot carry an Authorization header. A signature is
/// minted only after a viewer passes `can_view`, so possession of a valid
/// signed URL is the capability (presigned-URL model, short TTL).
impl SessionKey {
    fn media_mac(&self, attachment_id: &str, exp: i64) -> HmacSha256 {
        self.mac(format!("media\0{attachment_id}\0{exp}").as_bytes())
    }

    /// Signs access to one attachment until `exp` (unix). Returns the hex
    /// signature to place in the URL's `sig` query parameter.
    #[must_use]
    pub fn sign_media(&self, attachment_id: &str, exp: i64) -> String {
        let sig = self.media_mac(attachment_id, exp).finalize().into_bytes();
        B64.encode(sig)
    }

    /// Verifies a media signature for `attachment_id`, unexpired at `now`.
    #[must_use]
    pub fn verify_media(&self, attachment_id: &str, exp: i64, sig: &str, now_unix: i64) -> bool {
        if now_unix >= exp {
            return false;
        }
        let Ok(sig) = B64.decode(sig) else {
            return false;
        };
        self.media_mac(attachment_id, exp)
            .verify_slice(&sig)
            .is_ok()
    }
}

/// Builds signed media URL pairs (full + thumbnail) for one viewing
/// session; carries the key and a shared expiry so DTOs stay key-free.
pub struct MediaSigner<'a> {
    key: &'a SessionKey,
    exp: i64,
}

impl<'a> MediaSigner<'a> {
    /// A signer valid for `ttl_secs` from `now_unix`.
    #[must_use]
    pub const fn new(key: &'a SessionKey, now_unix: i64, ttl_secs: i64) -> Self {
        Self {
            key,
            exp: now_unix + ttl_secs,
        }
    }

    /// `(full_url, thumb_url)` for an attachment, both signed.
    #[must_use]
    pub fn urls(&self, attachment_id: &str) -> (String, String) {
        let sig = self.key.sign_media(attachment_id, self.exp);
        let exp = self.exp;
        (
            format!("/api/media/{attachment_id}?exp={exp}&sig={sig}"),
            format!("/api/media/{attachment_id}?thumb=1&exp={exp}&sig={sig}"),
        )
    }
}

/// Why a session token was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// No / unparseable bearer token.
    #[error("malformed session token")]
    Malformed,
    /// Signature did not verify (forged or wrong key).
    #[error("invalid session token signature")]
    BadSignature,
    /// Token is past its expiry.
    #[error("session token expired")]
    Expired,
}

/// Current unix time in seconds.
#[must_use]
pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The Discord calls the API needs. Behind a trait so routes test offline.
pub trait DiscordApi: Send + Sync + 'static {
    /// Exchanges an OAuth `code` for the user's access token.
    fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;

    /// Resolves the user id behind an access token (`/users/@me`).
    fn current_user_id(
        &self,
        access_token: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;

    /// The user's role ids in a guild, or `None` if they are not a member.
    /// Uses the bot token (the bot is in the guild), so no extra OAuth
    /// scope is required of the user.
    fn guild_member_roles(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<Vec<String>>, String>> + Send;
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "tests may panic")]

    use super::*;

    #[test]
    fn mint_then_verify_round_trips() {
        let key = SessionKey::derive("client-secret");
        let tok = key.mint("123456789", 1000, SESSION_TTL_SECS);
        assert_eq!(key.verify(&tok, 1000).unwrap(), "123456789");
        // Still valid just before expiry, invalid at/after it.
        assert!(key.verify(&tok, 1000 + SESSION_TTL_SECS - 1).is_ok());
        assert_eq!(
            key.verify(&tok, 1000 + SESSION_TTL_SECS),
            Err(AuthError::Expired)
        );
    }

    #[test]
    fn a_different_key_rejects_the_token() {
        let tok = SessionKey::derive("secret-a").mint("7", 0, 100);
        assert_eq!(
            SessionKey::derive("secret-b").verify(&tok, 0),
            Err(AuthError::BadSignature)
        );
    }

    #[test]
    fn tampering_with_the_user_id_is_caught() {
        let key = SessionKey::derive("k");
        let tok = key.mint("100", 0, 100);
        let (_, sig) = tok.split_once('.').unwrap();
        // Swap the payload for a different user, keep the old signature.
        let forged = format!("{}.{sig}", B64.encode(b"999:100"));
        assert_eq!(key.verify(&forged, 0), Err(AuthError::BadSignature));
    }

    #[test]
    fn garbage_is_malformed_not_a_panic() {
        let key = SessionKey::derive("k");
        for junk in ["", "no-dot", "a.b.c", "!!.??", "."] {
            assert!(key.verify(junk, 0).is_err());
        }
    }

    #[test]
    fn media_signatures_verify_and_expire_and_bind_to_id() {
        let key = SessionKey::derive("k");
        let sig = key.sign_media("att1", 1000);
        assert!(key.verify_media("att1", 1000, &sig, 999));
        // Expired.
        assert!(!key.verify_media("att1", 1000, &sig, 1000));
        // Signature is bound to the attachment id.
        assert!(!key.verify_media("att2", 1000, &sig, 999));
        // Garbage signature.
        assert!(!key.verify_media("att1", 1000, "not-base64!!", 999));
    }

    #[test]
    fn media_signer_emits_signed_full_and_thumb_urls() {
        let key = SessionKey::derive("k");
        let signer = MediaSigner::new(&key, 0, 100);
        let (full, thumb) = signer.urls("att9");
        assert!(full.starts_with("/api/media/att9?exp=100&sig="));
        assert!(thumb.contains("thumb=1"));
    }

    #[test]
    fn user_ids_with_no_colon_conflict_resolve_via_rsplit() {
        // user_id is a snowflake (digits only), but rsplit on ':' keeps the
        // exp unambiguous even if that ever changed.
        let key = SessionKey::derive("k");
        let tok = key.mint("abc:def", 0, 100);
        assert_eq!(key.verify(&tok, 0).unwrap(), "abc:def");
    }
}
