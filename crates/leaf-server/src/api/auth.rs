//! API authentication: short-lived HMAC session tokens, and the Discord
//! calls the token exchange and membership checks need (behind a trait so
//! the routes are testable without the network).
//!
//! Flow: the embedded app `authorize`s, POSTs the code to `/api/token`; we
//! exchange it server-side (client secret never leaves the server), resolve
//! the user id, and mint a session token. Subsequent calls carry that token
//! as `Authorization: Bearer …` and never re-hit Discord for identity —
//! only guild membership is re-checked (it can change), via the bot token.
//!
//! A session token lasts [`SESSION_TTL_SECS`]. While it is still valid it
//! can be swapped for a fresh one (`POST /api/token/refresh`) until
//! [`SESSION_MAX_AGE_SECS`] after the original sign-in, so a long sitting
//! does not die mid-edit and a leaked token still ages out.

use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hmac::{Hmac, Mac};
use sha2::{Digest as _, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Default session lifetime: long enough for a gallery sitting, short
/// enough that a leaked token expires on its own.
pub const SESSION_TTL_SECS: i64 = 6 * 3600;

/// How long after the original sign-in a session may keep being renewed.
/// Matches the lifetime of the Discord OAuth grant the sign-in rested on.
pub const SESSION_MAX_AGE_SECS: i64 = 7 * 86_400;

/// First field of a session token payload. Tokens minted before renewal
/// existed have no marker (`user_id:exp`) and are still accepted.
const SESSION_MARKER: &str = "s2";

/// What a session token carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionClaims {
    /// Discord user snowflake.
    pub user_id: String,
    /// When the user originally signed in (unix seconds). Renewals keep it,
    /// which is what bounds a session's total life.
    pub auth_at: i64,
    /// When this token stops being accepted (unix seconds).
    pub exp: i64,
}

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

    /// Mints a token for `user_id` valid for `ttl_secs` from `now_unix`,
    /// recording `now_unix` as the sign-in time.
    #[must_use]
    pub fn mint(&self, user_id: &str, now_unix: i64, ttl_secs: i64) -> String {
        self.mint_session(user_id, now_unix, now_unix + ttl_secs)
    }

    fn mint_session(&self, user_id: &str, auth_at: i64, exp: i64) -> String {
        let payload = format!("{SESSION_MARKER}\0{user_id}\0{auth_at}\0{exp}");
        let sig = self.mac(payload.as_bytes()).finalize().into_bytes();
        format!("{}.{}", B64.encode(payload.as_bytes()), B64.encode(sig))
    }

    /// Verifies a token and returns its `user_id` if the signature is valid
    /// and it has not expired as of `now_unix`.
    pub fn verify(&self, token: &str, now_unix: i64) -> Result<String, AuthError> {
        self.verify_session(token, now_unix).map(|c| c.user_id)
    }

    /// As [`Self::verify`], returning everything the token carries.
    pub fn verify_session(&self, token: &str, now_unix: i64) -> Result<SessionClaims, AuthError> {
        let (payload_b64, sig_b64) = token.split_once('.').ok_or(AuthError::Malformed)?;
        let payload = B64.decode(payload_b64).map_err(|_| AuthError::Malformed)?;
        let sig = B64.decode(sig_b64).map_err(|_| AuthError::Malformed)?;

        // Constant-time verification via the MAC itself.
        self.mac(&payload)
            .verify_slice(&sig)
            .map_err(|_| AuthError::BadSignature)?;

        let payload = String::from_utf8(payload).map_err(|_| AuthError::Malformed)?;
        let claims = parse_session(&payload).ok_or(AuthError::Malformed)?;
        if now_unix >= claims.exp {
            return Err(AuthError::Expired);
        }
        if claims.user_id.is_empty() {
            return Err(AuthError::Malformed);
        }
        Ok(claims)
    }

    /// Swaps a still-valid session for a fresh token: `(token, expires_in)`.
    ///
    /// The new token lasts [`SESSION_TTL_SECS`], cut short so it never
    /// outlives [`SESSION_MAX_AGE_SECS`] from the original sign-in. `None`
    /// once that cap has passed: the user has to sign in again.
    #[must_use]
    pub fn renew(&self, claims: &SessionClaims, now_unix: i64) -> Option<(String, i64)> {
        let cap = claims.auth_at.saturating_add(SESSION_MAX_AGE_SECS);
        let exp = now_unix.saturating_add(SESSION_TTL_SECS).min(cap);
        let expires_in = exp - now_unix;
        (expires_in > 0).then(|| {
            (
                self.mint_session(&claims.user_id, claims.auth_at, exp),
                expires_in,
            )
        })
    }
}

