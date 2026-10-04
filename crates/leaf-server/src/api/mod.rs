//! The embedded-app REST API: OAuth token exchange and renewal, guild-scoped
//! series / days / stats reads, creator series management, and the signed
//! media proxy.
//!
//! This is the product's security boundary. Two gates on every data route:
//! a valid session token (`AuthUser`) and guild membership; then
//! `policy::can_view` decides per series. "Not visible" and "doesn't exist"
//! collapse to 404 so the API reveals nothing.
//!
//! Admin-in-gallery (an admin viewing others' private series) is out of
//! scope here — API viewers are treated as non-admin members; moderation
//! stays in the bot. Membership lookups are cached with a short TTL (see
//! [`state::membership_cache`]) so a burst of gallery tiles doesn't fan out
//! into a burst of rate-limited Discord calls.
//!
//! Error bodies are `{error, message?, retryable?}` (see [`error`]): the
//! `message` is a sentence the Activity may show as it is.

pub mod admin;
pub mod auth;
pub mod discord;
pub mod dto;
pub mod error;
pub mod media;
pub mod state;

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Router, response::IntoResponse};
use leaf_core::db::LaunchIntentRepo;
use leaf_core::domain::{Cadence, GuildSettings, Privacy, Series, SeriesState};
use leaf_core::localtime;
use leaf_core::policy::{CreationContext, PolicyViolation, Viewer, can_view};
use leaf_core::series_ops::{
    self, CreateError, CreateSeriesInput, UpdateError, UpdateSeriesInput, ValidationError,
};
use serde::{Deserialize, Serialize};

use auth::{
    DiscordApi, ExchangeError, GuildChannel, GuildMember, GuildRole, LookupError, MediaSigner,
    SESSION_TTL_SECS, now_unix,
};
use dto::{DayDto, DaySummaryDto, GuildView, SeriesDto, SeriesFacts, StatsDto};
use error::ApiError;
use state::{ApiJson, ApiState, AuthUser, Session};

/// Hard cap on an explicitly requested day span (guards a pathological
/// range query). A request with no range gets the whole index.
const MAX_WINDOW: i64 = 366;

/// How long after creating a series a repeat of the same create request is
/// answered with that series instead of "name taken": the first answer was
/// probably lost on the way back.
const CREATE_REPEAT_SECS: i64 = 300;

/// Builds the API router over `state`.
pub fn router<D: DiscordApi>(state: ApiState<D>) -> Router {
    Router::new()
        .route("/api/token", post(token::<D>))
        .route("/api/token/refresh", post(refresh_token::<D>))
        .route("/api/guilds/{gid}/launch-intent", get(launch_intent::<D>))
        .route(
            "/api/guilds/{gid}/series",
            get(list_series::<D>).post(create_series::<D>),
        )
        .route(
            "/api/guilds/{gid}/series/eligibility",
            get(series_eligibility::<D>),
        )
        .route("/api/guilds/{gid}/series/options", get(series_options::<D>))
        .route("/api/guilds/{gid}/series/mine", get(list_my_series::<D>))
        .route("/api/guilds/{gid}/series/{sid}/days", get(list_days::<D>))
        .route(
            "/api/guilds/{gid}/series/{sid}/days/{day}",
            get(get_day::<D>),
        )
        .route("/api/guilds/{gid}/series/{sid}/stats", get(get_stats::<D>))
        .route(
            "/api/guilds/{gid}/series/{sid}/settings",
            get(get_series_settings::<D>),
        )
        .route(
            "/api/guilds/{gid}/series/{sid}",
            patch(update_series_route::<D>),
        )
        .route("/api/media/{attachment_id}", get(media::media::<D>))
        .merge(admin::router::<D>())
        .with_state(state)
}

#[derive(Deserialize)]
struct TokenRequest {
    code: String,
    redirect_uri: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    /// Our HMAC session token; gates every leaf API route.
    token: String,
    /// The Discord OAuth access token, handed back for the embedded-app
    /// `sdk.commands.authenticate({ access_token })` step.
    access_token: String,
    /// Lifetime of `token` in seconds.
    expires_in: i64,
}

/// The answer for an OAuth code Discord would not take. Repeating the
/// request cannot help: the code is spent.
fn code_rejected() -> ApiError {
    ApiError::Coded(StatusCode::BAD_REQUEST, "code_rejected")
        .with_message(
            "Discord didn't accept the sign-in. If this keeps happening, tell a server admin: \
             leaf's Discord credentials or redirect address may be wrong.",
        )
        .retryable(false)
}

/// Maps a failed code exchange to its API answer, logging the cause.
pub(crate) fn exchange_error_to_api(e: &ExchangeError) -> ApiError {
    tracing::warn!(error = %e, "oauth code exchange failed");
    match e {
        ExchangeError::Rejected(_) => code_rejected(),
        ExchangeError::Unavailable(_) => ApiError::discord_unavailable(),
    }
}

/// `POST /api/token` — exchange an OAuth code for a leaf session token.
async fn token<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    ApiJson(req): ApiJson<TokenRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let redirect = req.redirect_uri.unwrap_or_else(|| st.redirect_uri.clone());
    let access = st
        .discord
        .exchange_code(&req.code, &redirect)
        .await
        .map_err(|e| exchange_error_to_api(&e))?;
    let user_id = st.discord.current_user_id(&access).await.map_err(|e| {
        tracing::error!(error = %e, "user lookup failed");
        ApiError::discord_unavailable()
    })?;
    let token = st.key.mint(&user_id, now_unix(), SESSION_TTL_SECS);
    Ok(Json(TokenResponse {
        token,
        access_token: access,
        expires_in: SESSION_TTL_SECS,
    }))
}

#[derive(Serialize)]
struct RefreshResponse {
    /// A fresh session token for the same user.
    token: String,
    /// Lifetime of `token` in seconds.
    expires_in: i64,
}

/// `POST /api/token/refresh` — swap a still-valid session token for a fresh
/// one. The bearer token is the only credential: no body, and never a
/// Discord access token (any app's `identify` token would otherwise mint a
/// leaf session). 401 once the session is past its 7-day cap.
async fn refresh_token<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Session(claims): Session,
) -> Result<Json<RefreshResponse>, ApiError> {
    let (token, expires_in) = st
        .key
        .renew(&claims, now_unix())
        .ok_or(ApiError::Unauthorized)?;
    Ok(Json(RefreshResponse { token, expires_in }))
}

/// Longest guild id accepted from a URL path. A snowflake is at most 20
/// digits.
const MAX_PATH_ID_LEN: usize = 32;

/// Whether `id` from a URL path is safe to look up: a short run of ASCII
/// letters, digits, `-` and `_`, so it cannot add a path segment or a query
/// to a Discord API URL. The exact "is this a snowflake" check is made where
/// those URLs are built (see [`discord`]).
fn is_plain_id(id: &str) -> bool {
    (1..=MAX_PATH_ID_LEN).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Resolves the caller's membership in a guild (roles + join time), or
/// `Forbidden` if not a member. The single membership gate for the
/// guild-scoped routes.
async fn member<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    user_id: &str,
) -> Result<GuildMember, ApiError> {
    // The guild id comes from the URL path, percent-decoded, and ends up in
    // a Discord API URL and a cache key. Anything that could not be an id
    // is refused before either.
    if !is_plain_id(gid) {
        return Err(ApiError::NotFound);
    }
    let key = (gid.to_owned(), user_id.to_owned());
    if let Some(cached) = st.membership.get(&key).await {
        return cached.ok_or(ApiError::Forbidden);
    }
    // A guild the bot is not in is answered without asking Discord: each
    // such lookup is a refused bot-token call, and anyone signed in can ask
    // for any guild id as often as they like. The bot makes a settings row
    // for every guild it is in (when it joins, and again at every connect),
    // so a guild without one is a guild leaf has never been in; one it has
    // left is remembered for a minute after Discord says so.
    if state::bot_known_absent(gid) || st.guilds.get(gid).await?.is_none() {
        return Err(bot_absent());
    }
    // `try_get_with` single-flights concurrent misses for the same key (the
    // cold gallery burst becomes one Discord call) and never caches the
    // error, so a transient failure is retried on the next request.
    let discord = Arc::clone(&st.discord);
    let (g, u) = key.clone();
    let lookup = st
        .membership
        .try_get_with(key, async move { discord.guild_member(&g, &u).await })
        .await;
    match lookup {
        Ok(Some(m)) => Ok(m),
        Ok(None) => Err(ApiError::Forbidden),
        Err(e) => match &*e {
            LookupError::BotNotInGuild => {
                state::note_bot_absent(gid).await;
                Err(bot_absent())
            }
            LookupError::Unavailable(detail) => {
                tracing::warn!(error = %detail, "guild membership lookup failed");
                Err(ApiError::discord_unavailable())
            }
        },
    }
}

/// The refusal for a guild leaf's bot is not in.
fn bot_absent() -> ApiError {
    ApiError::Forbidden.with_message(
        "leaf's bot isn't in this server, so the gallery can't open here. \
         Ask a server admin to invite it.",
    )
}

/// Just the caller's role ids — the membership gate for read routes.
async fn member_roles<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    user_id: &str,
) -> Result<Vec<String>, ApiError> {
    Ok(member(st, gid, user_id).await?.roles)
}

/// The guild's channels, cached for the pickers and name lookups. `None`
/// when Discord could not supply them: callers degrade (no names) rather
/// than fail.
pub(crate) async fn guild_channels<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
) -> Option<Arc<Vec<GuildChannel>>> {
    let discord = Arc::clone(&st.discord);
    let g = gid.to_owned();
    st.channels
        .try_get_with(g.clone(), async move {
            discord.guild_channels(&g).await.map(Arc::new)
        })
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "guild channel lookup failed"))
        .ok()
}

/// The guild's channels asked of Discord now, replacing the cached copy.
/// When Discord cannot supply them the cached copy (if any) still serves.
pub(crate) async fn fresh_guild_channels<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
) -> Option<Arc<Vec<GuildChannel>>> {
    match st.discord.guild_channels(gid).await {
        Ok(channels) => {
            let channels = Arc::new(channels);
            st.channels
                .insert(gid.to_owned(), Arc::clone(&channels))
                .await;
            Some(channels)
        }
        Err(e) => {
            tracing::warn!(error = %e, "guild channel lookup failed");
            st.channels.get(gid).await
        }
    }
}

/// The guild's channels as Discord lists them now, for naming channels:
/// asked of Discord, and kept only for the few seconds a burst of requests
/// takes (see [`state::channel_names_cache`]). The flag says whether this
/// call is the one that asked. `None` when Discord could not supply the
/// list: there is then no name to give, and no older list is used instead.
async fn current_guild_channels<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
) -> Option<(Arc<Vec<GuildChannel>>, bool)> {
    let discord = Arc::clone(&st.discord);
    let kept = st.channels.clone();
    let g = gid.to_owned();
    state::channel_names_cache()
        .entry(g.clone())
        .or_try_insert_with(async move {
            let list = discord.guild_channels(&g).await.map(Arc::new)?;
            // The longer-lived copy gets the newer list too.
            kept.insert(g, Arc::clone(&list)).await;
            Ok::<_, String>(list)
        })
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "guild channel lookup failed"))
        .ok()
        .map(|entry| {
            let asked = entry.is_fresh();
            (entry.into_value(), asked)
        })
}

/// The guild's channels for naming `ids`: `None` when there is nothing to
/// name or Discord could not supply the list.
///
/// The list that checks ids ([`guild_channels`]) is not good enough for
/// names. A channel deleted since it was cached is still in it, under its
/// old name, and the gallery would go on sending people there. Names come
/// from the list as it is now ([`current_guild_channels`]). When that list
/// is a few seconds old and lacks one of `ids`, the channel may have been
/// made since, so Discord is asked once more (at most once every few
/// seconds per guild).
///
/// Whoever names from the result must treat an id that is not in it as a
/// channel with no name: there is no older name to fall back on.
async fn guild_channels_naming<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    mut ids: impl Iterator<Item = &str> + Clone,
) -> Option<Arc<Vec<GuildChannel>>> {
    ids.clone().next()?;
    let (current, just_asked) = current_guild_channels(st, gid).await?;
    let all_known = ids.all(|id| current.iter().any(|channel| channel.id == id));
    if just_asked || all_known || !state::refresh_allowed(state::GuildList::Channels, gid).await {
        return Some(current);
    }
    match st.discord.guild_channels(gid).await {
        Ok(list) => {
            let list = Arc::new(list);
            state::channel_names_cache()
                .insert(gid.to_owned(), Arc::clone(&list))
                .await;
            st.channels.insert(gid.to_owned(), Arc::clone(&list)).await;
            Some(list)
        }
        Err(e) => {
            // The list of a moment ago still says what is there.
            tracing::warn!(error = %e, "guild channel lookup failed");
            Some(current)
        }
    }
}

/// How Discord's current channel list (see [`guild_channels_naming`]) has a
/// stored channel.
struct NamedChannel {
    /// Its name; `None` when the list does not have it, or there is no
    /// list.
    name: Option<String>,
    /// The list was obtained and the channel is not in it: deleted, or
    /// hidden from leaf. False when Discord could not be asked, which says
    /// nothing about the channel.
    missing: bool,
}

impl NamedChannel {
    fn of(id: &str, current: Option<&[GuildChannel]>) -> Self {
        let name = current
            .and_then(|list| list.iter().find(|channel| channel.id == id))
            .map(|channel| channel.name.clone());
        Self {
            missing: current.is_some() && name.is_none(),
            name,
        }
    }
}

/// Discord's own order for roles: highest position first, then by id.
fn sort_roles(roles: &mut [GuildRole]) {
    roles.sort_by(|a, b| b.position.cmp(&a.position).then_with(|| a.id.cmp(&b.id)));
}

/// The guild's assignable roles, highest first, cached. `None` when Discord
/// could not supply them.
pub(crate) async fn guild_roles<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
) -> Option<Arc<Vec<GuildRole>>> {
    let discord = Arc::clone(&st.discord);
    let g = gid.to_owned();
    state::roles_cache()
        .try_get_with(g.clone(), async move {
            discord.guild_roles(&g).await.map(|mut roles| {
                sort_roles(&mut roles);
                Arc::new(roles)
            })
        })
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "guild role lookup failed"))
        .ok()
}

/// The guild's roles asked of Discord now, replacing the cached copy. When
/// Discord cannot supply them the cached copy (if any) still serves.
pub(crate) async fn fresh_guild_roles<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
) -> Option<Arc<Vec<GuildRole>>> {
    match st.discord.guild_roles(gid).await {
        Ok(mut roles) => {
            sort_roles(&mut roles);
            let roles = Arc::new(roles);
            state::roles_cache()
                .insert(gid.to_owned(), Arc::clone(&roles))
                .await;
            Some(roles)
        }
        Err(e) => {
            tracing::warn!(error = %e, "guild role lookup failed");
            state::roles_cache().get(gid).await
        }
    }
}

/// Checks that `role_id` is a role of this guild that members can hold.
/// When Discord cannot be asked the change is refused (retryable) rather
/// than stored unchecked.
pub(crate) async fn require_role<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    role_id: &str,
) -> Result<(), ApiError> {
    let roles = guild_roles(st, gid)
        .await
        .ok_or_else(ApiError::discord_unavailable)?;
    if roles.iter().any(|r| r.id == role_id) {
        return Ok(());
    }
    // The cached list may be older than the role: ask once more before
    // refusing a role that was only just made.
    if state::refresh_allowed(state::GuildList::Roles, gid).await
        && let Some(roles) = fresh_guild_roles(st, gid).await
        && roles.iter().any(|r| r.id == role_id)
    {
        return Ok(());
    }
    Err(ApiError::unprocessable(
        "unknown_role",
        "That role isn't one members of this server can hold. Pick a role from the list.",
    ))
}

