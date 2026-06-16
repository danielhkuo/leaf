//! The admin web panel's backend.
//!
//! A browser OAuth login proves the caller manages a guild (Manage-Guild via
//! the `guilds` scope), after which an [`AdminUser`] token authorizes editing
//! that guild's settings and series. Distinct from the gallery API: a
//! different token (`verify_admin`) and a different gate (Manage-Guild, not
//! membership). State-changing routes are `PATCH`; everything is guild-scoped
//! to the set baked into the token at login.

use axum::Json;
use axum::Router;
use axum::extract::{FromRef, FromRequestParts, Path, Query, State};
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::response::Redirect;
use axum::routing::{get, patch};
use leaf_core::domain::{GuildSettings, Privacy, Series, SeriesState};
use serde::{Deserialize, Serialize};

use crate::api::auth::{AdminClaims, DiscordApi, SessionKey, now_unix};
use crate::api::error::ApiError;
use crate::api::state::ApiState;

/// Admin session lifetime — short; the panel is for occasional changes.
const ADMIN_TTL_SECS: i64 = 3600;
/// How long a login may take to come back with its `state`.
const OAUTH_STATE_TTL_SECS: i64 = 600;
/// OAuth scopes: identity plus the guild list (with per-guild permissions).
const SCOPES: &str = "identify guilds";

/// Builds the admin routes (browser OAuth + the JSON admin API).
pub fn router<D: DiscordApi>() -> Router<ApiState<D>> {
    Router::new()
        .route("/admin/login", get(login::<D>))
        .route("/admin/callback", get(callback::<D>))
        .route("/api/admin/guilds", get(list_guilds::<D>))
        .route("/api/admin/guilds/{gid}", get(guild_detail::<D>))
        .route(
            "/api/admin/guilds/{gid}/settings",
            patch(patch_settings::<D>),
        )
        .route(
            "/api/admin/guilds/{gid}/series/{sid}",
            patch(patch_series::<D>),
        )
}

/// The authenticated admin, from the `Authorization: Bearer` admin token.
pub struct AdminUser(pub AdminClaims);

impl AdminUser {
    /// Asserts the admin manages `gid` (else hides it as not-found).
    fn require_guild(&self, gid: &str) -> Result<(), ApiError> {
        if self.0.guild_ids.iter().any(|g| g == gid) {
            Ok(())
        } else {
            // Not-found, not forbidden: don't confirm a guild exists.
            Err(ApiError::NotFound)
        }
    }
}

impl<S> FromRequestParts<S> for AdminUser
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
        let claims = key
            .verify_admin(token.trim(), now_unix())
            .map_err(|_| ApiError::Unauthorized)?;
        Ok(Self(claims))
    }
}

// --- browser OAuth -------------------------------------------------------

fn admin_redirect_uri(public_url: &str) -> String {
    format!("{}/admin/callback", public_url.trim_end_matches('/'))
}

/// RFC-3986 percent-encoding for query values (avoids a URL-crate dependency).
fn pct(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b));
            }
            // write! to a String is infallible.
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

/// `GET /admin/login` — redirect to Discord's consent screen.
async fn login<D: DiscordApi>(State(st): State<ApiState<D>>) -> Redirect {
    let state = st.key.sign_oauth_state(now_unix(), OAUTH_STATE_TTL_SECS);
    let redirect = admin_redirect_uri(&st.redirect_uri);
    let url = format!(
        "https://discord.com/oauth2/authorize?response_type=code\
         &client_id={cid}&scope={scope}&redirect_uri={redirect}&state={state}&prompt=none",
        cid = pct(&st.client_id),
        scope = pct(SCOPES),
        redirect = pct(&redirect),
        state = pct(&state),
    );
    Redirect::to(&url)
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
}

/// `GET /admin/callback` — exchange the code, mint an admin token, hand it to
/// the SPA via the URL fragment (kept out of logs/referrers).
async fn callback<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Query(q): Query<CallbackQuery>,
) -> Result<Redirect, ApiError> {
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Err(ApiError::BadRequest);
    };
    if !st.key.verify_oauth_state(&state, now_unix()) {
        return Err(ApiError::Unauthorized);
    }
    let redirect = admin_redirect_uri(&st.redirect_uri);
    let access = st
        .discord
        .exchange_code(&code, &redirect)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "admin oauth exchange failed");
            ApiError::BadRequest
        })?;
    let user_id = st
        .discord
        .current_user_id(&access)
        .await
        .map_err(|_| ApiError::Internal)?;
    let manageable = st
        .discord
        .managed_guild_ids(&access)
        .await
        .map_err(|_| ApiError::Internal)?;

    // Keep only guilds leaf is actually in, so the token (and panel) shows
    // only what's actionable.
    let mut guilds = Vec::new();
    for gid in manageable {
        if st.guilds.get(&gid).await?.is_some() {
            guilds.push(gid);
        }
    }
    if guilds.is_empty() {
        return Err(ApiError::Forbidden);
    }

    let token = st
        .key
        .mint_admin(&user_id, &guilds, now_unix(), ADMIN_TTL_SECS);
    Ok(Redirect::to(&format!("/admin#token={token}")))
}

// --- admin API -----------------------------------------------------------

#[derive(Serialize)]
struct AdminGuildDto {
    guild_id: String,
    series_count: usize,
}

#[derive(Serialize)]
struct AdminSettingsDto {
    timezone: String,
    creator_role_id: Option<String>,
    log_channel_id: Option<String>,
    max_series_per_user: i64,
    min_account_age_days: i64,
    min_membership_age_days: i64,
    sprout_enabled: bool,
    sprout_threshold: i64,
}