/// Parses a verified session payload: the current
/// `s2\0user_id\0auth_at\0exp`, or the older `user_id:exp` (every such
/// token was minted with [`SESSION_TTL_SECS`], which dates its sign-in).
fn parse_session(payload: &str) -> Option<SessionClaims> {
    let mut parts = payload.split('\0');
    if let (Some(SESSION_MARKER), Some(user_id), Some(auth_at), Some(exp), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        return Some(SessionClaims {
            user_id: user_id.to_owned(),
            auth_at: auth_at.parse().ok()?,
            exp: exp.parse().ok()?,
        });
    }
    if payload.contains('\0') {
        // Some other token kind (admin), never a session.
        return None;
    }
    let (user_id, exp) = payload.rsplit_once(':')?;
    let exp: i64 = exp.parse().ok()?;
    Some(SessionClaims {
        user_id: user_id.to_owned(),
        auth_at: exp.saturating_sub(SESSION_TTL_SECS),
        exp,
    })
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

/// What an admin session token carries: who, and which guilds they manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminClaims {
    /// Discord user snowflake.
    pub user_id: String,
    /// Guild ids the user managed at login (Manage-Guild or owner).
    pub guild_ids: Vec<String>,
}

/// Admin tokens authorize the web panel.
///
/// Minted only after a browser OAuth login proves the user manages a guild,
/// and domain-separated from gallery tokens so neither works as the other.
/// The managed-guild set is baked in at login — re-login to pick up changes.
impl SessionKey {
    /// Mints an admin token for `user_id` managing `guild_ids`.
    #[must_use]
    pub fn mint_admin(
        &self,
        user_id: &str,
        guild_ids: &[String],
        now_unix: i64,
        ttl_secs: i64,
    ) -> String {
        let payload = format!(
            "admin\0{user_id}\0{}\0{}",
            guild_ids.join(","),
            now_unix + ttl_secs
        );
        let sig = self.mac(payload.as_bytes()).finalize().into_bytes();
        format!("{}.{}", B64.encode(payload.as_bytes()), B64.encode(sig))
    }

    /// Verifies an admin token, returning its claims if valid and unexpired.
    pub fn verify_admin(&self, token: &str, now_unix: i64) -> Result<AdminClaims, AuthError> {
        let (payload_b64, sig_b64) = token.split_once('.').ok_or(AuthError::Malformed)?;
        let payload = B64.decode(payload_b64).map_err(|_| AuthError::Malformed)?;
        let sig = B64.decode(sig_b64).map_err(|_| AuthError::Malformed)?;
        self.mac(&payload)
            .verify_slice(&sig)
            .map_err(|_| AuthError::BadSignature)?;

        let payload = String::from_utf8(payload).map_err(|_| AuthError::Malformed)?;
        let mut parts = payload.split('\0');
        if parts.next() != Some("admin") {
            return Err(AuthError::Malformed);
        }
        let (Some(user_id), Some(guilds), Some(exp), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(AuthError::Malformed);
        };
        let exp: i64 = exp.parse().map_err(|_| AuthError::Malformed)?;
        if now_unix >= exp {
            return Err(AuthError::Expired);
        }
        if user_id.is_empty() {
            return Err(AuthError::Malformed);
        }
        let guild_ids = if guilds.is_empty() {
            Vec::new()
        } else {
            guilds.split(',').map(ToOwned::to_owned).collect()
        };
        Ok(AdminClaims {
            user_id: user_id.to_owned(),
            guild_ids,
        })
    }

    /// Signs a short-lived OAuth `state` (a CSRF/replay guard for admin login).
    #[must_use]
    pub fn sign_oauth_state(&self, now_unix: i64, ttl_secs: i64) -> String {
        let exp = now_unix + ttl_secs;
        let sig = self
            .mac(format!("oauth-state\0{exp}").as_bytes())
            .finalize()
            .into_bytes();
        format!("{exp}.{}", B64.encode(sig))
    }