/// The guild-wide values a series entry repeats. A guild without a settings
/// row (never set up) reads as the defaults.
fn guild_view(settings: Option<&GuildSettings>) -> GuildView {
    settings.map_or_else(
        || GuildView {
            tz: localtime::Tz::UTC,
            sprout_threshold: GuildSettings::defaults_for("").sprout_threshold,
        },
        |s| GuildView {
            tz: localtime::tz_or_utc(&s.timezone),
            sprout_threshold: s.sprout_threshold,
        },
    )
}

/// A series as the list shows it to `viewer_id`, with its cheap aggregates.
async fn series_dto<D: DiscordApi>(
    st: &ApiState<D>,
    s: &Series,
    viewer_id: &str,
    view: &GuildView,
) -> Result<SeriesDto, ApiError> {
    let facts = SeriesFacts {
        max_day: st.posts.max_day(s.id).await?,
        total_days: st.posts.count(s.id).await?,
        last_posted_at: st.posts.latest_posted_at(s.id).await?,
    };
    Ok(SeriesDto::build(s, viewer_id, facts, view))
}

/// Membership + load + visibility, returning a viewable series. Anything
/// the caller may not see is `NotFound` (indistinguishable from absent).
async fn resolve_viewable<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    sid: i64,
    user_id: &str,
) -> Result<Series, ApiError> {
    let roles = member_roles(st, gid, user_id).await?;
    let series = st.series.get(sid).await?.ok_or(ApiError::NotFound)?;
    let viewer = Viewer {
        user_id,
        role_ids: &roles,
        is_admin: false,
    };
    if series.guild_id != gid || !can_view(&series, &viewer) {
        return Err(ApiError::NotFound);
    }
    Ok(series)
}

/// `GET /api/guilds/{gid}/series` — series in the guild visible to the
/// caller, plus the caller's own revoked series (listed so their creator can
/// see what happened; their days and stats stay 404).
async fn list_series<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    user: AuthUser,
) -> Result<Json<Vec<SeriesDto>>, ApiError> {
    let roles = member_roles(&st, &gid, &user.user_id).await?;
    let viewer = Viewer {
        user_id: &user.user_id,
        role_ids: &roles,
        is_admin: false,
    };
    let settings = st.guilds.get(&gid).await?;
    let view = guild_view(settings.as_ref());

    let mut out = Vec::new();
    for s in st.series.list_by_guild(&gid).await? {
        let own_revoked = s.state == SeriesState::Revoked && s.creator_id == user.user_id;
        if can_view(&s, &viewer) || own_revoked {
            out.push(series_dto(&st, &s, &user.user_id, &view).await?);
        }
    }
    Ok(Json(out))
}

#[derive(Serialize)]
struct LaunchIntentDto {
    series_id: i64,
    day: Option<i64>,
}

#[derive(Deserialize)]
struct AttemptQuery {
    /// The caller's id for this logical read, repeated on its retry.
    attempt: Option<String>,
}

/// Longest `attempt` id that is remembered.
const MAX_ATTEMPT_ID_LEN: usize = 32;

/// The `attempt` id when it is one worth remembering: 1 to 32 lowercase
/// letters and digits (what the Activity sends). Anything else is ignored,
/// and the read behaves as if no id was sent.
fn usable_attempt(attempt: Option<String>) -> Option<String> {
    attempt.filter(|a| {
        (1..=MAX_ATTEMPT_ID_LEN).contains(&a.len())
            && a.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    })
}

/// `GET /api/guilds/{gid}/launch-intent[?attempt=<id>]` — where a button in
/// chat asked the gallery to open, or JSON `null`. Reading consumes the
/// intent; a repeat of the same request (same `attempt` id, same user, within
/// about half a minute) reads it again, so an answer lost on the way back
/// does not drop the press. One that names a series the caller cannot view
/// (deleted, another guild's, made private since the press) is dropped:
/// `null`, never a hint it existed.
async fn launch_intent<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    Query(query): Query<AttemptQuery>,
    user: AuthUser,
) -> Result<Json<Option<LaunchIntentDto>>, ApiError> {
    let roles = member_roles(&st, &gid, &user.user_id).await?;
    let replay_key = usable_attempt(query.attempt).map(|a| (user.user_id.clone(), gid.clone(), a));
    let intents = LaunchIntentRepo::new(st.series.pool().clone());
    // A newer press always wins: only when no row is waiting does a repeat
    // read what its first try collected.
    let intent = match intents.take(&user.user_id, &gid, now_unix()).await? {
        Some(taken) => {
            // Remembered before the checks below, so a retry after one of
            // them failed still has the intent to check again.
            if let Some(key) = replay_key {
                state::launch_replay_cache().insert(key, taken).await;
            }
            Some(taken)
        }
        None => match replay_key {
            Some(key) => state::launch_replay_cache().get(&key).await,
            None => None,
        },
    };
    let Some(intent) = intent else {
        return Ok(Json(None));
    };
    let viewer = Viewer {
        user_id: &user.user_id,
        role_ids: &roles,
        is_admin: false,
    };
    let viewable = st
        .series
        .get(intent.series_id)
        .await?
        .is_some_and(|s| s.guild_id == gid && can_view(&s, &viewer));
    Ok(Json(viewable.then_some(LaunchIntentDto {
        series_id: intent.series_id,
        day: intent.day,
    })))
}

