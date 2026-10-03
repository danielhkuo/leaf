//! Shared API state, the caches in front of Discord, and the extractors.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

use axum::Json;
use axum::extract::{FromRef, FromRequest, FromRequestParts, Request};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo};
use object_store::ObjectStore;
use serde::de::DeserializeOwned;

use crate::api::auth::{
    DiscordApi, GuildChannel, GuildMember, GuildRole, GuildSummary, SessionClaims, SessionKey,
    now_unix,
};
use crate::api::error::ApiError;

/// Guild-membership cache: `(guild_id, user_id)` → Discord's answer
/// (`None` = not a member). See [`membership_cache`].
pub type MembershipCache = moka::future::Cache<(String, String), Option<GuildMember>>;

/// Guild-channel cache: `guild_id` → the bot-visible channel list. See
/// [`channels_cache`].
pub type ChannelsCache = moka::future::Cache<String, Arc<Vec<GuildChannel>>>;

/// Guild-role cache: `guild_id` → the assignable roles, highest first.
pub type RolesCache = moka::future::Cache<String, Arc<Vec<GuildRole>>>;

/// Guild name/icon cache: `guild_id` → summary (`None` = bot not in guild).
pub type GuildSummaryCache = moka::future::Cache<String, Option<GuildSummary>>;

/// How long a membership lookup is trusted before re-checking with Discord.
/// Short enough that a removed member loses gallery access promptly; long
/// enough that a gallery sitting is a handful of lookups, not hundreds.
const MEMBERSHIP_TTL: Duration = Duration::from_mins(1);

/// Cap on cached `(guild, user)` pairs — bounds memory for a public archive.
const MEMBERSHIP_CACHE_CAPACITY: u64 = 10_000;

/// How long a guild's channel list is trusted; channels change rarely and
/// the picker only needs names, so a few minutes of staleness is fine.
const CHANNELS_TTL: Duration = Duration::from_mins(5);

/// Cap on cached guild channel lists.
const CHANNELS_CACHE_CAPACITY: u64 = 1_000;

/// How long a guild's role list is trusted (same reasoning as channels).
const ROLES_TTL: Duration = Duration::from_mins(5);

/// How long a guild's name and icon are trusted; they only label the panel.
const GUILD_SUMMARY_TTL: Duration = Duration::from_mins(10);

/// Cap on cached role lists and on cached guild summaries.
const GUILD_CACHE_CAPACITY: u64 = 1_000;

/// Builds the membership cache with leaf's standard TTL and capacity.
#[must_use]
pub fn membership_cache() -> MembershipCache {
    moka::future::Cache::builder()
        .time_to_live(MEMBERSHIP_TTL)
        .max_capacity(MEMBERSHIP_CACHE_CAPACITY)
        .build()
}

/// Builds the guild-channel cache.
#[must_use]
pub fn channels_cache() -> ChannelsCache {
    moka::future::Cache::builder()
        .time_to_live(CHANNELS_TTL)
        .max_capacity(CHANNELS_CACHE_CAPACITY)
        .build()
}

/// The process-wide role cache. A static rather than an [`ApiState`] field
/// because the binary builds that struct by literal; one process talks to
/// one Discord application, so keying by guild id is enough.
#[must_use]
pub fn roles_cache() -> &'static RolesCache {
    static CACHE: LazyLock<RolesCache> = LazyLock::new(|| {
        moka::future::Cache::builder()
            .time_to_live(ROLES_TTL)
            .max_capacity(GUILD_CACHE_CAPACITY)
            .build()
    });
    &CACHE
}

/// The process-wide guild name/icon cache (a static for the same reason as
/// [`roles_cache`]).
#[must_use]
pub fn guild_summary_cache() -> &'static GuildSummaryCache {
    static CACHE: LazyLock<GuildSummaryCache> = LazyLock::new(|| {
        moka::future::Cache::builder()
            .time_to_live(GUILD_SUMMARY_TTL)
            .max_capacity(GUILD_CACHE_CAPACITY)
            .build()
    });
    &CACHE
}

/// Which cached Discord list a refresh is for (see [`refresh_allowed`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum GuildList {
    /// The guild's roles.
    Roles,
    /// The guild's channels.
    Channels,
}

/// How long after an early refresh of a guild's list the next one waits.
const REFRESH_EVERY: Duration = Duration::from_secs(10);

/// Whether a guild's cached list may be refetched ahead of its TTL now.
///
/// A list is refetched early when an id someone sent is not in it (the role
/// or channel may be newer than the cached copy). At most once per guild and
/// list every [`REFRESH_EVERY`], so repeating a wrong id cannot be turned
/// into a stream of Discord calls. A `true` answer counts as the refresh.
pub(crate) async fn refresh_allowed(list: GuildList, guild_id: &str) -> bool {
    static RECENT: LazyLock<moka::future::Cache<(GuildList, String), ()>> = LazyLock::new(|| {
        moka::future::Cache::builder()
            .time_to_live(REFRESH_EVERY)
            .max_capacity(GUILD_CACHE_CAPACITY)
            .build()
    });
    let entry = RECENT
        .entry((list, guild_id.to_owned()))
        .or_insert(())
        .await;
    entry.is_fresh()
}