impl From<&GuildSettings> for AdminSettingsDto {
    fn from(s: &GuildSettings) -> Self {
        Self {
            timezone: s.timezone.clone(),
            creator_role_id: s.creator_role_id.clone(),
            log_channel_id: s.log_channel_id.clone(),
            max_series_per_user: s.max_series_per_user,
            min_account_age_days: s.min_account_age_days,
            min_membership_age_days: s.min_membership_age_days,
            sprout_enabled: s.sprout_enabled,
            sprout_threshold: s.sprout_threshold,
        }
    }
}

#[derive(Serialize)]
struct AdminSeriesDto {
    id: i64,
    name: String,
    creator_id: String,
    privacy: String,
    privacy_role_id: Option<String>,
    state: String,
}

impl From<&Series> for AdminSeriesDto {
    fn from(s: &Series) -> Self {
        Self {
            id: s.id,
            name: s.name.clone(),
            creator_id: s.creator_id.clone(),
            privacy: s.privacy.as_str().to_owned(),
            privacy_role_id: s.privacy_role_id.clone(),
            state: s.state.as_str().to_owned(),
        }
    }
}

#[derive(Serialize)]
struct AdminGuildDetailDto {
    guild_id: String,
    settings: AdminSettingsDto,
    series: Vec<AdminSeriesDto>,
}

/// `GET /api/admin/guilds` — the guilds this admin manages that have leaf.
async fn list_guilds<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    admin: AdminUser,
) -> Result<Json<Vec<AdminGuildDto>>, ApiError> {
    let mut out = Vec::new();
    for gid in &admin.0.guild_ids {
        let count = st.series.list_by_guild(gid).await?.len();
        out.push(AdminGuildDto {
            guild_id: gid.clone(),
            series_count: count,
        });
    }
    Ok(Json(out))
}

/// `GET /api/admin/guilds/{gid}` — that guild's settings and series.
async fn guild_detail<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    admin: AdminUser,
) -> Result<Json<AdminGuildDetailDto>, ApiError> {
    admin.require_guild(&gid)?;
    let settings = st.guilds.get(&gid).await?.ok_or(ApiError::NotFound)?;
    let series = st.series.list_by_guild(&gid).await?;
    Ok(Json(AdminGuildDetailDto {
        guild_id: gid,
        settings: (&settings).into(),
        series: series.iter().map(AdminSeriesDto::from).collect(),
    }))
}

#[derive(Deserialize)]
struct SettingsPatch {
    timezone: Option<String>,
    /// Empty string clears the field.
    creator_role_id: Option<String>,
    /// Empty string clears the field.
    log_channel_id: Option<String>,
    max_series_per_user: Option<i64>,
    min_account_age_days: Option<i64>,
    min_membership_age_days: Option<i64>,
    sprout_enabled: Option<bool>,
    sprout_threshold: Option<i64>,
}

fn none_if_empty(s: String) -> Option<String> {
    (!s.is_empty()).then_some(s)
}

fn apply_settings(s: &mut GuildSettings, p: SettingsPatch) {
    if let Some(v) = p.timezone {
        s.timezone = v;
    }
    if let Some(v) = p.creator_role_id {
        s.creator_role_id = none_if_empty(v);
    }
    if let Some(v) = p.log_channel_id {
        s.log_channel_id = none_if_empty(v);
    }
    if let Some(v) = p.max_series_per_user {
        s.max_series_per_user = v;
    }
    if let Some(v) = p.min_account_age_days {
        s.min_account_age_days = v;
    }
    if let Some(v) = p.min_membership_age_days {
        s.min_membership_age_days = v;
    }
    if let Some(v) = p.sprout_enabled {
        s.sprout_enabled = v;
    }
    if let Some(v) = p.sprout_threshold {
        s.sprout_threshold = v;
    }
}

/// `PATCH /api/admin/guilds/{gid}/settings` — partial settings update.
async fn patch_settings<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    admin: AdminUser,
    Json(p): Json<SettingsPatch>,
) -> Result<Json<AdminSettingsDto>, ApiError> {
    admin.require_guild(&gid)?;
    let mut settings = st.guilds.get(&gid).await?.ok_or(ApiError::NotFound)?;
    apply_settings(&mut settings, p);
    st.guilds.upsert(&settings).await?;
    Ok(Json((&settings).into()))
}

#[derive(Deserialize)]
struct SeriesPatch {
    privacy: Option<String>,
    /// Empty string clears the role.
    privacy_role_id: Option<String>,
    state: Option<String>,
}

/// `PATCH /api/admin/guilds/{gid}/series/{sid}` — edit privacy / revoke.
async fn patch_series<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    admin: AdminUser,
    Json(p): Json<SeriesPatch>,
) -> Result<Json<AdminSeriesDto>, ApiError> {
    admin.require_guild(&gid)?;
    let mut series = st.series.get(sid).await?.ok_or(ApiError::NotFound)?;
    if series.guild_id != gid {
        return Err(ApiError::NotFound);
    }

    // privacy / role live on the series row (`update`); state via `set_state`.
    let edits_row = p.privacy.is_some() || p.privacy_role_id.is_some();
    if let Some(privacy) = p.privacy {
        series.privacy = privacy
            .parse::<Privacy>()
            .map_err(|_| ApiError::BadRequest)?;
    }
    if let Some(role) = p.privacy_role_id {
        series.privacy_role_id = none_if_empty(role);
    }
    if edits_row {
        st.series.update(&series).await?;
    }
    if let Some(state) = p.state {
        let new_state = state
            .parse::<SeriesState>()
            .map_err(|_| ApiError::BadRequest)?;
        st.series.set_state(series.id, new_state).await?;
        series.state = new_state;
    }
    Ok(Json(AdminSeriesDto::from(&series)))
}