#[derive(Deserialize)]
struct DayRange {
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/guilds/{gid}/series/{sid}/days[?from&to]` — the day index
/// (signed thumbnails, calendar dates in the guild's timezone), ascending.
/// With no range it is the whole series in one answer; `[]` means the
/// series has no days.
async fn list_days<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    Query(range): Query<DayRange>,
    user: AuthUser,
) -> Result<Json<Vec<DaySummaryDto>>, ApiError> {
    let series = resolve_viewable(&st, &gid, sid, &user.user_id).await?;
    let (from, to) = (range.from.unwrap_or(i64::MIN), range.to.unwrap_or(i64::MAX));
    if from > to {
        return Err(ApiError::BadRequest);
    }
    if let (Some(from), Some(to)) = (range.from, range.to)
        && to.saturating_sub(from) > MAX_WINDOW
    {
        return Err(ApiError::BadRequest);
    }

    let settings = st.guilds.get(&gid).await?;
    let tz = guild_view(settings.as_ref()).tz;
    let signer = MediaSigner::new(&st.key, now_unix());
    let out = st
        .posts
        .day_summaries(series.id)
        .await?
        .iter()
        .filter(|row| (from..=to).contains(&row.day))
        .map(|row| DaySummaryDto::build(row, tz, &signer))
        .collect();
    Ok(Json(out))
}

/// `GET /api/guilds/{gid}/series/{sid}/days/{day}` — one day in full.
async fn get_day<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid, day)): Path<(String, i64, i64)>,
    user: AuthUser,
) -> Result<Json<DayDto>, ApiError> {
    let series = resolve_viewable(&st, &gid, sid, &user.user_id).await?;
    let (post, media) = st
        .posts
        .get(series.id, day)
        .await?
        .ok_or(ApiError::NotFound)?;
    let signer = MediaSigner::new(&st.key, now_unix());
    Ok(Json(DayDto::build(&gid, &post, &media, &signer)))
}

/// `GET /api/guilds/{gid}/series/{sid}/stats` — streak/coverage stats.
async fn get_stats<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    user: AuthUser,
) -> Result<Json<StatsDto>, ApiError> {
    let series = resolve_viewable(&st, &gid, sid, &user.user_id).await?;
    let days = st.posts.all_days(series.id).await?;
    Ok(Json(
        leaf_core::stats::compute(&days, series.start_day).into(),
    ))
}

// --- creator series management -------------------------------------------
//
// Unlike the read routes (membership + `can_view`), these are owner-scoped
// mutations and an eligibility check. Every one re-runs the same
// `series_ops`/`policy` rules the bot runs — hiding a button is not a gate.

/// The numbers behind a violation, so the client can word it precisely
/// ("you can start a series on 14 Oct"). Which keys are set depends on the
/// rule.
#[derive(Serialize, Default)]
struct ViolationParams {
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    current: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    days: Option<i64>,
    /// Unix seconds at which an age rule stops applying.
    #[serde(skip_serializing_if = "Option::is_none")]
    eligible_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role_name: Option<String>,
}

/// A single eligibility/creation blocker with a stable code and a message.
#[derive(Serialize)]
struct ViolationDto {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<ViolationParams>,
}

/// Whether the caller may start a series here, and why not if they can't.
#[derive(Serialize)]
struct EligibilityDto {
    can_create: bool,
    violations: Vec<ViolationDto>,
    /// Whether the caller created any series here, revoked ones included.
    owns_any: bool,
}

/// What a guild that has not run `/setup` is told, on every creator route.
const NOT_SET_UP: &str =
    "leaf isn't set up in this server yet. A server admin needs to run /setup first.";

fn guild_not_setup() -> ApiError {
    ApiError::Coded(StatusCode::FORBIDDEN, "guild_not_setup").with_message(NOT_SET_UP)
}

/// Stable machine code for a creation-policy violation (matches the design's
/// eligibility/create error codes).
const fn policy_code(v: &PolicyViolation) -> &'static str {
    match v {
        PolicyViolation::MaxSeries(_) => "max_series",
        PolicyViolation::AccountTooNew(_) => "account_too_new",
        PolicyViolation::MembershipTooNew(_) => "membership_too_new",
        PolicyViolation::MissingCreatorRole => "missing_creator_role",
        PolicyViolation::ChannelNotWatched => "invalid_channel",
    }
}

/// Stable machine code for a field validation error.
const fn validation_code(e: &ValidationError) -> &'static str {
    match e {
        ValidationError::NameLength => "invalid_name",
        ValidationError::DescriptionTooLong => "invalid_description",
        ValidationError::EmojiTooLong => "invalid_emoji",
        ValidationError::StartDayTooLow => "invalid_start_day",
        ValidationError::MissingPrivacyRole => "missing_privacy_role",
        ValidationError::InvalidReminderTime(_) => "invalid_reminder_time",
        ValidationError::InvalidTimezone(_) => "invalid_timezone",
        ValidationError::ReminderTimeRequired => "reminder_time_required",
        ValidationError::ReminderOnFreeform => "reminder_on_freeform",
    }
}

/// A policy refusal as an API error: its code, and the rule's own sentence.
fn policy_error(v: &PolicyViolation) -> ApiError {
    let status = if matches!(v, PolicyViolation::ChannelNotWatched) {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::FORBIDDEN
    };
    ApiError::Coded(status, policy_code(v)).with_message(v.to_string())
}

/// A field refusal as an API error: its code, and the rule's own sentence.
fn validation_error(v: &ValidationError) -> ApiError {
    ApiError::Coded(StatusCode::BAD_REQUEST, validation_code(v)).with_message(v.to_string())
}

// Names are unique per server (`UNIQUE (guild_id, name)`), so this answer
// also tells a creator that a series they cannot see holds the exact name
// they asked for. Nothing else about that series is given away. Known and
// accepted as the cost of server-wide names; making names unique per
// creator instead is a schema and product decision.
fn name_taken(message: String) -> ApiError {
    ApiError::Coded(StatusCode::CONFLICT, "name_taken").with_message(message)
}

fn create_error_to_api(e: CreateError) -> ApiError {
    match e {
        CreateError::Policy(v) => policy_error(&v),
        CreateError::Validation(v) => validation_error(&v),
        CreateError::NameTaken => name_taken(CreateError::NameTaken.to_string()),
        CreateError::Db(e) => e.into(),
    }
}

fn update_error_to_api(e: UpdateError) -> ApiError {
    match e {
        // Hide the series from non-owners (indistinguishable from missing).
        UpdateError::Forbidden(_) => ApiError::NotFound,
        UpdateError::Policy(v) => policy_error(&v),
        UpdateError::Validation(v) => validation_error(&v),
        UpdateError::Revoked => ApiError::Coded(StatusCode::FORBIDDEN, "revoked")
            .with_message(UpdateError::Revoked.to_string()),
        UpdateError::NameTaken => name_taken(UpdateError::NameTaken.to_string()),
        UpdateError::Db(e) => e.into(),
    }
}

/// Loads the guild's settings only when `/setup` is complete.
async fn setup_settings<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
) -> Result<Option<GuildSettings>, ApiError> {
    Ok(st.guilds.get(gid).await?.filter(|s| s.setup_complete))
}

/// The policy facts about `user_id` as a would-be creator in `gid`.
async fn creation_context<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    user_id: &str,
    m: &GuildMember,
    settings: &GuildSettings,
) -> Result<CreationContext, ApiError> {
    let live = st.series.count_live_by_creator(gid, user_id).await?;
    Ok(series_ops::build_creation_context(
        now_unix(),
        series_ops::account_created_unix(user_id),
        m.joined_at,
        live,
        &m.roles,
        settings,
    ))
}

/// One violation as the client reads it: code, sentence and numbers.
fn violation_dto(
    v: &PolicyViolation,
    ctx: &CreationContext,
    creator_role_name: Option<&str>,
) -> ViolationDto {
    let mut params = ViolationParams {
        eligible_at: v.eligible_at(ctx),
        ..ViolationParams::default()
    };
    match *v {
        PolicyViolation::MaxSeries(limit) => {
            params.limit = Some(limit);
            params.current = Some(ctx.live_series_count);
        }
        PolicyViolation::AccountTooNew(days) | PolicyViolation::MembershipTooNew(days) => {
            params.days = Some(days);
        }
        PolicyViolation::MissingCreatorRole => {
            params.role_name = creator_role_name.map(ToOwned::to_owned);
        }
        PolicyViolation::ChannelNotWatched => {}
    }
    ViolationDto {
        code: policy_code(v),
        message: v.to_string(),
        params: Some(params),
    }
}

/// `GET /api/guilds/{gid}/series/eligibility` — may this user start a
/// series? Lists every rule in the way, not just the first.
async fn series_eligibility<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    user: AuthUser,
) -> Result<Json<EligibilityDto>, ApiError> {
    let m = member(&st, &gid, &user.user_id).await?;
    let owns_any = !st
        .series
        .list_by_creator(&gid, &user.user_id)
        .await?
        .is_empty();
    let Some(settings) = setup_settings(&st, &gid).await? else {
        return Ok(Json(EligibilityDto {
            can_create: false,
            violations: vec![ViolationDto {
                code: "guild_not_setup",
                message: NOT_SET_UP.to_owned(),
                params: None,
            }],
            owns_any,
        }));
    };
    let ctx = creation_context(&st, &gid, &user.user_id, &m, &settings).await?;
    let found = leaf_core::policy::creation_violations(&settings, &ctx);

    // The creator role's name is only worth a Discord call when it is the
    // thing in the way.
    let role_name = if found.contains(&PolicyViolation::MissingCreatorRole)
        && let Some(role_id) = &settings.creator_role_id
        && let Some(roles) = guild_roles(&st, &gid).await
    {
        roles
            .iter()
            .find(|r| &r.id == role_id)
            .map(|r| r.name.clone())
    } else {
        None
    };

    let violations: Vec<ViolationDto> = found
        .iter()
        .map(|v| violation_dto(v, &ctx, role_name.as_deref()))
        .collect();
    Ok(Json(EligibilityDto {
        can_create: violations.is_empty(),
        violations,
        owns_any,
    }))
}

/// A channel a series may post in; `name` is `null` when Discord no longer
/// lists it (deleted, or hidden from leaf) or could not be asked.
#[derive(Serialize)]
struct ChannelOptionDto {
    id: String,
    name: Option<String>,
}

/// A role a series may be gated on.
#[derive(Serialize)]
struct RoleOptionDto {
    id: String,
    name: String,
    /// Whether the caller holds the role.
    held: bool,
}

/// Form metadata for the create form and settings screen.
#[derive(Serialize)]
struct OptionsDto {
    channels: Vec<ChannelOptionDto>,
    /// Highest role first, as Discord lists them.
    roles: Vec<RoleOptionDto>,
    cadences: Vec<&'static str>,
    privacy_modes: Vec<&'static str>,
    guild_timezone: String,
    sprout_enabled: bool,
    sprout_threshold: i64,
    /// True when Discord could not supply the role list (`roles` is empty).
    roles_unavailable: bool,
}

/// `GET /api/guilds/{gid}/series/options` — channels, roles, and enum
/// choices. A Discord hiccup costs names or the role list, not the form.
async fn series_options<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    user: AuthUser,
) -> Result<Json<OptionsDto>, ApiError> {
    let m = member(&st, &gid, &user.user_id).await?;
    let settings = setup_settings(&st, &gid)
        .await?
        .ok_or_else(guild_not_setup)?;

    // Series channels, labelled by Discord's current names where available.
    let watched = settings.watched_channels.iter().map(String::as_str);
    let current = guild_channels_naming(&st, &gid, watched).await;
    let current = current.as_ref().map(|list| list.as_slice());
    let channels = settings
        .watched_channels
        .iter()
        .map(|id| ChannelOptionDto {
            id: id.clone(),
            name: NamedChannel::of(id, current).name,
        })
        .collect();

    let role_list = guild_roles(&st, &gid).await;
    let roles_unavailable = role_list.is_none();
    let roles = role_list
        .iter()
        .flat_map(|list| list.iter())
        .map(|r| RoleOptionDto {
            id: r.id.clone(),
            name: r.name.clone(),
            held: m.roles.contains(&r.id),
        })
        .collect();

    Ok(Json(OptionsDto {
        channels,
        roles,
        cadences: vec![
            Cadence::Daily.as_str(),
            Cadence::Weekdays.as_str(),
            Cadence::Weekly.as_str(),
            Cadence::Freeform.as_str(),
        ],
        privacy_modes: vec![
            Privacy::Public.as_str(),
            Privacy::RoleGated.as_str(),
            Privacy::CreatorOnly.as_str(),
        ],
        guild_timezone: settings.timezone,
        sprout_enabled: settings.sprout_enabled,
        sprout_threshold: settings.sprout_threshold,
        roles_unavailable,
    }))
}

const fn default_start_day() -> i64 {
    1
}

#[derive(Deserialize)]
struct CreateSeriesRequest {
    name: String,
    #[serde(default)]
    description: String,
    channel_id: String,
    cadence: String,
    privacy: String,
    #[serde(default)]
    privacy_role_id: Option<String>,
    #[serde(default = "default_start_day")]
    start_day: i64,
}

/// The series a repeated create request is really about: one this caller
/// made under the same name moments ago. The first answer was lost on the
/// way back, so the repeat succeeds with it instead of "name taken".
async fn recent_duplicate<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    user_id: &str,
    name: &str,
) -> Result<Option<Series>, ApiError> {
    Ok(st.series.get_by_name(gid, name.trim()).await?.filter(|s| {
        s.creator_id == user_id
            && s.state != SeriesState::Revoked
            && now_unix().saturating_sub(s.created_at) <= CREATE_REPEAT_SECS
    }))
}

/// `POST /api/guilds/{gid}/series` — create a series (re-runs policy).
/// 201 with the series; 200 with the existing one when the same creator
/// repeats a create inside five minutes.
async fn create_series<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    user: AuthUser,
    ApiJson(req): ApiJson<CreateSeriesRequest>,
) -> Result<(StatusCode, Json<SeriesDto>), ApiError> {
    let m = member(&st, &gid, &user.user_id).await?;
    let settings = setup_settings(&st, &gid)
        .await?
        .ok_or_else(guild_not_setup)?;
    let view = guild_view(Some(&settings));

    // Before policy runs: the series being repeated may itself have used up
    // the caller's last slot.
    if let Some(existing) = recent_duplicate(&st, &gid, &user.user_id, &req.name).await? {
        let dto = series_dto(&st, &existing, &user.user_id, &view).await?;
        return Ok((StatusCode::OK, Json(dto)));
    }

    let cadence = req
        .cadence
        .parse::<Cadence>()
        .map_err(|_| ApiError::BadRequest)?;
    let privacy = req
        .privacy
        .parse::<Privacy>()
        .map_err(|_| ApiError::BadRequest)?;
    let input = CreateSeriesInput {
        name: req.name,
        description: req.description,
        channel_id: req.channel_id,
        cadence,
        privacy,
        // A role only means something on a role-gated series. Stored with
        // any other privacy it would sit there unchecked and take effect on
        // a later switch to role-gated.
        privacy_role_id: req
            .privacy_role_id
            .filter(|s| !s.is_empty() && privacy == Privacy::RoleGated),
        start_day: req.start_day,
    };

    if input.privacy == Privacy::RoleGated
        && let Some(role) = &input.privacy_role_id
    {
        require_role(&st, &gid, role).await?;
    }

    let ctx = creation_context(&st, &gid, &user.user_id, &m, &settings).await?;
    let created = series_ops::create_series(
        &st.series,
        &settings,
        &ctx,
        &user.user_id,
        &input,
        now_unix(),
    )
    .await
    .map_err(create_error_to_api)?;

    let dto = series_dto(&st, &created, &user.user_id, &view).await?;
    Ok((StatusCode::CREATED, Json(dto)))
}

/// A series the caller owns, for the "my series" dashboard.
#[derive(Serialize)]
struct MySeriesDto {
    id: i64,
    name: String,
    emoji: String,
    state: String,
    cadence: String,
    channel_id: Option<String>,
    /// The channel's name as Discord has it now; `null` when Discord no
    /// longer lists the channel or could not be asked. Never a name from
    /// before the channel was deleted.
    channel_name: Option<String>,
    /// True when Discord was asked and no longer lists the series' channel
    /// (deleted, or hidden from leaf): nothing can be posted there until
    /// the creator picks another. False when there is no channel, and when
    /// Discord could not be asked.
    channel_missing: bool,
    archived_days: i64,
    reminder_enabled: bool,
    /// Why the last reminder could not be delivered (`dm_closed`,
    /// `channel_missing`, `no_permission`); `null` while delivery works.
    reminder_error: Option<String>,
}

/// `GET /api/guilds/{gid}/series/mine` — the caller's own series.
async fn list_my_series<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    user: AuthUser,
) -> Result<Json<Vec<MySeriesDto>>, ApiError> {
    member(&st, &gid, &user.user_id).await?;
    let mine = st.series.list_by_creator(&gid, &user.user_id).await?;

    // Channel names are best-effort: a Discord hiccup leaves them unlabelled
    // rather than failing the whole dashboard.
    let channel_ids = mine
        .iter()
        .filter_map(|s| s.channels.first().map(String::as_str));
    let current = guild_channels_naming(&st, &gid, channel_ids).await;
    let current = current.as_ref().map(|list| list.as_slice());

    let mut out = Vec::with_capacity(mine.len());
    for s in mine {
        let archived_days = st.posts.count(s.id).await?;
        let reminder_error = st.series.reminder_error(s.id).await?.map(|f| f.reason);
        let channel_id = s.channels.first().cloned();
        let channel = channel_id
            .as_deref()
            .map(|id| NamedChannel::of(id, current));
        out.push(MySeriesDto {
            id: s.id,
            name: s.name,
            emoji: s.emoji,
            state: s.state.as_str().to_owned(),
            cadence: s.cadence.as_str().to_owned(),
            channel_id,
            channel_missing: channel.as_ref().is_some_and(|c| c.missing),
            channel_name: channel.and_then(|c| c.name),
            archived_days,
            reminder_enabled: s.reminder_enabled,
            reminder_error,
        });
    }
    Ok(Json(out))
}

/// Every editable field of a series, for the settings screen.
#[derive(Serialize)]
struct SeriesSettingsDto {
    id: i64,
    name: String,
    description: String,
    emoji: String,
    cadence: String,
    privacy: String,
    privacy_role_id: Option<String>,
    channel_id: Option<String>,
    /// Always `context_menu` in practice: passive capture is not built, and
    /// the API no longer lets it be switched on.
    detection_mode: String,
    state: String,
    reminder_enabled: bool,
    reminder_time: Option<String>,
    reminder_timezone: Option<String>,
    reminder_dm: bool,
    start_day: i64,
    /// Why the last reminder could not be delivered, passed through as the
    /// scheduler recorded it; `null` while delivery works.
    reminder_error: Option<String>,
    /// When that failure was recorded, unix seconds.
    reminder_error_at: Option<i64>,
}

impl SeriesSettingsDto {
    async fn load<D: DiscordApi>(st: &ApiState<D>, s: &Series) -> Result<Self, ApiError> {
        let failure = st.series.reminder_error(s.id).await?;
        Ok(Self {
            id: s.id,
            name: s.name.clone(),
            description: s.description.clone(),
            emoji: s.emoji.clone(),
            cadence: s.cadence.as_str().to_owned(),
            privacy: s.privacy.as_str().to_owned(),
            privacy_role_id: s.privacy_role_id.clone(),
            channel_id: s.channels.first().cloned(),
            detection_mode: s.detection_mode.as_str().to_owned(),
            state: s.state.as_str().to_owned(),
            reminder_enabled: s.reminder_enabled,
            reminder_time: s.reminder_time.clone(),
            reminder_timezone: s.reminder_timezone.clone(),
            reminder_dm: s.reminder_dm,
            start_day: s.start_day,
            reminder_error_at: failure.as_ref().map(|f| f.at),
            reminder_error: failure.map(|f| f.reason),
        })
    }
}

/// Loads a series in `gid` that `user_id` created; anything else is 404, so
/// a non-owner cannot tell a series exists.
async fn owned_series<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    sid: i64,
    user_id: &str,
) -> Result<Series, ApiError> {
    let series = st.series.get(sid).await?.ok_or(ApiError::NotFound)?;
    if series.guild_id != gid {
        return Err(ApiError::NotFound);
    }
    series_ops::assert_owner(&series, user_id).map_err(|_| ApiError::NotFound)?;
    Ok(series)
}

/// `GET /api/guilds/{gid}/series/{sid}/settings` — full editable fields
/// (owner only; hidden as 404 from everyone else).
async fn get_series_settings<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    user: AuthUser,
) -> Result<Json<SeriesSettingsDto>, ApiError> {
    member(&st, &gid, &user.user_id).await?;
    let series = owned_series(&st, &gid, sid, &user.user_id).await?;
    Ok(Json(SeriesSettingsDto::load(&st, &series).await?))
}

/// The PATCH body. Unknown keys are ignored, which is how a client that
/// still sends `detection_mode` (passive capture, never built) is handled.
#[derive(Deserialize)]
struct UpdateSeriesRequest {
    name: Option<String>,
    start_day: Option<i64>,
    description: Option<String>,
    emoji: Option<String>,
    cadence: Option<String>,
    privacy: Option<String>,
    privacy_role_id: Option<String>,
    channel_id: Option<String>,
    reminder_enabled: Option<bool>,
    reminder_time: Option<String>,
    /// An empty string clears the override (the server's zone applies).
    reminder_timezone: Option<String>,
    reminder_dm: Option<bool>,
}

/// Applies a rename and a new first day number to the loaded series, each
/// only when it differs from what is stored. The write itself (and the
/// duplicate-name answer) is `update_series`'.
async fn apply_identity<D: DiscordApi>(
    st: &ApiState<D>,
    series: &mut Series,
    name: Option<&str>,
    start_day: Option<i64>,
) -> Result<(), ApiError> {
    if let Some(name) = name
        && name.trim() != series.name
    {
        if let Some(problem) = series_ops::name_problem(name) {
            return Err(ApiError::Coded(StatusCode::BAD_REQUEST, "invalid_name")
                .with_message(problem.to_string()));
        }
        name.trim().clone_into(&mut series.name);
    }
    if let Some(start_day) = start_day
        && start_day != series.start_day
    {
        let earliest = st.posts.min_day(series.id).await?;
        if let Some(problem) = series_ops::start_day_problem(start_day, earliest) {
            return Err(
                ApiError::Coded(StatusCode::BAD_REQUEST, "invalid_start_day")
                    .with_message(problem.to_string()),
            );
        }
        series.start_day = start_day;
    }
    Ok(())
}

/// `PATCH /api/guilds/{gid}/series/{sid}` — owner-only partial update.
async fn update_series_route<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    user: AuthUser,
    ApiJson(req): ApiJson<UpdateSeriesRequest>,
) -> Result<Json<SeriesSettingsDto>, ApiError> {
    member(&st, &gid, &user.user_id).await?;
    let settings = setup_settings(&st, &gid)
        .await?
        .ok_or_else(guild_not_setup)?;
    // Owner and revoked are checked before any field, so a refusal about a
    // value never tells a non-owner the series exists.
    let mut series = owned_series(&st, &gid, sid, &user.user_id).await?;
    if series.state == SeriesState::Revoked {
        return Err(update_error_to_api(UpdateError::Revoked));
    }

    let cadence = req
        .cadence
        .as_deref()
        .map(str::parse::<Cadence>)
        .transpose()
        .map_err(|_| ApiError::BadRequest)?;
    let privacy = req
        .privacy
        .as_deref()
        .map(str::parse::<Privacy>)
        .transpose()
        .map_err(|_| ApiError::BadRequest)?;

    apply_identity(&st, &mut series, req.name.as_deref(), req.start_day).await?;
    let new_role = req.privacy_role_id.filter(|s| !s.is_empty());
    // The role is checked against this server when it changes, and when the
    // series becomes role-gated on a role stored earlier (a stored role was
    // not always checked).
    let role_changes = new_role
        .as_ref()
        .is_some_and(|role| series.privacy_role_id.as_ref() != Some(role));
    let becomes_gated = privacy == Some(Privacy::RoleGated) && series.privacy != Privacy::RoleGated;
    if (role_changes || becomes_gated)
        && let Some(role) = new_role.as_ref().or(series.privacy_role_id.as_ref())
    {
        require_role(&st, &gid, role).await?;
    }

    let input = UpdateSeriesInput {
        description: req.description,
        emoji: req.emoji,
        cadence,
        privacy,
        privacy_role_id: new_role,
        channel_id: req.channel_id,
        detection_mode: None,
        reminder_enabled: req.reminder_enabled,
        reminder_time: req.reminder_time,
        reminder_timezone: req.reminder_timezone,
        reminder_dm: req.reminder_dm,
    };

    let updated = series_ops::update_series(&st.series, &settings, series, &user.user_id, &input)
        .await
        .map_err(update_error_to_api)?;
    Ok(Json(SeriesSettingsDto::load(&st, &updated).await?))
}

/// Maps a thrown `ApiError` into a response (so handlers can `?`).
impl From<ApiError> for axum::response::Response {
    fn from(e: ApiError) -> Self {
        e.into_response()
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; JSON indexing is fine in assertions"
    )]

    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo, SqlitePool};
    use leaf_core::domain::{
        DetectionMode, NewMediaAttachment, NewSeries, Post, ReminderFailureKind,
    };
    use object_store::ObjectStore;
    use object_store::memory::InMemory;
    use tower::ServiceExt as _;

    use super::*;
    use crate::api::auth::{SESSION_MAX_AGE_SECS, SessionKey};

    /// Mock Discord: fixed membership + managed-guild maps, never hits the
    /// network. The test "code"/access-token encodes the user id.
    struct MockDiscord {
        /// (guild, user) → roles. Absent = not a member.
        members: std::collections::HashMap<(String, String), Vec<String>>,
        /// user → guild ids they manage (Manage-Guild).
        manages: std::collections::HashMap<String, Vec<String>>,
        /// Counts `guild_member` calls, so a test can prove caching.
        calls: Arc<AtomicUsize>,
        /// Every `(channel, content)` sent as a log line.
        sent: Arc<Mutex<Vec<(String, String)>>>,
        /// Roles and channels "made in Discord" during a test: listed after
        /// the fixed ones from the next lookup on.
        extra_roles: Arc<Mutex<Vec<auth::GuildRole>>>,
        extra_channels: Arc<Mutex<Vec<auth::GuildChannel>>>,
        /// Channels "deleted in Discord" during a test: no longer listed.
        gone_channels: Arc<Mutex<Vec<String>>>,
        /// While set, the channel list cannot be had.
        channels_down: Arc<AtomicBool>,
        /// Counts `guild_channels` calls.
        channel_lists: Arc<AtomicUsize>,
    }

    /// A member whose lookup Discord never answers in time.
    const SLOW_USER: &str = "slowpoke";

    impl DiscordApi for MockDiscord {
        async fn exchange_code(
            &self,
            code: &str,
            _redirect: &str,
        ) -> Result<String, ExchangeError> {
            // The test "code" is the user id we want a token for, except
            // for two codes that stand for Discord's two kinds of failure.
            match code {
                "spent" => Err(ExchangeError::Rejected("status 400".to_owned())),
                "down" => Err(ExchangeError::Unavailable("status 502".to_owned())),
                _ => Ok(format!("access-for-{code}")),
            }
        }
        async fn current_user_id(&self, access_token: &str) -> Result<String, String> {
            Ok(access_token.trim_start_matches("access-for-").to_owned())
        }
        async fn guild_member(
            &self,
            guild_id: &str,
            user_id: &str,
        ) -> Result<Option<auth::GuildMember>, LookupError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if user_id == SLOW_USER {
                tokio::time::sleep(std::time::Duration::from_mins(1)).await;
            }
            if guild_id.starts_with("botless") {
                return Err(LookupError::BotNotInGuild);
            }
            Ok(self
                .members
                .get(&(guild_id.to_owned(), user_id.to_owned()))
                .map(|roles| auth::GuildMember {
                    roles: roles.clone(),
                    joined_at: None,
                    name: Some(format!("{user_id} (nick)")),
                }))
        }
        async fn guild_channels(&self, _guild_id: &str) -> Result<Vec<auth::GuildChannel>, String> {
            self.channel_lists.fetch_add(1, Ordering::Relaxed);
            if self.channels_down.load(Ordering::Relaxed) {
                return Err("channel list returned 502".to_owned());
            }
            let ch = |id: &str, name: &str, kind: u8, position: i64| auth::GuildChannel {
                id: id.to_owned(),
                name: name.to_owned(),
                kind,
                position,
            };
            let mut channels = vec![
                ch("c2", "sketches", auth::CHANNEL_KIND_TEXT, 2),
                ch("c1", "daily-photos", auth::CHANNEL_KIND_TEXT, 1),
                ch("v1", "Voice", 2, 0),
                ch("n1", "news", auth::CHANNEL_KIND_ANNOUNCEMENT, 3),
            ];
            channels.extend(self.extra_channels.lock().unwrap().iter().cloned());
            let gone = self.gone_channels.lock().unwrap();
            channels.retain(|channel| !gone.contains(&channel.id));
            drop(gone);
            Ok(channels)
        }
        async fn guild_roles(&self, _guild_id: &str) -> Result<Vec<auth::GuildRole>, String> {
            let mut roles = vec![
                auth::GuildRole {
                    id: "role-low".to_owned(),
                    name: "Regular".to_owned(),
                    position: 1,
                },
                auth::GuildRole {
                    id: ROLE.to_owned(),
                    name: "VIP".to_owned(),
                    position: 5,
                },
            ];
            roles.extend(self.extra_roles.lock().unwrap().iter().cloned());
            Ok(roles)
        }
        async fn guild_summary(
            &self,
            _guild_id: &str,
        ) -> Result<Option<auth::GuildSummary>, String> {
            Ok(Some(auth::GuildSummary {
                name: "Test Guild".to_owned(),
                icon: Some("abc123".to_owned()),
            }))
        }
        async fn managed_guild_ids(&self, access_token: &str) -> Result<Vec<String>, String> {
            let user = access_token.trim_start_matches("access-for-");
            Ok(self.manages.get(user).cloned().unwrap_or_default())
        }
        async fn send_message(&self, channel_id: &str, content: &str) -> Result<(), String> {
            self.sent
                .lock()
                .unwrap()
                .push((channel_id.to_owned(), content.to_owned()));
            Ok(())
        }
    }

    const GUILD: &str = "g1";
    const CREATOR: &str = "creator1";
    const MEMBER: &str = "member1";
    const ROLE: &str = "role-vip";
    /// Bytes seeded at the `k-orig` object key for the media-stream test.
    const MEDIA_BYTES: &[u8] = b"\x89PNG\r\n\x1a\n not-a-real-image";

    /// Everything a test may want to reach into.
    struct Fixture {
        app: Router,
        /// What the router was built over: the same caches and stub.
        state: ApiState<MockDiscord>,
        key: SessionKey,
        calls: Arc<AtomicUsize>,
        sent: Arc<Mutex<Vec<(String, String)>>>,
        pool: SqlitePool,
        extra_roles: Arc<Mutex<Vec<auth::GuildRole>>>,
        extra_channels: Arc<Mutex<Vec<auth::GuildChannel>>>,
        gone_channels: Arc<Mutex<Vec<String>>>,
        channels_down: Arc<AtomicBool>,
        channel_lists: Arc<AtomicUsize>,
    }

    /// The seeded app + key + the membership-lookup counter.
    async fn app() -> (Router, SessionKey, Arc<AtomicUsize>) {
        let f = fixture().await;
        (f.app, f.key, f.calls)
    }

    /// Builds a fully-seeded app. Members: creator (no roles), a plain
    /// member, and a VIP member holding `ROLE`. Series of each visibility.
    #[allow(clippy::too_many_lines, reason = "test fixture seeds many rows")]
    async fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let pool = leaf_core::db::connect(&dir.path().join("t.db"))
            .await
            .unwrap();
        // Keep the temp DB file alive for the rest of the test process.
        std::mem::forget(dir);

        let guilds_repo = GuildSettingsRepo::new(pool.clone());
        guilds_repo.ensure_exists(GUILD).await.unwrap();
        // GUILD is fully set up with two watched channels and open creation.
        let mut gs = leaf_core::domain::GuildSettings::defaults_for(GUILD);
        gs.setup_complete = true;
        gs.watched_channels = vec!["c1".to_owned(), "c2".to_owned()];
        gs.timezone = "America/Chicago".to_owned();
        gs.max_series_per_user = 10;
        guilds_repo.upsert(&gs).await.unwrap();
        // "g3" is set up but requires a creator role the plain member lacks.
        guilds_repo.ensure_exists("g3").await.unwrap();
        let mut g3 = leaf_core::domain::GuildSettings::defaults_for("g3");
        g3.setup_complete = true;
        g3.watched_channels = vec!["c1".to_owned()];
        g3.creator_role_id = Some("role-needed".to_owned());
        guilds_repo.upsert(&g3).await.unwrap();
        // "gns" is a guild the bot has joined (it has the row the bot makes
        // on joining) where nobody has run `/setup`.
        guilds_repo.ensure_exists("gns").await.unwrap();
        let series_repo = SeriesRepo::new(pool.clone());
        let posts = PostRepo::new(pool.clone());

        let mk = |name: &str, privacy: Privacy, role: Option<&str>, state: SeriesState| NewSeries {
            guild_id: GUILD.to_owned(),
            creator_id: CREATOR.to_owned(),
            name: name.to_owned(),
            description: String::new(),
            channels: vec!["c1".to_owned()],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy,
            privacy_role_id: role.map(ToOwned::to_owned),
            start_day: 1,
            state,
        };

        // id 1 public-active, 2 role-gated, 3 creator-only, 4 sprout, 5 revoked.
        for ns in [
            mk("public", Privacy::Public, None, SeriesState::Active),
            mk("gated", Privacy::RoleGated, Some(ROLE), SeriesState::Active),
            mk("private", Privacy::CreatorOnly, None, SeriesState::Active),
            mk("sprout", Privacy::Public, None, SeriesState::Sprout),
            mk("revoked", Privacy::Public, None, SeriesState::Revoked),
        ] {
            series_repo.create(&ns, 0).await.unwrap();
        }
        // A day with media in the public series (id 1).
        posts
            .insert_with_media(
                &Post {
                    series_id: 1,
                    day: 1,
                    message_id: "m1".to_owned(),
                    channel_id: "c1".to_owned(),
                    caption: "Day 1".to_owned(),
                    posted_at: 1000,
                    archived_at: 1001,
                },
                &[NewMediaAttachment {
                    attachment_id: "att1".to_owned(),
                    channel_id: "c1".to_owned(),
                    message_id: "m1".to_owned(),
                    content_type: "image/png".to_owned(),
                    original_key: Some("k-orig".to_owned()),
                    thumb_key: Some("k-thumb".to_owned()),
                    media_missing: false,
                }],
            )
            .await
            .unwrap();

        let mut members = std::collections::HashMap::new();
        members.insert((GUILD.to_owned(), CREATOR.to_owned()), vec![]);
        members.insert((GUILD.to_owned(), MEMBER.to_owned()), vec![]);
        members.insert((GUILD.to_owned(), "vip".to_owned()), vec![ROLE.to_owned()]);
        // For eligibility tests: a member of a not-set-up guild, and a member
        // of a guild that requires a creator role they lack.
        members.insert(("gns".to_owned(), MEMBER.to_owned()), vec![]);
        members.insert(("g3".to_owned(), MEMBER.to_owned()), vec![]);
        // Guilds some tests set up for themselves (see `own_guild`).
        members.insert(("g-fresh-channels".to_owned(), MEMBER.to_owned()), vec![]);
        for gid in ["g-gone-channel", "g-names-down", "g-names-burst"] {
            members.insert((gid.to_owned(), CREATOR.to_owned()), vec![]);
        }
        members.insert(("g-slow".to_owned(), "quick".to_owned()), vec![]);

        // CREATOR manages the guild; MEMBER manages nothing.
        let mut manages = std::collections::HashMap::new();
        manages.insert(CREATOR.to_owned(), vec![GUILD.to_owned()]);

        let key = SessionKey::derive("test-secret");
        let calls = Arc::new(AtomicUsize::new(0));
        let sent = Arc::new(Mutex::new(Vec::new()));
        let extra_roles = Arc::new(Mutex::new(Vec::new()));
        let extra_channels = Arc::new(Mutex::new(Vec::new()));
        let gone_channels = Arc::new(Mutex::new(Vec::new()));
        let channels_down = Arc::new(AtomicBool::new(false));
        let channel_lists = Arc::new(AtomicUsize::new(0));
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        // Seed only the original (not the thumb), so the streaming test gets
        // bytes while the signature-gate test still 404s on the absent thumb.
        store
            .put(
                &object_store::path::Path::from("k-orig"),
                bytes::Bytes::from_static(MEDIA_BYTES).into(),
            )
            .await
            .unwrap();
        let state = ApiState {
            series: series_repo,
            posts,
            guilds: GuildSettingsRepo::new(pool.clone()),
            store,
            key: key.clone(),
            discord: Arc::new(MockDiscord {
                members,
                manages,
                calls: Arc::clone(&calls),
                sent: Arc::clone(&sent),
                extra_roles: Arc::clone(&extra_roles),
                extra_channels: Arc::clone(&extra_channels),
                gone_channels: Arc::clone(&gone_channels),
                channels_down: Arc::clone(&channels_down),
                channel_lists: Arc::clone(&channel_lists),
            }),
            membership: crate::api::state::membership_cache(),
            channels: crate::api::state::channels_cache(),
            redirect_uri: "https://leaf.test".to_owned(),
            client_id: "client-123".to_owned(),
        };
        Fixture {
            app: router(state.clone()),
            state,
            key,
            calls,
            sent,
            pool,
            extra_roles,
            extra_channels,
            gone_channels,
            channels_down,
            channel_lists,
        }
    }

    async fn get(router: &Router, path: &str, bearer: Option<&str>) -> StatusCode {
        let mut req = Request::get(path);
        if let Some(b) = bearer {
            req = req.header("Authorization", format!("Bearer {b}"));
        }
        router
            .clone()
            .oneshot(req.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    fn token_for(key: &SessionKey, user: &str) -> String {
        key.mint(user, now_unix(), SESSION_TTL_SECS)
    }

    #[tokio::test]
    async fn unauthenticated_is_401_everywhere() {
        let (app, _key, _calls) = app().await;
        for path in [
            "/api/guilds/g1/series",
            "/api/guilds/g1/series/1/days",
            "/api/guilds/g1/series/1/days/1",
            "/api/guilds/g1/series/1/stats",
        ] {
            assert_eq!(
                get(&app, path, None).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
            assert_eq!(
                get(&app, path, Some("garbage")).await,
                StatusCode::UNAUTHORIZED,
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn non_member_is_forbidden() {
        let (app, key, _calls) = app().await;
        let outsider = token_for(&key, "stranger");
        assert_eq!(
            get(&app, "/api/guilds/g1/series", Some(&outsider)).await,
            StatusCode::FORBIDDEN
        );
        // A series route for a non-member is also refused (Forbidden before
        // any series existence is revealed).
        assert_eq!(
            get(&app, "/api/guilds/g1/series/1/stats", Some(&outsider)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn membership_lookups_are_cached() {
        // Two requests by the same member resolve membership with a single
        // Discord call — the cache answers the second. This is what stops a
        // burst of gallery tiles from fanning out into rate-limited lookups.
        let (app, key, calls) = app().await;
        let member = token_for(&key, MEMBER);
        for _ in 0..2 {
            assert_eq!(
                get(&app, "/api/guilds/g1/series", Some(&member)).await,
                StatusCode::OK
            );
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn list_hides_sprout_revoked_and_unentitled_private() {
        let (app, key, _calls) = app().await;
        // Plain member sees only public (id 1). gated/private/sprout/revoked hidden.
        let member = token_for(&key, MEMBER);
        let resp = router_json(&app, "/api/guilds/g1/series", &member).await;
        let names: Vec<&str> = resp.iter().map(|s| s["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["public"]);

        // VIP additionally sees the role-gated series.
        let vip = token_for(&key, "vip");
        let resp = router_json(&app, "/api/guilds/g1/series", &vip).await;
        let mut names: Vec<&str> = resp.iter().map(|s| s["name"].as_str().unwrap()).collect();
        names.sort_unstable();
        assert_eq!(names, ["gated", "public"]);

        // The creator sees their own creator-only and sprout too, and their
        // revoked series, so they can see what happened to it.
        let creator = token_for(&key, CREATOR);
        let resp = router_json(&app, "/api/guilds/g1/series", &creator).await;
        assert_eq!(resp.len(), 5);
        let revoked = resp.iter().find(|s| s["name"] == "revoked").unwrap();
        assert_eq!(revoked["state"], "revoked");
        assert_eq!(revoked["is_owner"], true);
        let sprout = resp.iter().find(|s| s["name"] == "sprout").unwrap();
        assert_eq!(sprout["sprout"]["archived"], 0);
        assert_eq!(sprout["sprout"]["threshold"], 3);
        // Listed, but still not readable.
        for path in [
            "/api/guilds/g1/series/5/days",
            "/api/guilds/g1/series/5/stats",
        ] {
            assert_eq!(
                get(&app, path, Some(&creator)).await,
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }

        let public = resp.iter().find(|s| s["name"] == "public").unwrap();
        assert_eq!(public["timezone"], "America/Chicago");
        assert_eq!(public["total_days"], 1);
        assert_eq!(public["last_posted_at"], 1000);
        assert_eq!(public["channel_ids"][0], "c1");
        assert!(public["sprout"].is_null());
    }

    #[tokio::test]
    async fn private_series_is_404_for_non_creator_member() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);
        // Series 3 is creator-only → 404 for a plain member on every route.
        for path in [
            "/api/guilds/g1/series/3/stats",
            "/api/guilds/g1/series/3/days/1",
        ] {
            assert_eq!(
                get(&app, path, Some(&member)).await,
                StatusCode::NOT_FOUND,
                "{path}"
            );
        }
        // The creator can reach it (stats ok).
        let creator = token_for(&key, CREATOR);
        assert_eq!(
            get(&app, "/api/guilds/g1/series/3/stats", Some(&creator)).await,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn cross_guild_series_id_is_404() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);
        // Series 1 exists but not under guild "other".
        // (member isn't in "other", so this is Forbidden at the membership gate.)
        assert_eq!(
            get(&app, "/api/guilds/other/series/1/stats", Some(&member)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn day_and_media_signing_round_trip() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);
        // Day 1 of the public series is visible and carries signed media.
        let body = router_json_value(&app, "/api/guilds/g1/series/1/days/1", &member).await;
        let thumb = body["media"][0]["thumb_url"].as_str().unwrap();
        assert!(thumb.starts_with("/api/media/att1?thumb=1&exp="));
        // A missing day is 404.
        assert_eq!(
            get(&app, "/api/guilds/g1/series/1/days/99", Some(&member)).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn media_requires_a_valid_signature() {
        let (app, key, _calls) = app().await;
        // Unsigned / bad signature → 403.
        assert_eq!(
            get(&app, "/api/media/att1?exp=9999999999&sig=bad", None).await,
            StatusCode::FORBIDDEN
        );
        // A correctly-signed *thumbnail* request passes the gate but 404s,
        // because the thumb object (`k-thumb`) isn't seeded — proving the sig
        // gate let it through to the store.
        let exp = now_unix() + 60;
        let sig = key.sign_media("att1", exp);
        let path = format!("/api/media/att1?thumb=1&exp={exp}&sig={sig}");
        assert_eq!(get(&app, &path, None).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn media_streams_the_original_with_bounded_cache_headers() {
        let (app, key, _calls) = app().await;
        let exp = now_unix() + 60;
        // The original (no `thumb`) maps to the seeded `k-orig` object.
        let sig = key.sign_media("att1", exp);
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!("/api/media/att1?exp={exp}&sig={sig}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap(),
            "image/png"
        );
        // A browser may keep its copy for good; a shared cache only for as
        // long as the signed URL itself is valid (here: at most a minute).
        let cache = resp
            .headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap();
        let shared: i64 = cache
            .strip_prefix("public, max-age=31536000, s-maxage=")
            .and_then(|rest| rest.strip_suffix(", immutable"))
            .unwrap()
            .parse()
            .unwrap();
        assert!((1..=60).contains(&shared), "{cache}");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), MEDIA_BYTES);
    }

    #[tokio::test]
    async fn token_exchange_mints_a_usable_session() {
        let (app, _key, _calls) = app().await;
        // Exchange a code for member1, then use the returned token.
        let resp = app
            .clone()
            .oneshot(
                Request::post("/api/token")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"code":"member1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let token = v["token"].as_str().unwrap();
        // The Discord access token is handed back for `sdk.authenticate`.
        assert_eq!(v["access_token"].as_str().unwrap(), "access-for-member1");
        assert_eq!(
            get(&app, "/api/guilds/g1/series", Some(token)).await,
            StatusCode::OK
        );
    }

    // --- admin panel ---
    fn admin_token(key: &SessionKey, user: &str, guilds: &[&str]) -> String {
        let g: Vec<String> = guilds.iter().map(|s| (*s).to_owned()).collect();
        key.mint_admin(user, &g, now_unix(), 3600)
    }

    async fn patch_json(
        router: &Router,
        path: &str,
        bearer: &str,
        body: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = router
            .clone()
            .oneshot(
                Request::patch(path)
                    .header("Authorization", format!("Bearer {bearer}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    #[tokio::test]
    async fn admin_routes_reject_missing_and_gallery_tokens() {
        let (app, key, _calls) = app().await;
        assert_eq!(
            get(&app, "/api/admin/guilds", None).await,
            StatusCode::UNAUTHORIZED
        );
        // A gallery session token is not an admin token.
        let gallery = token_for(&key, CREATOR);
        assert_eq!(
            get(&app, "/api/admin/guilds", Some(&gallery)).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn admin_lists_managed_guilds_and_hides_others() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        let guilds = router_json(&app, "/api/admin/guilds", &tok).await;
        assert_eq!(guilds.len(), 1);
        assert_eq!(guilds[0]["guild_id"].as_str().unwrap(), GUILD);
        assert_eq!(guilds[0]["series_count"].as_u64().unwrap(), 5);
        // A guild not in the token is hidden as 404.
        assert_eq!(
            get(&app, "/api/admin/guilds/other", Some(&tok)).await,
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn admin_reads_guild_detail() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        let detail = router_json_value(&app, "/api/admin/guilds/g1", &tok).await;
        assert_eq!(detail["guild_id"].as_str().unwrap(), GUILD);
        assert!(detail["settings"].is_object());
        assert_eq!(detail["series"].as_array().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn admin_patches_settings() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        let (status, v) = patch_json(
            &app,
            "/api/admin/guilds/g1/settings",
            &tok,
            r#"{"timezone":"America/Chicago","sprout_enabled":true,"max_series_per_user":9}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["timezone"].as_str().unwrap(), "America/Chicago");
        assert!(v["sprout_enabled"].as_bool().unwrap());
        assert_eq!(v["max_series_per_user"].as_i64().unwrap(), 9);
    }

    #[tokio::test]
    async fn admin_revokes_and_edits_series_privacy() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        let (status, v) = patch_json(
            &app,
            "/api/admin/guilds/g1/series/1",
            &tok,
            r#"{"state":"revoked"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["state"].as_str().unwrap(), "revoked");

        let (status, v) = patch_json(
            &app,
            "/api/admin/guilds/g1/series/1",
            &tok,
            r#"{"privacy":"creator_only"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["privacy"].as_str().unwrap(), "creator_only");

        // A bogus enum value is a 400.
        let (status, _) = patch_json(
            &app,
            "/api/admin/guilds/g1/series/1",
            &tok,
            r#"{"state":"nope"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn admin_cannot_reach_series_through_an_unmanaged_guild() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        // Series 1 exists (in g1) but is referenced under an unmanaged guild.
        let (status, _) = patch_json(
            &app,
            "/api/admin/guilds/other/series/1",
            &tok,
            r#"{"state":"revoked"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn admin_login_redirects_to_discord_consent() {
        let (app, _key, _calls) = app().await;
        let resp = app
            .clone()
            .oneshot(Request::get("/admin/login").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(loc.contains("discord.com/oauth2/authorize"));
        assert!(loc.contains("client_id=client-123"));
        assert!(loc.contains("redirect_uri=https%3A%2F%2Fleaf.test%2Fadmin%2Fcallback"));
    }

    #[tokio::test]
    async fn admin_callback_mints_a_scoped_token_or_refuses() {
        let (app, key, _calls) = app().await;
        let state = key.sign_oauth_state(now_unix(), 600);

        // CREATOR manages GUILD → a token scoped to it.
        let resp = app
            .clone()
            .oneshot(
                Request::get(format!("/admin/callback?code={CREATOR}&state={state}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        let token = loc.strip_prefix("/admin#token=").unwrap();
        let claims = key.verify_admin(token, now_unix()).unwrap();
        assert_eq!(claims.user_id, CREATOR);
        assert_eq!(claims.guild_ids, vec![GUILD.to_owned()]);

        // Every failure goes back to the panel as a fragment code, never a
        // JSON page: a browser opened this address.
        let landing = |query: String| {
            let app = app.clone();
            async move {
                let resp = app
                    .oneshot(
                        Request::get(format!("/admin/callback?{query}"))
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.status(), StatusCode::SEE_OTHER, "{query}");
                resp.headers()
                    .get("location")
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_owned()
            }
        };
        // A member who manages nothing.
        assert_eq!(
            landing(format!("code={MEMBER}&state={state}")).await,
            "/admin#error=no_guilds"
        );
        // A forged state is rejected before any exchange.
        assert_eq!(
            landing(format!("code={CREATOR}&state=forged")).await,
            "/admin#error=expired"
        );
        assert_eq!(landing("code=x".to_owned()).await, "/admin#error=expired");
        // The person pressed Cancel on Discord's consent screen.
        assert_eq!(
            landing(format!("error=access_denied&state={state}")).await,
            "/admin#error=denied"
        );
        // Discord refused the code, or could not be reached.
        assert_eq!(
            landing(format!("code=spent&state={state}")).await,
            "/admin#error=exchange_failed"
        );
        assert_eq!(
            landing(format!("code=down&state={state}")).await,
            "/admin#error=discord_unavailable"
        );
    }

    // --- creator series management ---

    async fn post_json(
        router: &Router,
        path: &str,
        bearer: &str,
        body: &str,
    ) -> (StatusCode, serde_json::Value) {
        let resp = router
            .clone()
            .oneshot(
                Request::post(path)
                    .header("Authorization", format!("Bearer {bearer}"))
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    #[tokio::test]
    async fn eligibility_reflects_membership_and_policy() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);

        // A set-up guild with open creation: allowed.
        let v = router_json_value(&app, "/api/guilds/g1/series/eligibility", &member).await;
        assert!(v["can_create"].as_bool().unwrap());
        assert!(v["violations"].as_array().unwrap().is_empty());

        // A guild without `/setup`: blocked with guild_not_setup.
        let v = router_json_value(&app, "/api/guilds/gns/series/eligibility", &member).await;
        assert!(!v["can_create"].as_bool().unwrap());
        assert_eq!(
            v["violations"][0]["code"].as_str().unwrap(),
            "guild_not_setup"
        );

        // A guild requiring a creator role the member lacks.
        let v = router_json_value(&app, "/api/guilds/g3/series/eligibility", &member).await;
        assert!(!v["can_create"].as_bool().unwrap());
        assert_eq!(
            v["violations"][0]["code"].as_str().unwrap(),
            "missing_creator_role"
        );

        // A non-member is refused before any policy runs.
        let stranger = token_for(&key, "stranger");
        assert_eq!(
            get(&app, "/api/guilds/g1/series/eligibility", Some(&stranger)).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn options_lists_watched_channels_and_roles() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);
        let v = router_json_value(&app, "/api/guilds/g1/series/options", &member).await;
        let channels = v["channels"].as_array().unwrap();
        assert_eq!(channels.len(), 2);
        assert_eq!(channels[0]["id"].as_str().unwrap(), "c1");
        assert_eq!(channels[0]["name"].as_str().unwrap(), "daily-photos");
        assert_eq!(v["roles"][0]["name"].as_str().unwrap(), "VIP");
        assert_eq!(v["cadences"].as_array().unwrap().len(), 4);
        assert_eq!(v["guild_timezone"].as_str().unwrap(), "America/Chicago");
    }

    #[tokio::test]
    async fn create_enforces_policy_and_uniqueness() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);

        // A valid create succeeds and starts active (sprout disabled).
        let (status, v) = post_json(
            &app,
            "/api/guilds/g1/series",
            &member,
            r#"{"name":"Member Daily","channel_id":"c1","cadence":"daily","privacy":"public"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(v["state"].as_str().unwrap(), "active");

        // It now shows in the member's own series.
        let mine = router_json(&app, "/api/guilds/g1/series/mine", &member).await;
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0]["name"].as_str().unwrap(), "Member Daily");
        assert_eq!(mine[0]["channel_name"].as_str().unwrap(), "daily-photos");
        assert_eq!(mine[0]["channel_missing"], false);

        // A duplicate name (CREATOR already owns "public") is a 409.
        let (status, v) = post_json(
            &app,
            "/api/guilds/g1/series",
            &member,
            r#"{"name":"public","channel_id":"c1","cadence":"daily","privacy":"public"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error"].as_str().unwrap(), "name_taken");

        // An unwatched channel is a 400 invalid_channel.
        let (status, v) = post_json(
            &app,
            "/api/guilds/g1/series",
            &member,
            r#"{"name":"Stray","channel_id":"nope","cadence":"daily","privacy":"public"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"].as_str().unwrap(), "invalid_channel");

        // A too-short name is a 400 invalid_name.
        let (status, v) = post_json(
            &app,
            "/api/guilds/g1/series",
            &member,
            r#"{"name":"x","channel_id":"c1","cadence":"daily","privacy":"public"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"].as_str().unwrap(), "invalid_name");
    }

    #[tokio::test]
    async fn settings_and_patch_are_owner_only() {
        let (app, key, _calls) = app().await;
        let creator = token_for(&key, CREATOR);
        let member = token_for(&key, MEMBER);

        // The creator reads full settings for their series.
        let v = router_json_value(&app, "/api/guilds/g1/series/1/settings", &creator).await;
        assert_eq!(v["name"].as_str().unwrap(), "public");
        assert_eq!(v["channel_id"].as_str().unwrap(), "c1");

        // A non-owner cannot even see settings exist (404).
        assert_eq!(
            get(&app, "/api/guilds/g1/series/1/settings", Some(&member)).await,
            StatusCode::NOT_FOUND
        );

        // The creator patches the description.
        let (status, v) = patch_json(
            &app,
            "/api/guilds/g1/series/1",
            &creator,
            r#"{"description":"now with words"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["description"].as_str().unwrap(), "now with words");

        // A non-owner PATCH is hidden as 404.
        let (status, _) = patch_json(
            &app,
            "/api/guilds/g1/series/1",
            &member,
            r#"{"description":"hijack"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn patch_rejects_revoked_and_freeform_reminder() {
        let (app, key, _calls) = app().await;
        let creator = token_for(&key, CREATOR);

        // Series 5 is revoked → 403 revoked.
        let (status, v) = patch_json(
            &app,
            "/api/guilds/g1/series/5",
            &creator,
            r#"{"description":"x"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(v["error"].as_str().unwrap(), "revoked");

        // Turning a series freeform and enabling reminders is rejected.
        let (status, v) = patch_json(
            &app,
            "/api/guilds/g1/series/1",
            &creator,
            r#"{"cadence":"freeform","reminder_enabled":true,"reminder_time":"10:00"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"].as_str().unwrap(), "reminder_on_freeform");
    }

    // --- session renewal, launch intents, the whole-series index ---

    async fn send_json(
        router: &Router,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&str>,
    ) -> (StatusCode, serde_json::Value) {
        let mut req = Request::builder().method(method).uri(path);
        if let Some(b) = bearer {
            req = req.header("Authorization", format!("Bearer {b}"));
        }
        let body = body.map_or_else(Body::empty, |b| {
            req = std::mem::take(&mut req).header("content-type", "application/json");
            Body::from(b.to_owned())
        });
        let resp = router
            .clone()
            .oneshot(req.body(body).unwrap())
            .await
            .unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, v)
    }

    #[tokio::test]
    async fn refresh_swaps_a_valid_session_and_nothing_else() {
        let (app, key, _calls) = app().await;
        let session = token_for(&key, MEMBER);
        let (status, v) = send_json(&app, "POST", "/api/token/refresh", Some(&session), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["expires_in"].as_i64().unwrap(), SESSION_TTL_SECS);
        let fresh = v["token"].as_str().unwrap();
        assert_eq!(key.verify(fresh, now_unix()).unwrap(), MEMBER);
        assert_eq!(
            get(&app, "/api/guilds/g1/series", Some(fresh)).await,
            StatusCode::OK
        );

        // No token, a forged token and an admin token are all refused.
        let admin = admin_token(&key, CREATOR, &[GUILD]);
        for bearer in [None, Some("garbage"), Some(admin.as_str())] {
            let (status, v) = send_json(&app, "POST", "/api/token/refresh", bearer, None).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert_eq!(v["error"], "unauthorized");
        }
        // A Discord access token in the body buys nothing.
        let (status, _) = send_json(
            &app,
            "POST",
            "/api/token/refresh",
            None,
            Some(r#"{"access_token":"access-for-member1"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // Past the 7-day cap the session cannot be renewed, even though the
        // token itself has not lapsed yet.
        let signed_in = now_unix() - SESSION_MAX_AGE_SECS - 10;
        let old = key.mint(MEMBER, signed_in, SESSION_MAX_AGE_SECS + 600);
        assert!(key.verify(&old, now_unix()).is_ok());
        let (status, _) = send_json(&app, "POST", "/api/token/refresh", Some(&old), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn launch_intent_is_read_once_and_only_for_viewable_series() {
        let f = fixture().await;
        let intents = LaunchIntentRepo::new(f.pool.clone());
        let member = token_for(&f.key, MEMBER);
        let path = "/api/guilds/g1/launch-intent";

        // Nothing pending: JSON null, not an error.
        let (status, v) = send_json(&f.app, "GET", path, Some(&member), None).await;
        assert_eq!((status, v), (StatusCode::OK, serde_json::Value::Null));

        intents
            .put(MEMBER, GUILD, 1, Some(1), now_unix())
            .await
            .unwrap();
        let (status, v) = send_json(&f.app, "GET", path, Some(&member), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v, serde_json::json!({ "series_id": 1, "day": 1 }));
        // Reading consumed it.
        let (_, v) = send_json(&f.app, "GET", path, Some(&member), None).await;
        assert!(v.is_null());

        // A series the caller may not view (3 is creator-only), one that is
        // gone, and one filed under another guild all read as nothing.
        for (guild, series) in [(GUILD, 3), (GUILD, 999), ("g3", 1)] {
            intents
                .put(MEMBER, guild, series, None, now_unix())
                .await
                .unwrap();
            let url = format!("/api/guilds/{guild}/launch-intent");
            let (status, v) = send_json(&f.app, "GET", &url, Some(&member), None).await;
            assert_eq!(status, StatusCode::OK, "{guild}/{series}");
            assert!(v.is_null(), "{guild}/{series}");
        }

        // The creator gets their own private series, with a null day.
        intents
            .put(CREATOR, GUILD, 3, None, now_unix())
            .await
            .unwrap();
        let creator = token_for(&f.key, CREATOR);
        let (_, v) = send_json(&f.app, "GET", path, Some(&creator), None).await;
        assert_eq!(v, serde_json::json!({ "series_id": 3, "day": null }));

        // Membership still gates the route.
        let stranger = token_for(&f.key, "stranger");
        assert_eq!(
            get(&f.app, path, Some(&stranger)).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(get(&f.app, path, None).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn a_repeated_launch_intent_read_gets_the_same_answer() {
        let f = fixture().await;
        let intents = LaunchIntentRepo::new(f.pool.clone());
        let member = token_for(&f.key, MEMBER);
        let first = "/api/guilds/g1/launch-intent?attempt=replaytest1";

        intents
            .put(MEMBER, GUILD, 1, Some(1), now_unix())
            .await
            .unwrap();
        let want = serde_json::json!({ "series_id": 1, "day": 1 });
        // The first answer was lost on the way back; the retry repeats the id.
        for _ in 0..2 {
            let (status, v) = send_json(&f.app, "GET", first, Some(&member), None).await;
            assert_eq!((status, v), (StatusCode::OK, want.clone()));
        }

        // Another read, no id, an unusable id, and another user repeating
        // the id all get nothing: the intent is still acted on once.
        let creator = token_for(&f.key, CREATOR);
        for (path, bearer) in [
            ("/api/guilds/g1/launch-intent?attempt=replaytest2", &member),
            ("/api/guilds/g1/launch-intent", &member),
            (
                "/api/guilds/g1/launch-intent?attempt=Replay%20Test",
                &member,
            ),
            (first, &creator),
        ] {
            let (status, v) = send_json(&f.app, "GET", path, Some(bearer), None).await;
            assert_eq!((status, v), (StatusCode::OK, serde_json::Value::Null));
        }

        // A newer press wins over what an old id remembers.
        intents
            .put(MEMBER, GUILD, 1, None, now_unix())
            .await
            .unwrap();
        let (_, v) = send_json(&f.app, "GET", first, Some(&member), None).await;
        assert_eq!(v, serde_json::json!({ "series_id": 1, "day": null }));

        // A remembered intent is checked again on the repeat: once the
        // series is no longer the caller's to view, it reads as nothing.
        intents
            .put(MEMBER, GUILD, 1, Some(1), now_unix())
            .await
            .unwrap();
        let again = "/api/guilds/g1/launch-intent?attempt=replaytest3";
        let (_, v) = send_json(&f.app, "GET", again, Some(&member), None).await;
        assert_eq!(v, want);
        let series = SeriesRepo::new(f.pool.clone());
        let mut public = series.get(1).await.unwrap().unwrap();
        public.privacy = Privacy::CreatorOnly;
        series.update(&public).await.unwrap();
        let (status, v) = send_json(&f.app, "GET", again, Some(&member), None).await;
        assert_eq!((status, v), (StatusCode::OK, serde_json::Value::Null));
    }

    #[test]
    fn attempt_ids_are_short_lowercase_alphanumerics() {
        let usable = |s: &str| usable_attempt(Some(s.to_owned())).is_some();
        assert!(usable("mb3k2x9a0f1c"));
        assert!(usable(&"a".repeat(32)));
        for bad in ["", "A1", "a b", "a-b", "é", &"a".repeat(33)] {
            assert!(!usable(bad), "{bad:?}");
        }
        assert_eq!(usable_attempt(None), None);
    }

    #[tokio::test]
    async fn a_guild_id_that_is_not_an_id_never_reaches_discord() {
        let (app, key, calls) = app().await;
        let member = token_for(&key, MEMBER);
        // Percent-decoded by the router into `1/members/2?`, `../channels/3?`
        // and friends: each would otherwise change the Discord URL asked for.
        for gid in [
            "1%2Fmembers%2F2%3F",
            "..%2Fchannels%2F3%3F",
            "g1%23",
            "g1%20",
            "g1%3Fx%3D1",
            "%2E%2E",
            "111111111111111111111111111111111",
        ] {
            for tail in [
                "series",
                "launch-intent",
                "series/eligibility",
                "series/1/days",
            ] {
                let path = format!("/api/guilds/{gid}/{tail}");
                assert_eq!(
                    get(&app, &path, Some(&member)).await,
                    StatusCode::NOT_FOUND,
                    "{path}"
                );
            }
        }
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        assert!(is_plain_id("123456789012345678"));
        assert!(is_plain_id("g-fresh_1"));
        assert!(!is_plain_id(""));
    }

    #[tokio::test]
    async fn days_without_a_range_are_the_whole_index() {
        let f = fixture().await;
        let posts = PostRepo::new(f.pool.clone());
        // 60 more days in the public series: more than the old 35-day window.
        for day in 2..=61 {
            posts
                .insert_with_media(
                    &Post {
                        series_id: 1,
                        day,
                        message_id: format!("m{day}"),
                        channel_id: "c1".to_owned(),
                        caption: String::new(),
                        // 2024-01-01 03:30 UTC: still 31 Dec in Chicago.
                        posted_at: 1_704_079_800,
                        archived_at: 1_704_079_900,
                    },
                    &[],
                )
                .await
                .unwrap();
        }
        let member = token_for(&f.key, MEMBER);
        let all = router_json(&f.app, "/api/guilds/g1/series/1/days", &member).await;
        assert_eq!(all.len(), 61);
        assert_eq!(all[0]["day"], 1);
        assert_eq!(all[60]["day"], 61);
        // Every row dates itself in the guild's timezone.
        assert_eq!(all[1]["local_date"], "2023-12-31");
        assert!(all.iter().all(|d| d["local_date"].is_string()));
        // Day 1 has a stored thumbnail; the bare days have none and say so.
        assert!(all[0]["thumb_url"].as_str().unwrap().contains("thumb=1"));
        assert!(all[1]["thumb_url"].is_null());
        assert_eq!(all[1]["missing"], true);
        assert_eq!(all[1]["count"], 0);

        // An explicit range still pages, and stays capped.
        let page = router_json(
            &f.app,
            "/api/guilds/g1/series/1/days?from=10&to=12",
            &member,
        )
        .await;
        let days: Vec<i64> = page.iter().map(|d| d["day"].as_i64().unwrap()).collect();
        assert_eq!(days, [10, 11, 12]);
        for bad in ["from=5&to=1", "from=1&to=1000"] {
            assert_eq!(
                get(
                    &f.app,
                    &format!("/api/guilds/g1/series/1/days?{bad}"),
                    Some(&member)
                )
                .await,
                StatusCode::BAD_REQUEST,
                "{bad}"
            );
        }

        // An empty series is 200 [], which the client reads as "no days".
        let vip = token_for(&f.key, "vip");
        let none = router_json(&f.app, "/api/guilds/g1/series/2/days", &vip).await;
        assert!(none.is_empty());
    }

    #[tokio::test]
    async fn token_exchange_failures_carry_their_own_codes() {
        let (app, _key, _calls) = app().await;
        let (status, v) = send_json(
            &app,
            "POST",
            "/api/token",
            None,
            Some(r#"{"code":"spent"}"#),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "code_rejected");
        assert_eq!(v["retryable"], false);
        assert!(v["message"].is_string());

        let (status, v) =
            send_json(&app, "POST", "/api/token", None, Some(r#"{"code":"down"}"#)).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(v["error"], "discord_unavailable");
        assert_eq!(v["retryable"], true);

        // A body leaf cannot read is answered in the API's own shape.
        let (status, v) = send_json(&app, "POST", "/api/token", None, Some("not json")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "bad_request");
    }

    #[tokio::test]
    async fn a_guild_without_the_bot_says_so() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);
        let (status, v) = send_json(
            &app,
            "GET",
            "/api/guilds/botless/series",
            Some(&member),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(
            v["message"]
                .as_str()
                .unwrap()
                .contains("bot isn't in this server")
        );
    }

    #[tokio::test]
    async fn a_guild_without_the_bot_is_not_asked_about_again() {
        let f = fixture().await;
        let member = token_for(&f.key, MEMBER);
        let refused = |v: &serde_json::Value| {
            v["message"]
                .as_str()
                .unwrap()
                .contains("bot isn't in this server")
        };

        // A guild leaf has never seen (no settings row) is refused without
        // a call to Discord, however often it is asked for.
        for _ in 0..3 {
            let path = "/api/guilds/4242424242/series";
            let (status, v) = send_json(&f.app, "GET", path, Some(&member), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert!(refused(&v));
        }
        assert_eq!(f.calls.load(Ordering::Relaxed), 0);

        // A guild leaf knew but has left: Discord says so once, and that
        // answer is remembered, for every member who asks.
        GuildSettingsRepo::new(f.pool.clone())
            .ensure_exists("botless-left")
            .await
            .unwrap();
        for user in [MEMBER, CREATOR, MEMBER] {
            let token = token_for(&f.key, user);
            let path = "/api/guilds/botless-left/series";
            let (status, v) = send_json(&f.app, "GET", path, Some(&token), None).await;
            assert_eq!(status, StatusCode::FORBIDDEN);
            assert!(refused(&v));
        }
        assert_eq!(f.calls.load(Ordering::Relaxed), 1);
    }

    // --- creator API: the richer shapes ---

    #[tokio::test]
    async fn eligibility_lists_every_blocker_with_its_numbers() {
        let f = fixture().await;
        let guilds = GuildSettingsRepo::new(f.pool.clone());
        let mut g3 = guilds.get("g3").await.unwrap().unwrap();
        g3.max_series_per_user = 1;
        g3.min_membership_age_days = 30;
        g3.creator_role_id = Some(ROLE.to_owned());
        guilds.upsert(&g3).await.unwrap();
        // The member already has their one series there.
        SeriesRepo::new(f.pool.clone())
            .create(
                &NewSeries {
                    guild_id: "g3".to_owned(),
                    creator_id: MEMBER.to_owned(),
                    name: "mine".to_owned(),
                    description: String::new(),
                    channels: vec!["c1".to_owned()],
                    cadence: Cadence::Daily,
                    detection_mode: DetectionMode::ContextMenu,
                    privacy: Privacy::Public,
                    privacy_role_id: None,
                    start_day: 1,
                    state: SeriesState::Active,
                },
                0,
            )
            .await
            .unwrap();

        let member = token_for(&f.key, MEMBER);
        let v = router_json_value(&f.app, "/api/guilds/g3/series/eligibility", &member).await;
        assert_eq!(v["can_create"], false);
        assert_eq!(v["owns_any"], true);
        let codes: Vec<&str> = v["violations"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x["code"].as_str().unwrap())
            .collect();
        assert_eq!(
            codes,
            ["missing_creator_role", "max_series", "membership_too_new"]
        );
        assert_eq!(v["violations"][0]["params"]["role_name"], "VIP");
        assert_eq!(v["violations"][1]["params"]["limit"], 1);
        assert_eq!(v["violations"][1]["params"]["current"], 1);
        assert_eq!(v["violations"][2]["params"]["days"], 30);
        // The join date is unknown in the mock, so no date is promised.
        assert!(v["violations"][2]["params"].get("eligible_at").is_none());
        assert!(
            v["violations"][1]["message"]
                .as_str()
                .unwrap()
                .contains("limit of 1")
        );

        // In the open guild the plain member owns nothing yet.
        let v = router_json_value(&f.app, "/api/guilds/g1/series/eligibility", &member).await;
        assert_eq!(v["owns_any"], false);
        // A creator whose series include a revoked one still "owns" some.
        let creator = token_for(&f.key, CREATOR);
        let v = router_json_value(&f.app, "/api/guilds/g1/series/eligibility", &creator).await;
        assert_eq!(v["owns_any"], true);
    }

    #[tokio::test]
    async fn options_sort_roles_mark_held_and_null_unknown_channels() {
        let f = fixture().await;
        let guilds = GuildSettingsRepo::new(f.pool.clone());
        let mut gs = guilds.get(GUILD).await.unwrap().unwrap();
        gs.watched_channels.push("gone".to_owned());
        guilds.upsert(&gs).await.unwrap();

        let vip = token_for(&f.key, "vip");
        let v = router_json_value(&f.app, "/api/guilds/g1/series/options", &vip).await;
        // Highest role first; `held` marks the caller's own.
        assert_eq!(v["roles"][0]["name"], "VIP");
        assert_eq!(v["roles"][0]["held"], true);
        assert_eq!(v["roles"][1]["name"], "Regular");
        assert_eq!(v["roles"][1]["held"], false);
        assert_eq!(v["roles_unavailable"], false);
        // A series channel Discord no longer lists has no name, not its id.
        assert_eq!(v["channels"][2]["id"], "gone");
        assert!(v["channels"][2]["name"].is_null());

        // A guild that has not run /setup says so, with a sentence.
        let member = token_for(&f.key, MEMBER);
        let (status, v) = send_json(
            &f.app,
            "GET",
            "/api/guilds/gns/series/options",
            Some(&member),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(v["error"], "guild_not_setup");
        assert!(v["message"].as_str().unwrap().contains("/setup"));
    }

    #[tokio::test]
    async fn create_answers_the_full_series_and_tolerates_a_repeat() {
        let (app, key, _calls) = app().await;
        let member = token_for(&key, MEMBER);
        let body =
            r#"{"name":"  Member Daily ","channel_id":"c1","cadence":"daily","privacy":"public"}"#;
        let (status, first) = post_json(&app, "/api/guilds/g1/series", &member, body).await;
        assert_eq!(status, StatusCode::CREATED);
        assert_eq!(first["name"], "Member Daily");
        assert_eq!(first["is_owner"], true);
        assert_eq!(first["total_days"], 0);
        assert_eq!(first["channel_ids"][0], "c1");

        // The same request again (the first answer was lost): the same
        // series, not "name taken".
        let (status, again) = post_json(&app, "/api/guilds/g1/series", &member, body).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(again["id"], first["id"]);
        let mine = router_json(&app, "/api/guilds/g1/series/mine", &member).await;
        assert_eq!(mine.len(), 1);

        // Someone else asking for that name is still refused, with a sentence.
        let creator = token_for(&key, CREATOR);
        let (status, v) = post_json(&app, "/api/guilds/g1/series", &creator, body).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error"], "name_taken");
        assert!(v["message"].as_str().unwrap().contains("already exists"));

        // A role that is not this guild's is refused before anything is made.
        let (status, v) = post_json(
            &app,
            "/api/guilds/g1/series",
            &member,
            r#"{"name":"Gated","channel_id":"c1","cadence":"daily","privacy":"role_gated","privacy_role_id":"nope"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "unknown_role");
    }

    #[tokio::test]
    async fn patch_renames_and_moves_the_first_day() {
        let (app, key, _calls) = app().await;
        let creator = token_for(&key, CREATOR);
        let path = "/api/guilds/g1/series/1";

        let (status, v) = patch_json(&app, path, &creator, r#"{"name":" Renamed "}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["name"], "Renamed");
        assert_eq!(v["start_day"], 1);

        // Another series' name is refused; so is a name with a mention.
        let (status, v) = patch_json(&app, path, &creator, r#"{"name":"gated"}"#).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(v["error"], "name_taken");
        let (status, v) = patch_json(&app, path, &creator, r#"{"name":"hi @everyone"}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "invalid_name");
        assert!(v["message"].as_str().unwrap().contains("mention"));

        // Day 1 is archived, so the first day cannot move past it…
        let (status, v) = patch_json(&app, path, &creator, r#"{"start_day":2}"#).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(v["error"], "invalid_start_day");
        assert!(v["message"].as_str().unwrap().contains("Day 1"));
        // …but an empty series can start anywhere in range.
        let (status, v) = patch_json(
            &app,
            "/api/guilds/g1/series/2",
            &creator,
            r#"{"start_day":40}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["start_day"], 40);
        let list = router_json(&app, "/api/guilds/g1/series", &creator).await;
        assert_eq!(list.iter().find(|s| s["id"] == 2).unwrap()["start_day"], 40);

        // A client that still sends the passive-capture switch changes nothing.
        let (status, v) = patch_json(&app, path, &creator, r#"{"detection_mode":"passive"}"#).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["detection_mode"], "context_menu");

        // A non-owner learns nothing from a bad value: still 404.
        let member = token_for(&key, MEMBER);
        let (status, _) = patch_json(&app, path, &member, r#"{"name":"x"}"#).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn settings_and_my_series_report_an_undelivered_reminder() {
        let f = fixture().await;
        let creator = token_for(&f.key, CREATOR);
        let v = router_json_value(&f.app, "/api/guilds/g1/series/1/settings", &creator).await;
        assert!(v["reminder_error"].is_null());
        assert!(v["reminder_error_at"].is_null());

        SeriesRepo::new(f.pool.clone())
            .set_reminder_error(1, Some(ReminderFailureKind::DmClosed.as_str()), 1234)
            .await
            .unwrap();
        let v = router_json_value(&f.app, "/api/guilds/g1/series/1/settings", &creator).await;
        assert_eq!(v["reminder_error"], "dm_closed");
        assert_eq!(v["reminder_error_at"], 1234);
        let mine = router_json(&f.app, "/api/guilds/g1/series/mine", &creator).await;
        assert_eq!(mine[0]["reminder_error"], "dm_closed");
        assert!(mine[1]["reminder_error"].is_null());
    }

    // --- admin panel: names, pickers, validation, sprouts, log lines ---

    #[tokio::test]
    async fn admin_detail_names_the_server_creators_and_roles() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        let guilds = router_json(&app, "/api/admin/guilds", &tok).await;
        assert_eq!(guilds[0]["name"], "Test Guild");
        assert_eq!(
            guilds[0]["icon_url"],
            "https://cdn.discordapp.com/icons/g1/abc123.png?size=64"
        );

        let detail = router_json_value(&app, "/api/admin/guilds/g1", &tok).await;
        assert_eq!(detail["name"], "Test Guild");
        assert_eq!(detail["setup_complete"], true);
        let rows = detail["series"].as_array().unwrap();
        assert_eq!(rows[0]["creator_name"], "creator1 (nick)");
        assert_eq!(rows[0]["archived_days"], 1);
        assert_eq!(rows[1]["privacy_role_name"], "VIP");
        assert!(rows[0]["privacy_role_name"].is_null());
    }

    #[tokio::test]
    async fn admin_options_offer_roles_and_text_channels_of_managed_guilds_only() {
        let (app, key, _calls) = app().await;
        let tok = admin_token(&key, CREATOR, &[GUILD]);
        let v = router_json_value(&app, "/api/admin/guilds/g1/options", &tok).await;
        let names = |list: &serde_json::Value| -> Vec<String> {
            list.as_array()
                .unwrap()
                .iter()
                .map(|x| x["name"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(names(&v["roles"]), ["VIP", "Regular"]);
        // Sidebar order, text and announcement only (no voice channel).
        assert_eq!(names(&v["channels"]), ["daily-photos", "sketches", "news"]);
        assert_eq!(v["roles_unavailable"], false);
        assert_eq!(v["channels_unavailable"], false);

        assert_eq!(
            get(&app, "/api/admin/guilds/other/options", Some(&tok)).await,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&app, "/api/admin/guilds/g1/options", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    #[tokio::test]
    async fn admin_settings_are_validated_before_they_are_stored() {
        let f = fixture().await;
        let tok = admin_token(&f.key, CREATOR, &[GUILD]);
        let path = "/api/admin/guilds/g1/settings";
        for (body, code) in [
            (r#"{"timezone":"Mars/Olympus"}"#, "invalid_timezone"),
            (r#"{"max_series_per_user":0}"#, "invalid_limit"),
            (r#"{"min_account_age_days":-1}"#, "invalid_limit"),
            (r#"{"sprout_threshold":0}"#, "invalid_limit"),
            // A number cannot be cleared: null is refused, not ignored.
            (r#"{"max_series_per_user":null}"#, "invalid_limit"),
            (r#"{"sprout_threshold":null}"#, "invalid_limit"),
            (r#"{"creator_role_id":"12345"}"#, "unknown_role"),
            // A channel id from somewhere else, and a voice channel here.
            (r#"{"log_channel_id":"999000"}"#, "unknown_channel"),
            (r#"{"log_channel_id":"v1"}"#, "unknown_channel"),
        ] {
            let (status, v) = patch_json(&f.app, path, &tok, body).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
            assert_eq!(v["error"], code, "{body}");
            assert!(v["message"].is_string(), "{body}");
        }
        // Nothing above was stored.
        let stored = GuildSettingsRepo::new(f.pool.clone())
            .get(GUILD)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.timezone, "America/Chicago");
        assert_eq!(stored.max_series_per_user, 10);
        assert_eq!(stored.log_channel_id, None);
        assert_eq!(stored.creator_role_id, None);

        // Good values are stored; the timezone in its canonical case.
        let (status, v) = patch_json(
            &f.app,
            path,
            &tok,
            r#"{"timezone":"europe/paris","log_channel_id":"n1","creator_role_id":"role-vip","min_account_age_days":0}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["timezone"], "Europe/Paris");
        assert_eq!(v["log_channel_id"], "n1");
        assert_eq!(v["creator_role_id"], "role-vip");
        assert_eq!(v["sprouts_published"], 0);
        // An empty string clears.
        let (_, v) = patch_json(&f.app, path, &tok, r#"{"log_channel_id":""}"#).await;
        assert!(v["log_channel_id"].is_null());
    }

    #[tokio::test]
    async fn sprout_settings_publish_the_sprouts_they_release() {
        let f = fixture().await;
        let tok = admin_token(&f.key, CREATOR, &[GUILD]);
        let path = "/api/admin/guilds/g1/settings";
        // Series 4 is a sprout with nothing archived. A threshold it has not
        // reached releases nothing…
        let (status, v) = patch_json(
            &f.app,
            path,
            &tok,
            r#"{"sprout_enabled":true,"sprout_threshold":2}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["sprouts_published"], 0);
        // …a save that does not touch the stage never looks…
        let (_, v) = patch_json(&f.app, path, &tok, r#"{"max_series_per_user":4}"#).await;
        assert_eq!(v["sprouts_published"], 0);
        // …and switching the stage off publishes it.
        let (_, v) = patch_json(&f.app, path, &tok, r#"{"sprout_enabled":false}"#).await;
        assert_eq!(v["sprouts_published"], 1);
        let detail = router_json_value(&f.app, "/api/admin/guilds/g1", &tok).await;
        assert_eq!(detail["series"][3]["state"], "active");
        // The revoked series was not touched.
        assert_eq!(detail["series"][4]["state"], "revoked");
    }

    #[tokio::test]
    async fn admin_series_changes_follow_the_sprout_rules_and_are_logged() {
        let f = fixture().await;
        let guilds = GuildSettingsRepo::new(f.pool.clone());
        let mut gs = guilds.get(GUILD).await.unwrap().unwrap();
        gs.log_channel_id = Some("c2".to_owned());
        gs.sprout_enabled = true;
        gs.sprout_threshold = 3;
        guilds.upsert(&gs).await.unwrap();
        let tok = admin_token(&f.key, CREATOR, &[GUILD]);
        let lines = || -> Vec<String> {
            f.sent
                .lock()
                .unwrap()
                .iter()
                .map(|(channel, line)| format!("{channel}: {line}"))
                .collect()
        };

        // Role-gated needs a role, and the role must be this guild's.
        let series = "/api/admin/guilds/g1/series/1";
        let (status, v) = patch_json(&f.app, series, &tok, r#"{"privacy":"role_gated"}"#).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "role_required");
        let (status, v) = patch_json(
            &f.app,
            series,
            &tok,
            r#"{"privacy":"role_gated","privacy_role_id":"elsewhere"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "unknown_role");
        let (status, v) = patch_json(
            &f.app,
            series,
            &tok,
            r#"{"privacy":"role_gated","privacy_role_id":"role-vip"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(v["privacy_role_name"], "VIP");
        let (_, v) = patch_json(&f.app, series, &tok, r#"{"privacy":"public"}"#).await;
        assert_eq!(v["privacy"], "public");
        assert!(lines().is_empty(), "privacy edits write no log line");

        // Revoke, named as the series was known until now.
        let (_, v) = patch_json(&f.app, series, &tok, r#"{"state":"revoked"}"#).await;
        assert_eq!(v["state"], "revoked");
        assert_eq!(lines(), ["c2: 🚫 **public** · revoked by <@creator1>"]);
        // Revoking again changes nothing and says nothing.
        patch_json(&f.app, series, &tok, r#"{"state":"revoked"}"#).await;
        assert_eq!(lines().len(), 1);

        // Restore: one archived day of three, so it goes back to the sprout
        // stage and is not named.
        let (_, v) = patch_json(&f.app, series, &tok, r#"{"state":"active"}"#).await;
        assert_eq!(v["state"], "sprout");
        assert_eq!(v["archived_days"], 1);
        assert_eq!(
            lines().last().unwrap(),
            "c2: 🌿 A private series · restored by <@creator1>"
        );
        // Publish now: active, whatever the threshold.
        let (_, v) = patch_json(&f.app, series, &tok, r#"{"state":"active"}"#).await;
        assert_eq!(v["state"], "active");
        assert_eq!(
            lines().last().unwrap(),
            "c2: 🌿 **public** · published by <@creator1>"
        );

        // A stored log channel that is not this guild's gets nothing.
        gs.log_channel_id = Some("another-guilds-channel".to_owned());
        guilds.upsert(&gs).await.unwrap();
        let before = lines().len();
        patch_json(&f.app, series, &tok, r#"{"state":"revoked"}"#).await;
        assert_eq!(lines().len(), before);
    }

    // --- helpers that return parsed JSON ---
    /// A set-up guild of its own, so a test that changes what Discord lists
    /// does not share the process-wide role cache with the other tests.
    async fn own_guild(f: &Fixture, gid: &str) {
        let guilds = GuildSettingsRepo::new(f.pool.clone());
        guilds.ensure_exists(gid).await.unwrap();
        let mut settings = leaf_core::domain::GuildSettings::defaults_for(gid);
        settings.setup_complete = true;
        settings.watched_channels = vec!["c1".to_owned()];
        guilds.upsert(&settings).await.unwrap();
    }

    fn seeded(
        gid: &str,
        creator: &str,
        name: &str,
        privacy: Privacy,
        role: Option<&str>,
    ) -> NewSeries {
        NewSeries {
            guild_id: gid.to_owned(),
            creator_id: creator.to_owned(),
            name: name.to_owned(),
            description: String::new(),
            channels: vec!["c1".to_owned()],
            cadence: Cadence::Daily,
            detection_mode: DetectionMode::ContextMenu,
            privacy,
            privacy_role_id: role.map(ToOwned::to_owned),
            start_day: 1,
            state: SeriesState::Active,
        }
    }

    #[tokio::test]
    async fn a_state_change_does_not_depend_on_the_stored_privacy() {
        let f = fixture().await;
        let tok = admin_token(&f.key, CREATOR, &[GUILD]);
        // What the panel used to store: role-gated with no role.
        let legacy = SeriesRepo::new(f.pool.clone())
            .create(
                &seeded(GUILD, CREATOR, "legacy", Privacy::RoleGated, None),
                0,
            )
            .await
            .unwrap();
        let path = format!("/api/admin/guilds/g1/series/{}", legacy.id);

        for (body, state) in [
            (r#"{"state":"revoked"}"#, "revoked"),
            (r#"{"state":"active"}"#, "active"),
        ] {
            let (status, v) = patch_json(&f.app, &path, &tok, body).await;
            assert_eq!(status, StatusCode::OK, "{body}: {v}");
            assert_eq!(v["state"], state);
        }
        // An edit of the privacy itself is still held to the rule.
        let (status, v) = patch_json(&f.app, &path, &tok, r#"{"privacy":"role_gated"}"#).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "role_required");
    }

    #[tokio::test]
    async fn a_role_or_channel_made_after_the_list_was_cached_is_accepted() {
        let f = fixture().await;
        let gid = "g-fresh";
        own_guild(&f, gid).await;
        let tok = admin_token(&f.key, CREATOR, &[gid]);
        let settings = format!("/api/admin/guilds/{gid}/settings");
        let options = format!("/api/admin/guilds/{gid}/options");

        // The panel loads: both lists are now cached.
        let before = router_json_value(&f.app, &options, &tok).await;
        assert_eq!(before["roles"].as_array().unwrap().len(), 2);

        // An admin makes a role and a channel in Discord.
        f.extra_roles.lock().unwrap().push(auth::GuildRole {
            id: "role-new".to_owned(),
            name: "Newcomer".to_owned(),
            position: 9,
        });
        f.extra_channels.lock().unwrap().push(auth::GuildChannel {
            id: "c-new".to_owned(),
            name: "leaf-log".to_owned(),
            kind: auth::CHANNEL_KIND_TEXT,
            position: 9,
        });

        // Their ids are accepted although the cached lists predate them.
        let (status, v) = patch_json(
            &f.app,
            &settings,
            &tok,
            r#"{"creator_role_id":"role-new","log_channel_id":"c-new"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{v}");
        assert_eq!(v["creator_role_id"], "role-new");
        assert_eq!(v["log_channel_id"], "c-new");

        // And a reload of the panel offers them.
        let after = router_json_value(&f.app, &options, &tok).await;
        assert_eq!(after["roles"][0]["name"], "Newcomer");
        assert!(
            after["channels"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == "c-new")
        );

        // An id Discord does not list is still refused.
        let (status, v) =
            patch_json(&f.app, &settings, &tok, r#"{"creator_role_id":"nope"}"#).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(v["error"], "unknown_role");
    }

    #[tokio::test]
    async fn a_new_series_channel_is_named_in_the_create_form() {
        let f = fixture().await;
        let gid = "g-fresh-channels";
        own_guild(&f, gid).await;
        let guilds = GuildSettingsRepo::new(f.pool.clone());
        let admin = admin_token(&f.key, CREATOR, &[gid]);
        // Something loads the channel list before the channel exists.
        router_json_value(&f.app, &format!("/api/admin/guilds/{gid}/options"), &admin).await;

        // The channel is made and chosen with /setup.
        f.extra_channels.lock().unwrap().push(auth::GuildChannel {
            id: "c-new".to_owned(),
            name: "comics".to_owned(),
            kind: auth::CHANNEL_KIND_TEXT,
            position: 9,
        });
        let mut settings = guilds.get(gid).await.unwrap().unwrap();
        settings.watched_channels.push("c-new".to_owned());
        guilds.upsert(&settings).await.unwrap();

        let member = token_for(&f.key, MEMBER);
        let v = router_json_value(
            &f.app,
            &format!("/api/guilds/{gid}/series/options"),
            &member,
        )
        .await;
        let named: Vec<_> = v["channels"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| (c["id"].as_str().unwrap(), c["name"].as_str()))
            .collect();
        assert_eq!(
            named,
            [("c1", Some("daily-photos")), ("c-new", Some("comics"))]
        );
    }

    /// A series of `CREATOR`'s in guild `gid` that posts in `channels`.
    async fn series_in(f: &Fixture, gid: &str, name: &str, channels: &[&str]) -> i64 {
        let mut new = seeded(gid, CREATOR, name, Privacy::Public, None);
        new.channels = channels.iter().map(|c| (*c).to_owned()).collect();
        SeriesRepo::new(f.pool.clone())
            .create(&new, 0)
            .await
            .unwrap()
            .id
    }

    #[tokio::test]
    async fn a_deleted_series_channel_has_no_name_and_is_flagged_missing() {
        let f = fixture().await;
        let gid = "g-gone-channel";
        own_guild(&f, gid).await;
        series_in(&f, gid, "Daily", &["c1"]).await;
        series_in(&f, gid, "Unbound", &[]).await;
        let creator = token_for(&f.key, CREATOR);
        let mine = format!("/api/guilds/{gid}/series/mine");
        let options = format!("/api/guilds/{gid}/series/options");
        let row = |list: &[serde_json::Value], name: &str| -> serde_json::Value {
            list.iter().find(|s| s["name"] == name).unwrap().clone()
        };

        // While the channel is there it is named, and not missing.
        let before = router_json(&f.app, &mine, &creator).await;
        let daily = row(&before, "Daily");
        assert_eq!(daily["channel_id"], "c1");
        assert_eq!(daily["channel_name"], "daily-photos");
        assert_eq!(daily["channel_missing"], false);
        // A series with no channel has none to miss.
        let unbound = row(&before, "Unbound");
        assert!(unbound["channel_id"].is_null() && unbound["channel_name"].is_null());
        assert_eq!(unbound["channel_missing"], false);

        // The channel is deleted in Discord, and the few seconds a list is
        // good for names go by. The list kept for checking ids still has
        // the channel, under its old name, for minutes.
        f.gone_channels.lock().unwrap().push("c1".to_owned());
        state::channel_names_cache().invalidate(gid).await;
        let kept = f.state.channels.get(gid).await.unwrap();
        assert!(
            kept.iter()
                .any(|c| c.id == "c1" && c.name == "daily-photos")
        );

        // The old name is never served: no name, and the flag.
        let after = router_json(&f.app, &mine, &creator).await;
        let daily = row(&after, "Daily");
        assert_eq!(daily["channel_id"], "c1");
        assert!(daily["channel_name"].is_null(), "{daily}");
        assert_eq!(daily["channel_missing"], true);
        assert_eq!(row(&after, "Unbound")["channel_missing"], false);
        // The create form and Series settings say the same of it.
        let v = router_json_value(&f.app, &options, &creator).await;
        assert_eq!(
            v["channels"],
            serde_json::json!([{ "id": "c1", "name": null }])
        );

        // The channel is back (or leaf can see it again): named again.
        f.gone_channels.lock().unwrap().clear();
        state::channel_names_cache().invalidate(gid).await;
        let back = router_json(&f.app, &mine, &creator).await;
        let daily = row(&back, "Daily");
        assert_eq!(daily["channel_name"], "daily-photos");
        assert_eq!(daily["channel_missing"], false);
    }

    #[tokio::test]
    async fn a_name_is_never_taken_from_a_list_cached_before_the_channel_was_deleted() {
        let f = fixture().await;
        let gid = "g-gone-channel-cached";
        own_guild(&f, gid).await;
        // Something caches the channel list while the channel exists.
        let admin = admin_token(&f.key, CREATOR, &[gid]);
        router_json_value(&f.app, &format!("/api/admin/guilds/{gid}/options"), &admin).await;
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), 1);

        // The channel is deleted; nothing has asked for names yet.
        f.gone_channels.lock().unwrap().push("c1".to_owned());
        let v = current_names(&f, gid, &["c1", "c2"]).await;

        // Discord was asked again rather than the cached list trusted. Once:
        // an answer that fresh cannot be missing a channel for being old.
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), 2);
        let expected = [
            ("c1".to_owned(), None),
            ("c2".to_owned(), Some("sketches".to_owned())),
        ];
        assert_eq!(v, expected);
        // Asking again within seconds says the same. The list is no longer
        // brand new, so the absent channel is looked for once more, and
        // then no more.
        assert_eq!(current_names(&f, gid, &["c1", "c2"]).await, expected);
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), 3);
        assert_eq!(current_names(&f, gid, &["c1", "c2"]).await, expected);
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn a_channel_made_seconds_after_names_were_asked_for_is_named_at_once() {
        let f = fixture().await;
        let gid = "g-names-new";
        own_guild(&f, gid).await;
        // The gallery has just asked for names: the list is seconds old.
        assert_eq!(
            current_names(&f, gid, &["c1"]).await,
            [("c1".to_owned(), Some("daily-photos".to_owned()))]
        );

        // A channel is made and chosen with /setup.
        f.extra_channels.lock().unwrap().push(auth::GuildChannel {
            id: "c-new".to_owned(),
            name: "comics".to_owned(),
            kind: auth::CHANNEL_KIND_TEXT,
            position: 9,
        });

        // It is not called missing for being newer than the list.
        let st = f.state.clone();
        let current = guild_channels_naming(&st, gid, ["c1", "c-new"].into_iter()).await;
        let made = NamedChannel::of("c-new", current.as_ref().map(|list| list.as_slice()));
        assert_eq!(made.name.as_deref(), Some("comics"));
        assert!(!made.missing);
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), 2);
    }

    /// What the naming lookup makes of `ids` in guild `gid`: each with its
    /// current name.
    async fn current_names(f: &Fixture, gid: &str, ids: &[&str]) -> Vec<(String, Option<String>)> {
        let st = f.state.clone();
        let current = guild_channels_naming(&st, gid, ids.iter().copied()).await;
        ids.iter()
            .map(|id| {
                let channel = NamedChannel::of(id, current.as_ref().map(|list| list.as_slice()));
                ((*id).to_owned(), channel.name)
            })
            .collect()
    }

    #[tokio::test]
    async fn a_channel_cannot_be_called_missing_when_discord_cannot_be_asked() {
        let f = fixture().await;
        let gid = "g-names-down";
        own_guild(&f, gid).await;
        series_in(&f, gid, "Daily", &["c1"]).await;
        let creator = token_for(&f.key, CREATOR);
        // The list kept for checking ids has the channel, from a minute ago.
        let admin = admin_token(&f.key, CREATOR, &[gid]);
        router_json_value(&f.app, &format!("/api/admin/guilds/{gid}/options"), &admin).await;
        assert!(f.state.channels.get(gid).await.is_some());
        f.channels_down.store(true, Ordering::Relaxed);

        // No current list: no name, not even the one from a minute ago, and
        // nothing is said about the channel.
        let mine = router_json(&f.app, &format!("/api/guilds/{gid}/series/mine"), &creator).await;
        assert_eq!(mine[0]["channel_id"], "c1");
        assert!(mine[0]["channel_name"].is_null());
        assert_eq!(mine[0]["channel_missing"], false);
        let v = router_json_value(
            &f.app,
            &format!("/api/guilds/{gid}/series/options"),
            &creator,
        )
        .await;
        assert_eq!(
            v["channels"],
            serde_json::json!([{ "id": "c1", "name": null }])
        );

        // A failure is not remembered: the next request asks again.
        f.channels_down.store(false, Ordering::Relaxed);
        let mine = router_json(&f.app, &format!("/api/guilds/{gid}/series/mine"), &creator).await;
        assert_eq!(mine[0]["channel_name"], "daily-photos");
        assert_eq!(mine[0]["channel_missing"], false);
    }

    #[tokio::test]
    async fn a_burst_of_requests_for_names_costs_one_discord_call() {
        let f = fixture().await;
        let gid = "g-names-burst";
        own_guild(&f, gid).await;
        series_in(&f, gid, "Daily", &["c1"]).await;
        let creator = token_for(&f.key, CREATOR);
        let mine = format!("/api/guilds/{gid}/series/mine");
        let options = format!("/api/guilds/{gid}/series/options");

        for _ in 0..3 {
            router_json(&f.app, &mine, &creator).await;
            router_json_value(&f.app, &options, &creator).await;
        }
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), 1);

        // Nothing to name, nothing asked: a member with no series.
        let before = f.channel_lists.load(Ordering::Relaxed);
        let st = f.state.clone();
        let nothing = std::iter::empty::<&str>();
        assert!(
            guild_channels_naming(&st, "g-names-nothing", nothing)
                .await
                .is_none()
        );
        assert_eq!(f.channel_lists.load(Ordering::Relaxed), before);
    }

    #[test]
    fn a_stored_channel_is_named_only_from_the_current_list() {
        let list = [GuildChannel {
            id: "c1".to_owned(),
            name: "daily-photos".to_owned(),
            kind: auth::CHANNEL_KIND_TEXT,
            position: 1,
        }];
        let there = NamedChannel::of("c1", Some(&list));
        assert_eq!(there.name.as_deref(), Some("daily-photos"));
        assert!(!there.missing);
        // Not in the list Discord gave: gone, or hidden from leaf.
        let gone = NamedChannel::of("c9", Some(&list));
        assert_eq!(gone.name, None);
        assert!(gone.missing);
        // No list: nothing is known about the channel either way.
        let unknown = NamedChannel::of("c1", None);
        assert_eq!(unknown.name, None);
        assert!(!unknown.missing);
    }

    #[tokio::test]
    async fn slow_creator_lookups_do_not_hold_up_the_admin_page() {
        let f = fixture().await;
        let gid = "g-slow";
        own_guild(&f, gid).await;
        let series = SeriesRepo::new(f.pool.clone());
        series
            .create(&seeded(gid, SLOW_USER, "slow", Privacy::Public, None), 0)
            .await
            .unwrap();
        series
            .create(&seeded(gid, "quick", "quick", Privacy::Public, None), 0)
            .await
            .unwrap();
        let tok = admin_token(&f.key, CREATOR, &[gid]);

        let began = std::time::Instant::now();
        let detail = router_json_value(&f.app, &format!("/api/admin/guilds/{gid}"), &tok).await;
        assert!(
            began.elapsed() < std::time::Duration::from_secs(10),
            "the page waited for Discord"
        );
        let rows = detail["series"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        // The name Discord did not supply in time is simply missing; the
        // one it did supply is there.
        for row in rows {
            if row["name"] == "slow" {
                assert!(row["creator_name"].is_null(), "{row}");
            } else {
                assert_eq!(row["creator_name"], "quick (nick)");
            }
        }
    }

    #[tokio::test]
    async fn a_role_is_checked_before_a_series_can_be_gated_on_it() {
        let f = fixture().await;
        let member = token_for(&f.key, MEMBER);

        // A role sent with a public series is not kept.
        let (status, v) = post_json(
            &f.app,
            "/api/guilds/g1/series",
            &member,
            r#"{"name":"Sneaky","channel_id":"c1","cadence":"daily","privacy":"public","privacy_role_id":"[x](https://y.example)"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
        let id = v["id"].as_i64().unwrap();
        let stored = SeriesRepo::new(f.pool.clone())
            .get(id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.privacy_role_id, None);
        // So gating it later needs a real role.
        let path = format!("/api/guilds/g1/series/{id}");
        let (status, v) = patch_json(&f.app, &path, &member, r#"{"privacy":"role_gated"}"#).await;
        assert_ne!(status, StatusCode::OK, "{v}");

        // A series from before this rule, with an unchecked role stored:
        // becoming role-gated checks that role against the server.
        let legacy = SeriesRepo::new(f.pool.clone())
            .create(
                &seeded(GUILD, MEMBER, "old", Privacy::Public, Some("not-a-role")),
                0,
            )
            .await
            .unwrap();
        let path = format!("/api/guilds/g1/series/{}", legacy.id);
        let (status, v) = patch_json(&f.app, &path, &member, r#"{"privacy":"role_gated"}"#).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
        assert_eq!(v["error"], "unknown_role");
        // With a role of this server it goes through.
        let (status, v) = patch_json(
            &f.app,
            &path,
            &member,
            r#"{"privacy":"role_gated","privacy_role_id":"role-vip"}"#,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{v}");
    }

    async fn router_json(router: &Router, path: &str, bearer: &str) -> Vec<serde_json::Value> {
        let v = router_json_value(router, path, bearer).await;
        v.as_array().unwrap().clone()
    }

    async fn router_json_value(router: &Router, path: &str, bearer: &str) -> serde_json::Value {
        let resp = router
            .clone()
            .oneshot(
                Request::get(path)
                    .header("Authorization", format!("Bearer {bearer}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }
}