    /// Verifies an OAuth `state` we issued that has not expired.
    #[must_use]
    pub fn verify_oauth_state(&self, state: &str, now_unix: i64) -> bool {
        let Some((exp_str, sig_b64)) = state.split_once('.') else {
            return false;
        };
        let Ok(exp) = exp_str.parse::<i64>() else {
            return false;
        };
        if now_unix >= exp {
            return false;
        }
        let Ok(sig) = B64.decode(sig_b64) else {
            return false;
        };
        self.mac(format!("oauth-state\0{exp}").as_bytes())
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

/// Bucket that signed-media expiries are quantized to.
///
/// Every session shares one URL per attachment per bucket — turning
/// per-session cache misses (one R2 op each) into shared edge cache hits.
/// Tradeoff: a minted URL is replayable until its bucket boundary (1–2×
/// this). One day balances the cache hit rate against the replay window for
/// already-entitled media.
pub const SIGNED_URL_BUCKET_SECS: i64 = 86_400;

/// Rounds `now` to a stable expiry at least one bucket in the future. The
/// result depends only on `now`'s bucket, so every session within a bucket
/// mints identical (edge-cacheable) URLs.
const fn quantized_exp(now_unix: i64) -> i64 {
    (now_unix / SIGNED_URL_BUCKET_SECS + 2) * SIGNED_URL_BUCKET_SECS
}

impl<'a> MediaSigner<'a> {
    /// A signer whose expiry is quantized to a shared bucket (see
    /// [`SIGNED_URL_BUCKET_SECS`]) so the URLs it emits are identical for
    /// every viewer in that bucket, while still expiring on their own.
    #[must_use]
    pub const fn new(key: &'a SessionKey, now_unix: i64) -> Self {
        Self {
            key,
            exp: quantized_exp(now_unix),
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

/// A guild member as the API needs them: role ids, when they joined (for
/// the membership-age policy) and what the server calls them. From the bot
/// member lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildMember {
    /// The member's role snowflakes.
    pub roles: Vec<String>,
    /// When they joined the guild, unix seconds, if Discord reported it.
    pub joined_at: Option<i64>,
    /// Display name in this guild: server nickname, else global display
    /// name, else username. `None` when Discord sent none of them.
    pub name: Option<String>,
}

/// Discord channel type: a regular text channel.
pub const CHANNEL_KIND_TEXT: u8 = 0;
/// Discord channel type: an announcement (news) channel.
pub const CHANNEL_KIND_ANNOUNCEMENT: u8 = 5;

/// A guild channel the bot can see, for the creator and admin pickers.
/// Creators are only offered the watched subset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildChannel {
    /// Channel snowflake.
    pub id: String,
    /// Channel name (without the leading `#`).
    pub name: String,
    /// Discord's channel type (see [`CHANNEL_KIND_TEXT`]); the list also
    /// holds categories, voice channels and forums.
    pub kind: u8,
    /// Sorting position within the guild's channel list.
    pub position: i64,
}

impl GuildChannel {
    /// True for a channel leaf can post lines in and archive from: text and
    /// announcement channels.
    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self.kind, CHANNEL_KIND_TEXT | CHANNEL_KIND_ANNOUNCEMENT)
    }
}

/// A guild role members can hold, for the role pickers and for checking a
/// role id someone sent.
///
/// `@everyone` and bots' own roles are filtered out; other managed roles
/// (server boosters, a streamer's subscribers) are kept, as in the bot's
/// `/setup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildRole {
    /// Role snowflake.
    pub id: String,
    /// Role name.
    pub name: String,
    /// Position in the guild's role list; higher sits nearer the top.
    pub position: i64,
}

/// A guild's name and icon, to show which server is being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuildSummary {
    /// Guild name.
    pub name: String,
    /// Icon hash for the Discord CDN, if the guild has an icon.
    pub icon: Option<String>,
}

/// Why an OAuth code could not be exchanged. `Display` is for logs only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ExchangeError {
    /// Discord refused the code (used, expired, wrong redirect URI or wrong
    /// client credentials). Asking again with the same code cannot work.
    #[error("Discord rejected the code: {0}")]
    Rejected(String),
    /// Discord could not be reached, or answered 5xx / 429. Passing.
    #[error("Discord unavailable: {0}")]
    Unavailable(String),
}