/// How long "the bot is not in this guild" is remembered.
const BOT_ABSENT_TTL: Duration = Duration::from_mins(1);

/// Guilds Discord just said the bot is not in. The membership cache never
/// keeps an error, so without this every request for such a guild would be
/// one more refused bot-token call, and Discord blocks an IP that collects
/// too many of those. A static for the same reason as [`roles_cache`].
fn bot_absent_cache() -> &'static moka::future::Cache<String, ()> {
    static CACHE: LazyLock<moka::future::Cache<String, ()>> = LazyLock::new(|| {
        moka::future::Cache::builder()
            .time_to_live(BOT_ABSENT_TTL)
            .max_capacity(GUILD_CACHE_CAPACITY)
            .build()
    });
    &CACHE
}

/// Remembers for [`BOT_ABSENT_TTL`] that the bot is not in `guild_id`.
pub(crate) async fn note_bot_absent(guild_id: &str) {
    bot_absent_cache().insert(guild_id.to_owned(), ()).await;
}

/// Whether Discord said within [`BOT_ABSENT_TTL`] that the bot is not in
/// `guild_id`.
pub(crate) fn bot_known_absent(guild_id: &str) -> bool {
    bot_absent_cache().contains_key(guild_id)
}

/// Creator display names: `(guild_id, user_id)` → the name (`None` = no
/// longer a member). They only label the admin panel's rows, so they are
/// kept much longer than a membership answer.
pub type CreatorNameCache = moka::future::Cache<(String, String), Option<String>>;

/// How long a creator's display name is trusted.
const CREATOR_NAME_TTL: Duration = Duration::from_mins(10);

/// The process-wide creator-name cache (a static for the same reason as
/// [`roles_cache`]).
#[must_use]
pub fn creator_name_cache() -> &'static CreatorNameCache {
    static CACHE: LazyLock<CreatorNameCache> = LazyLock::new(|| {
        moka::future::Cache::builder()
            .time_to_live(CREATOR_NAME_TTL)
            .max_capacity(MEMBERSHIP_CACHE_CAPACITY)
            .build()
    });
    &CACHE
}

/// Launch intents already handed out: `(user_id, guild_id, attempt)` → the
/// intent that request collected. See [`launch_replay_cache`].
pub(crate) type LaunchReplayCache =
    moka::future::Cache<(String, String, String), leaf_core::domain::LaunchIntent>;

/// How long a collected launch intent can be read again by a repeat of the
/// same request. Covers the Activity's 10 s read timeout plus its one retry.
const LAUNCH_REPLAY_TTL: Duration = Duration::from_secs(30);

/// Cap on remembered launch-intent answers.
const LAUNCH_REPLAY_CAPACITY: u64 = 1_000;

/// The process-wide memory of launch intents just collected.
///
/// Collecting an intent deletes its row, so an answer lost on the way back
/// (a phone returning from the background on a stale connection) would
/// drop the "Open gallery" press. The Activity repeats its `attempt` id on
/// the retry; this lets that retry read the same intent again. The key
/// includes the signed-in user, so an id guessed by someone else reads
/// nothing. A static for the same reason as [`roles_cache`].
pub(crate) fn launch_replay_cache() -> &'static LaunchReplayCache {
    static CACHE: LazyLock<LaunchReplayCache> = LazyLock::new(|| {
        moka::future::Cache::builder()
            .time_to_live(LAUNCH_REPLAY_TTL)
            .max_capacity(LAUNCH_REPLAY_CAPACITY)
            .build()
    });
    &CACHE
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
    /// Short-TTL cache in front of `guild_channels`, for the creator pickers.
    pub channels: ChannelsCache,
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
            channels: self.channels.clone(),
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

/// The bearer token of a request, if it carries one.
pub(crate) fn bearer(parts: &Parts) -> Option<&str> {
    parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
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
        let Session(claims) = Session::from_request_parts(parts, state).await?;
        Ok(Self {
            user_id: claims.user_id,
        })
    }
}

/// The caller's whole session (who, when they signed in, when the token
/// lapses), for the one route that renews it.
pub struct Session(pub SessionClaims);

impl<S> FromRequestParts<S> for Session
where
    S: Send + Sync,
    SessionKey: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, ApiError> {
        let key = SessionKey::from_ref(state);
        let token = bearer(parts).ok_or(ApiError::Unauthorized)?;
        key.verify_session(token, now_unix())
            .map(Self)
            .map_err(|_| ApiError::Unauthorized)
    }
}

/// A JSON request body, refused in the API's own error shape.
///
/// Like [`axum::Json`], except that a body leaf cannot read is answered as
/// `bad_request` with a sentence instead of axum's plain-text rejection,
/// which a client cannot map to copy.
pub struct ApiJson<T>(pub T);

impl<S, T> FromRequest<S> for ApiJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        match Json::<T>::from_request(req, state).await {
            Ok(Json(value)) => Ok(Self(value)),
            Err(rejection) => {
                tracing::debug!(error = %rejection, "unreadable JSON request body");
                Err(ApiError::BadRequest.with_message(
                    "leaf couldn't read that request. Reload the page and try again.",
                ))
            }
        }
    }
}
