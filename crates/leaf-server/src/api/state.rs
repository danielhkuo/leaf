//! Shared API state and the `AuthUser` extractor.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{FromRef, FromRequestParts};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo};
use object_store::ObjectStore;

use crate::api::auth::{DiscordApi, SessionKey, now_unix};
use crate::api::error::ApiError;

/// Guild-membership cache: `(guild_id, user_id)` → Discord's answer
/// (`None` = not a member). See [`membership_cache`].
pub type MembershipCache = moka::future::Cache<(String, String), Option<Vec<String>>>;

/// How long a membership lookup is trusted before re-checking with Discord.
/// Short enough that a removed member loses gallery access promptly; long
/// enough that a gallery sitting is a handful of lookups, not hundreds.
const MEMBERSHIP_TTL: Duration = Duration::from_mins(1);

/// Cap on cached `(guild, user)` pairs — bounds memory for a public archive.
const MEMBERSHIP_CACHE_CAPACITY: u64 = 10_000;

/// Builds the membership cache with leaf's standard TTL and capacity.
#[must_use]
pub fn membership_cache() -> MembershipCache {
    moka::future::Cache::builder()
        .time_to_live(MEMBERSHIP_TTL)
        .max_capacity(MEMBERSHIP_CACHE_CAPACITY)
        .build()
}

/// Everything the API handlers share. Generic over the `DiscordApi` impl so
/// tests substitute a mock; cheap to clone (repos and `Arc`s).
pub struct ApiState<D> {
    /// Series repository.
    pub series: SeriesRepo,
    /// Posts + media repository.
    pub posts: PostRepo,
    /// Guild settings repository.
    pub guilds: GuildSettingsRepo,
    /// Object storage backing the media proxy.
    pub store: Arc<dyn ObjectStore>,
    /// Session/media signing key.
    pub key: SessionKey,
    /// Discord calls (token exchange, membership).
    pub discord: Arc<D>,
    /// Short-TTL, single-flight cache in front of `guild_member_roles`. The
    /// gallery re-checks membership on every request; without this, a burst
    /// of concurrent tile loads becomes a burst of identical Discord
    /// member-lookups that trips Discord's per-route rate limit (429 →
    /// "couldn't load these days"). See [`membership_cache`].
    pub membership: MembershipCache,
    /// Default OAuth redirect URI for code exchange (the public origin).
    pub redirect_uri: String,
    /// OAuth client id (public) — builds the admin login URL.
    pub client_id: String,
}

// Manual `Clone`: deriving would wrongly require `D: Clone` (we only hold
// `Arc<D>`, which clones regardless).
impl<D> Clone for ApiState<D> {
    fn clone(&self) -> Self {
        Self {
            series: self.series.clone(),
            posts: self.posts.clone(),
            guilds: self.guilds.clone(),
            store: Arc::clone(&self.store),
            key: self.key.clone(),
            discord: Arc::clone(&self.discord),
            membership: self.membership.clone(),
            redirect_uri: self.redirect_uri.clone(),
            client_id: self.client_id.clone(),
        }
    }
}

impl<D: DiscordApi> FromRef<ApiState<D>> for SessionKey {
    fn from_ref(state: &ApiState<D>) -> Self {
        state.key.clone()
    }
}

/// The authenticated caller, extracted from the `Authorization: Bearer`
/// session token. Identity only — guild membership is checked per route.
pub struct AuthUser {
    /// Discord user snowflake.
    pub user_id: String,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    SessionKey: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let key = SessionKey::from_ref(state);
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        let user_id = key
            .verify(token.trim(), now_unix())
            .map_err(|_| ApiError::Unauthorized)?;
        Ok(Self { user_id })
    }
}