/// Why a bot-token member lookup failed. `Display` is for logs only.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LookupError {
    /// leaf's bot is not in the guild, so it cannot see who is.
    #[error("the bot is not in this guild")]
    BotNotInGuild,
    /// Discord could not be reached, or answered with an error. Passing.
    #[error("{0}")]
    Unavailable(String),
}

/// The Discord calls the API needs. Behind a trait so routes test offline.
pub trait DiscordApi: Send + Sync + 'static {
    /// Exchanges an OAuth `code` for the user's access token.
    fn exchange_code(
        &self,
        code: &str,
        redirect_uri: &str,
    ) -> impl Future<Output = Result<String, ExchangeError>> + Send;

    /// Resolves the user id behind an access token (`/users/@me`).
    fn current_user_id(
        &self,
        access_token: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;

    /// A user's membership in a guild (roles, join time, display name), or
    /// `None` if they are not a member. Uses the bot token (the bot is in
    /// the guild), so no extra OAuth scope is required of the user.
    fn guild_member(
        &self,
        guild_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<GuildMember>, LookupError>> + Send;

    /// The guild's channels, via the bot token. Used to label the watched
    /// channel picker and to offer the admin panel's log channel.
    fn guild_channels(
        &self,
        guild_id: &str,
    ) -> impl Future<Output = Result<Vec<GuildChannel>, String>> + Send;

    /// The guild's assignable roles, via the bot token. Used by the role
    /// pickers and to name the creator role.
    fn guild_roles(
        &self,
        guild_id: &str,
    ) -> impl Future<Output = Result<Vec<GuildRole>, String>> + Send;

    /// The guild's name and icon, via the bot token. `None` when the bot is
    /// not in the guild.
    fn guild_summary(
        &self,
        guild_id: &str,
    ) -> impl Future<Output = Result<Option<GuildSummary>, String>> + Send;

    /// Guild ids the access-token's user can manage (owner or Manage-Guild),
    /// via `/users/@me/guilds`. Gates the admin panel; needs the `guilds`
    /// OAuth scope.
    fn managed_guild_ids(
        &self,
        access_token: &str,
    ) -> impl Future<Output = Result<Vec<String>, String>> + Send;

    /// Posts `content` in a channel as the bot, quietly: no mention in it
    /// pings anyone and nobody gets a push notification. For log lines.
    fn send_message(
        &self,
        channel_id: &str,
        content: &str,
    ) -> impl Future<Output = Result<(), String>> + Send;
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
        let signer = MediaSigner::new(&key, 0);
        let (full, thumb) = signer.urls("att9");
        assert!(full.starts_with("/api/media/att9?exp="));
        assert!(thumb.contains("thumb=1"));
    }

    #[test]
    fn media_signer_quantizes_exp_into_a_shared_bucket() {
        let key = SessionKey::derive("k");
        // Two sessions seconds apart in the same bucket mint identical URLs,
        // so the edge caches one object instead of one per session.
        let a = MediaSigner::new(&key, 10).urls("att");
        let b = MediaSigner::new(&key, 20).urls("att");
        assert_eq!(a, b);
        // The shared expiry is bucket-aligned and outlives the session TTL.
        assert!(a.0.contains(&format!("exp={}", 2 * SIGNED_URL_BUCKET_SECS)));
    }

    #[test]
    fn a_colon_in_the_user_id_does_not_confuse_the_fields() {
        // user_id is a snowflake (digits only); the fields are NUL-separated
        // so the expiry stays unambiguous even if that ever changed.
        let key = SessionKey::derive("k");
        let tok = key.mint("abc:def", 0, 100);
        assert_eq!(key.verify(&tok, 0).unwrap(), "abc:def");
    }

    /// A token in the pre-renewal `user_id:exp` format.
    fn legacy_token(key: &SessionKey, user_id: &str, exp: i64) -> String {
        let payload = format!("{user_id}:{exp}");
        let sig = key.mac(payload.as_bytes()).finalize().into_bytes();
        format!("{}.{}", B64.encode(payload.as_bytes()), B64.encode(sig))
    }

    #[test]
    fn session_claims_carry_the_sign_in_time() {
        let key = SessionKey::derive("k");
        let tok = key.mint("42", 1000, SESSION_TTL_SECS);
        assert_eq!(
            key.verify_session(&tok, 1000).unwrap(),
            SessionClaims {
                user_id: "42".to_owned(),
                auth_at: 1000,
                exp: 1000 + SESSION_TTL_SECS,
            }
        );
    }

    #[test]
    fn tokens_from_before_renewal_still_verify() {
        let key = SessionKey::derive("k");
        let exp = 5000 + SESSION_TTL_SECS;
        let tok = legacy_token(&key, "42", exp);
        // Sign-in time is inferred from the fixed lifetime they all had.
        assert_eq!(
            key.verify_session(&tok, 5000).unwrap(),
            SessionClaims {
                user_id: "42".to_owned(),
                auth_at: 5000,
                exp,
            }
        );
        assert_eq!(key.verify(&tok, exp), Err(AuthError::Expired));
    }

    #[test]
    fn renew_slides_the_expiry_and_keeps_the_sign_in_time() {
        let key = SessionKey::derive("k");
        let first = key.verify_session(&key.mint("42", 0, SESSION_TTL_SECS), 0);
        let (tok, expires_in) = key.renew(&first.unwrap(), 3600).unwrap();
        assert_eq!(expires_in, SESSION_TTL_SECS);
        assert_eq!(
            key.verify_session(&tok, 3600).unwrap(),
            SessionClaims {
                user_id: "42".to_owned(),
                auth_at: 0,
                exp: 3600 + SESSION_TTL_SECS,
            }
        );
    }

    #[test]
    fn renew_stops_at_the_seven_day_cap() {
        let key = SessionKey::derive("k");
        let claims = SessionClaims {
            user_id: "42".to_owned(),
            auth_at: 0,
            exp: SESSION_MAX_AGE_SECS,
        };
        // One hour before the cap: a token for that last hour only.
        let (tok, expires_in) = key.renew(&claims, SESSION_MAX_AGE_SECS - 3600).unwrap();
        assert_eq!(expires_in, 3600);
        assert_eq!(
            key.verify(&tok, SESSION_MAX_AGE_SECS),
            Err(AuthError::Expired)
        );
        // At and after the cap there is nothing left to hand out.
        assert_eq!(key.renew(&claims, SESSION_MAX_AGE_SECS), None);
        assert_eq!(key.renew(&claims, SESSION_MAX_AGE_SECS + 1), None);
    }

    #[test]
    fn admin_token_round_trips_claims_and_expires() {
        let key = SessionKey::derive("k");
        let guilds = vec!["g1".to_owned(), "g2".to_owned()];
        let tok = key.mint_admin("u1", &guilds, 1000, 100);
        let claims = key.verify_admin(&tok, 1000).unwrap();
        assert_eq!(claims.user_id, "u1");
        assert_eq!(claims.guild_ids, guilds);
        assert!(key.verify_admin(&tok, 1099).is_ok());
        assert_eq!(key.verify_admin(&tok, 1100), Err(AuthError::Expired));
    }

    #[test]
    fn admin_token_handles_no_managed_guilds() {
        let key = SessionKey::derive("k");
        let tok = key.mint_admin("u1", &[], 0, 100);
        assert!(key.verify_admin(&tok, 0).unwrap().guild_ids.is_empty());
    }

    #[test]
    fn admin_and_gallery_tokens_are_not_interchangeable() {
        let key = SessionKey::derive("k");
        // A gallery token must not verify as admin, and vice versa.
        let gallery = key.mint("u1", 0, 100);
        assert!(key.verify_admin(&gallery, 0).is_err());
        let admin = key.mint_admin("u1", &["g".to_owned()], 0, 100);
        assert!(key.verify(&admin, 0).is_err());
        // A different key rejects an admin token.
        assert!(SessionKey::derive("other").verify_admin(&admin, 0).is_err());
    }

    #[test]
    fn oauth_state_signs_and_expires() {
        let key = SessionKey::derive("k");
        let state = key.sign_oauth_state(1000, 60);
        assert!(key.verify_oauth_state(&state, 1000));
        assert!(!key.verify_oauth_state(&state, 1060)); // expired
        assert!(!key.verify_oauth_state("garbage", 1000));
        assert!(!SessionKey::derive("other").verify_oauth_state(&state, 1000));
    }
}
