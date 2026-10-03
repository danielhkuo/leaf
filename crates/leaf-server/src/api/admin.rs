//! The admin web panel's backend.
//!
//! A browser OAuth login proves the caller manages a guild (Manage-Guild via
//! the `guilds` scope), after which an [`AdminUser`] token authorizes editing
//! that guild's settings and series. Distinct from the gallery API: a
//! different token (`verify_admin`) and a different gate (Manage-Guild, not
//! membership). State-changing routes are `PATCH`; everything is guild-scoped
//! to the set baked into the token at login, and every guild route checks
//! that set first.
//!
//! Settings are validated here, not only in the page: the timezone must be
//! one leaf knows, the limits must be in range, and a role or channel id must
//! belong to this guild. The last rule is a security boundary, because the
//! bot posts log lines (series names, who did what) to whatever channel is
//! stored.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{FromRef, FromRequestParts, Path, Query, State};
use axum::http::request::Parts;
use axum::response::Redirect;
use axum::routing::{get, patch};
use leaf_core::db::PanelChange;
use leaf_core::domain::{GuildSettings, Privacy, Series, SeriesState};
use leaf_core::series_ops;
use serde::{Deserialize, Serialize};

use crate::api::auth::{
    AdminClaims, DiscordApi, ExchangeError, GuildRole, GuildSummary, SessionKey, now_unix,
};
use crate::api::error::ApiError;
use crate::api::state::{self, ApiJson, ApiState};
use crate::api::{
    fresh_guild_channels, fresh_guild_roles, guild_channels, guild_roles, require_role,
};

/// Admin session lifetime — short; the panel is for occasional changes.
const ADMIN_TTL_SECS: i64 = 3600;
/// How long a login may take to come back with its `state`.
const OAUTH_STATE_TTL_SECS: i64 = 600;
/// OAuth scopes: identity plus the guild list (with per-guild permissions).
const SCOPES: &str = "identify guilds";
/// Upper bound of every numeric setting (the same cap as a day number).
const MAX_NUMBER: i64 = 999_999;
/// How many creator-name lookups run against Discord at once.
const NAME_LOOKUPS_AT_ONCE: usize = 4;
/// How long the panel waits for names from Discord (creators, servers).
/// They only label rows, so a slow Discord costs the labels, not the page.
const NAME_LOOKUP_DEADLINE: Duration = if cfg!(test) {
    Duration::from_millis(300)
} else {
    Duration::from_secs(3)
};
/// How long a log line may hold up the answer to the admin's own request.
const LOG_LINE_TIMEOUT: Duration = Duration::from_secs(4);

/// Builds the admin routes (browser OAuth + the JSON admin API).
pub fn router<D: DiscordApi>() -> Router<ApiState<D>> {
    Router::new()
        .route("/admin/login", get(login::<D>))
        .route("/admin/callback", get(callback::<D>))
        .route("/api/admin/guilds", get(list_guilds::<D>))
        .route("/api/admin/guilds/{gid}", get(guild_detail::<D>))
        .route("/api/admin/guilds/{gid}/options", get(guild_options::<D>))
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
        let token = state::bearer(parts).ok_or(ApiError::Unauthorized)?;
        let claims = key
            .verify_admin(token, now_unix())
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
    /// Set by Discord instead of `code` when the sign-in did not complete
    /// (`access_denied` when the person pressed Cancel).
    error: Option<String>,
}

/// Why a sign-in did not finish. The panel has a card for each; the code is
/// one of a fixed set, so nothing a caller sends ends up in the redirect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignInProblem {
    /// The person cancelled Discord's consent screen.
    Denied,
    /// The login took too long, or the link was not one leaf issued.
    Expired,
    /// Discord refused the code (spent, or leaf's credentials are wrong).
    ExchangeFailed,
    /// Discord could not be reached.
    DiscordUnavailable,
    /// The person manages no server that leaf is in.
    NoGuilds,
    /// leaf itself failed (the logs say how).
    Internal,
}

impl SignInProblem {
    const fn code(self) -> &'static str {
        match self {
            Self::Denied => "denied",
            Self::Expired => "expired",
            Self::ExchangeFailed => "exchange_failed",
            Self::DiscordUnavailable => "discord_unavailable",
            Self::NoGuilds => "no_guilds",
            Self::Internal => "internal",
        }
    }

    fn redirect(self) -> Redirect {
        Redirect::to(&format!("/admin#error={}", self.code()))
    }
}

/// `GET /admin/callback` — exchange the code, mint an admin token, hand it to
/// the SPA via the URL fragment (kept out of logs/referrers). Every failure
/// goes back to the panel as `/admin#error=<code>`, never a JSON page: this
/// address is opened by a browser, not by the panel's own code.
async fn callback<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Query(q): Query<CallbackQuery>,
) -> Redirect {
    match sign_in(&st, q).await {
        Ok(token) => Redirect::to(&format!("/admin#token={token}")),
        Err(problem) => problem.redirect(),
    }
}

async fn sign_in<D: DiscordApi>(
    st: &ApiState<D>,
    q: CallbackQuery,
) -> Result<String, SignInProblem> {
    if let Some(error) = q.error {
        return Err(if error == "access_denied" {
            SignInProblem::Denied
        } else {
            tracing::warn!(error, "admin oauth: Discord returned an error");
            SignInProblem::ExchangeFailed
        });
    }
    let (Some(code), Some(state)) = (q.code, q.state) else {
        return Err(SignInProblem::Expired);
    };
    if !st.key.verify_oauth_state(&state, now_unix()) {
        return Err(SignInProblem::Expired);
    }
    let redirect = admin_redirect_uri(&st.redirect_uri);
    let access = st
        .discord
        .exchange_code(&code, &redirect)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "admin oauth exchange failed");
            match e {
                ExchangeError::Rejected(_) => SignInProblem::ExchangeFailed,
                ExchangeError::Unavailable(_) => SignInProblem::DiscordUnavailable,
            }
        })?;
    let user_id = st.discord.current_user_id(&access).await.map_err(|e| {
        tracing::warn!(error = %e, "admin oauth: user lookup failed");
        SignInProblem::DiscordUnavailable
    })?;
    let manageable = st.discord.managed_guild_ids(&access).await.map_err(|e| {
        tracing::warn!(error = %e, "admin oauth: guild list failed");
        SignInProblem::DiscordUnavailable
    })?;

    // Keep only guilds leaf is actually in, so the token (and panel) shows
    // only what's actionable.
    let mut guilds = Vec::new();
    for gid in manageable {
        let known = st.guilds.get(&gid).await.map_err(|e| {
            tracing::error!(error = %e, "admin oauth: settings lookup failed");
            SignInProblem::Internal
        })?;
        if known.is_some() {
            guilds.push(gid);
        }
    }
    if guilds.is_empty() {
        return Err(SignInProblem::NoGuilds);
    }

    Ok(st
        .key
        .mint_admin(&user_id, &guilds, now_unix(), ADMIN_TTL_SECS))
}

// --- Discord lookups, all best-effort ------------------------------------

/// The guild's name and icon, cached. `None` when Discord could not say
/// (the panel then shows the id).
async fn guild_summary<D: DiscordApi>(st: &ApiState<D>, gid: &str) -> Option<GuildSummary> {
    let discord = Arc::clone(&st.discord);
    let g = gid.to_owned();
    state::guild_summary_cache()
        .try_get_with(g.clone(), async move { discord.guild_summary(&g).await })
        .await
        .inspect_err(|e| tracing::warn!(error = %e, "guild lookup failed"))
        .ok()
        .flatten()
}

/// The Discord CDN address of a guild icon, sized for a list row.
fn icon_url(gid: &str, summary: &GuildSummary) -> Option<String> {
    summary
        .icon
        .as_deref()
        .map(|hash| format!("https://cdn.discordapp.com/icons/{gid}/{hash}.png?size=64"))
}

/// What each creator is called in this guild, by user id.
///
/// A creator who has left, or whom Discord could not be asked about in time,
/// is simply absent. Names are kept in their own cache; the ones it lacks are
/// looked up a few at a time, for at most [`NAME_LOOKUP_DEADLINE`] in all.
/// Lookups still running then finish in the background, so the next load has
/// their names.
async fn creator_names<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    series: &[Series],
) -> HashMap<String, String> {
    let creators: HashSet<&str> = series.iter().map(|s| s.creator_id.as_str()).collect();
    let known = state::creator_name_cache();
    let gate = Arc::new(tokio::sync::Semaphore::new(NAME_LOOKUPS_AT_ONCE));
    let mut names = HashMap::new();
    let mut lookups = tokio::task::JoinSet::new();
    for user_id in creators {
        let key = (gid.to_owned(), user_id.to_owned());
        if let Some(name) = known.get(&key).await {
            if let Some(name) = name {
                names.insert(user_id.to_owned(), name);
            }
            continue;
        }
        let (gate, discord) = (Arc::clone(&gate), Arc::clone(&st.discord));
        let members = st.membership.clone();
        lookups.spawn(async move {
            let _permit = gate.acquire_owned().await.ok()?;
            let (g, u) = key.clone();
            // An answer is kept (a member who left has no name); a failed
            // lookup is not, so it is asked again next time.
            let member = members
                .try_get_with(
                    key.clone(),
                    async move { discord.guild_member(&g, &u).await },
                )
                .await
                .ok()?;
            let name = member.and_then(|m| m.name);
            known.insert(key.clone(), name.clone()).await;
            name.map(|name| (key.1, name))
        });
    }
    let deadline = tokio::time::Instant::now() + NAME_LOOKUP_DEADLINE;
    loop {
        match tokio::time::timeout_at(deadline, lookups.join_next()).await {
            Ok(Some(Ok(Some((user_id, name))))) => {
                names.insert(user_id, name);
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(
                    guild = gid,
                    waiting = lookups.len(),
                    "creator names: Discord is slow; showing the page without the rest"
                );
                lookups.detach_all();
                break;
            }
        }
    }
    names
}

/// Each guild's name and icon, looked up side by side for at most
/// [`NAME_LOOKUP_DEADLINE`] in all. A guild Discord did not answer for in
/// time is absent (the panel shows its id).
async fn guild_summaries<D: DiscordApi>(
    st: &ApiState<D>,
    guild_ids: &[String],
) -> HashMap<String, GuildSummary> {
    let mut lookups = tokio::task::JoinSet::new();
    for gid in guild_ids {
        let (st, gid) = (st.clone(), gid.clone());
        lookups.spawn(async move {
            let summary = guild_summary(&st, &gid).await?;
            Some((gid, summary))
        });
    }
    let mut summaries = HashMap::new();
    let deadline = tokio::time::Instant::now() + NAME_LOOKUP_DEADLINE;
    loop {
        match tokio::time::timeout_at(deadline, lookups.join_next()).await {
            Ok(Some(Ok(Some((gid, summary))))) => {
                summaries.insert(gid, summary);
            }
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(_) => {
                tracing::warn!(
                    waiting = lookups.len(),
                    "guild names: Discord is slow; showing the list without the rest"
                );
                lookups.detach_all();
                break;
            }
        }
    }
    summaries
}

// --- admin API -----------------------------------------------------------

#[derive(Serialize)]
struct AdminGuildDto {
    guild_id: String,
    series_count: usize,
    /// The server's name in Discord; `null` when Discord could not say.
    name: Option<String>,
    icon_url: Option<String>,
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
    /// On a PATCH answer: how many sprouts that save published.
    #[serde(skip_serializing_if = "Option::is_none")]
    sprouts_published: Option<u64>,
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
            sprouts_published: None,
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
    /// The creator's display name in this server; `null` once they have left.
    creator_name: Option<String>,
    /// How many days are archived (a sprout's progress).
    archived_days: i64,
    /// The name of `privacy_role_id`, while Discord still lists that role.
    privacy_role_name: Option<String>,
}

impl AdminSeriesDto {
    fn build(
        s: &Series,
        archived_days: i64,
        names: &HashMap<String, String>,
        roles: Option<&[GuildRole]>,
    ) -> Self {
        let privacy_role_name = s
            .privacy_role_id
            .as_ref()
            .and_then(|id| roles?.iter().find(|r| &r.id == id).map(|r| r.name.clone()));
        Self {
            id: s.id,
            name: s.name.clone(),
            creator_id: s.creator_id.clone(),
            privacy: s.privacy.as_str().to_owned(),
            privacy_role_id: s.privacy_role_id.clone(),
            state: s.state.as_str().to_owned(),
            creator_name: names.get(&s.creator_id).cloned(),
            archived_days,
            privacy_role_name,
        }
    }
}

#[derive(Serialize)]
struct AdminGuildDetailDto {
    guild_id: String,
    name: Option<String>,
    icon_url: Option<String>,
    /// False until someone has run `/setup` in the server.
    setup_complete: bool,
    settings: AdminSettingsDto,
    series: Vec<AdminSeriesDto>,
}

/// `GET /api/admin/guilds` — the guilds this admin manages that have leaf.
async fn list_guilds<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    admin: AdminUser,
) -> Result<Json<Vec<AdminGuildDto>>, ApiError> {
    let mut summaries = guild_summaries(&st, &admin.0.guild_ids).await;
    let mut out = Vec::new();
    for gid in &admin.0.guild_ids {
        let count = st.series.list_by_guild(gid).await?.len();
        let summary = summaries.remove(gid);
        out.push(AdminGuildDto {
            guild_id: gid.clone(),
            series_count: count,
            icon_url: summary.as_ref().and_then(|s| icon_url(gid, s)),
            name: summary.map(|s| s.name).filter(|n| !n.is_empty()),
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

    let summary = guild_summaries(&st, std::slice::from_ref(&gid))
        .await
        .remove(&gid);
    let names = creator_names(&st, &gid, &series).await;
    // Role names are only needed when some series is gated on a role.
    let roles = if series.iter().any(|s| s.privacy_role_id.is_some()) {
        guild_roles(&st, &gid).await
    } else {
        None
    };

    let mut rows = Vec::with_capacity(series.len());
    for s in &series {
        let archived = st.posts.count(s.id).await?;
        rows.push(AdminSeriesDto::build(
            s,
            archived,
            &names,
            roles.as_ref().map(|r| r.as_slice()),
        ));
    }
    Ok(Json(AdminGuildDetailDto {
        icon_url: summary.as_ref().and_then(|s| icon_url(&gid, s)),
        name: summary.map(|s| s.name).filter(|n| !n.is_empty()),
        guild_id: gid,
        setup_complete: settings.setup_complete,
        settings: (&settings).into(),
        series: rows,
    }))
}

#[derive(Serialize)]
struct NamedIdDto {
    id: String,
    name: String,
}

/// What the panel's pickers offer.
#[derive(Serialize)]
struct AdminOptionsDto {
    /// Assignable roles, highest first.
    roles: Vec<NamedIdDto>,
    /// Text and announcement channels, in sidebar order.
    channels: Vec<NamedIdDto>,
    /// True when Discord could not supply the role list.
    roles_unavailable: bool,
    /// True when Discord could not supply the channel list.
    channels_unavailable: bool,
}

/// `GET /api/admin/guilds/{gid}/options` — the roles and channels the
/// settings pickers offer, asked of Discord on every call (the panel calls
/// it once per load, and a role or channel made a minute ago must be in the
/// picker). A list Discord could not supply is flagged, and the panel falls
/// back to an id box for that field.
async fn guild_options<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    admin: AdminUser,
) -> Result<Json<AdminOptionsDto>, ApiError> {
    admin.require_guild(&gid)?;
    let roles = fresh_guild_roles(&st, &gid).await;
    let channels = fresh_guild_channels(&st, &gid).await;

    let mut text: Vec<_> = channels
        .iter()
        .flat_map(|list| list.iter())
        .filter(|c| c.is_text())
        .collect();
    text.sort_by(|a, b| a.position.cmp(&b.position).then_with(|| a.id.cmp(&b.id)));

    Ok(Json(AdminOptionsDto {
        roles_unavailable: roles.is_none(),
        channels_unavailable: channels.is_none(),
        roles: roles
            .iter()
            .flat_map(|list| list.iter())
            .map(|r| NamedIdDto {
                id: r.id.clone(),
                name: r.name.clone(),
            })
            .collect(),
        channels: text
            .into_iter()
            .map(|c| NamedIdDto {
                id: c.id.clone(),
                name: c.name.clone(),
            })
            .collect(),
    }))
}

#[derive(Deserialize, Default)]
struct SettingsPatch {
    timezone: Option<String>,
    /// Empty string clears the field.
    creator_role_id: Option<String>,
    /// Empty string clears the field.
    log_channel_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    max_series_per_user: Option<NumberField>,
    #[serde(default, deserialize_with = "present")]
    min_account_age_days: Option<NumberField>,
    #[serde(default, deserialize_with = "present")]
    min_membership_age_days: Option<NumberField>,
    sprout_enabled: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    sprout_threshold: Option<NumberField>,
}

/// A numeric field as it was sent: a number, or an explicit `null`.
///
/// A number cannot be cleared, so a `null` is refused by name instead of
/// being ignored, which needs it kept apart from a field left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NumberField(Option<i64>);

/// Reads a numeric field that is present in the body (serde leaves a
/// missing one at its default, `None`, without calling this).
fn present<'de, D>(deserializer: D) -> Result<Option<NumberField>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<i64>::deserialize(deserializer).map(|sent| Some(NumberField(sent)))
}

fn none_if_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// Checks a numeric setting against its range. The message names the field,
/// because the panel shows it next to Save, not under the control.
fn check_limit(label: &str, NumberField(value): NumberField, min: i64) -> Result<i64, ApiError> {
    value
        .filter(|value| (min..=MAX_NUMBER).contains(value))
        .ok_or_else(|| {
            ApiError::unprocessable(
                "invalid_limit",
                format!("{label} must be a whole number from {min} to {MAX_NUMBER}."),
            )
        })
}

/// Applies the pure parts of a settings patch (everything that needs no
/// Discord lookup). A value equal to the stored one is left alone and not
/// re-checked, so a stored value from before validation existed cannot block
/// an unrelated change.
fn apply_plain_settings(s: &mut GuildSettings, p: &SettingsPatch) -> Result<(), ApiError> {
    if let Some(tz) = &p.timezone
        && *tz != s.timezone
    {
        let canonical = series_ops::canonical_timezone(tz).ok_or_else(|| {
            ApiError::unprocessable(
                "invalid_timezone",
                "That isn't a timezone leaf knows. Pick one from the list, \
                 for example America/Chicago.",
            )
        })?;
        canonical.clone_into(&mut s.timezone);
    }
    if let Some(v) = p.max_series_per_user
        && v != NumberField(Some(s.max_series_per_user))
    {
        s.max_series_per_user = check_limit("Series per member", v, 1)?;
    }
    if let Some(v) = p.min_account_age_days
        && v != NumberField(Some(s.min_account_age_days))
    {
        s.min_account_age_days = check_limit("Minimum Discord account age", v, 0)?;
    }
    if let Some(v) = p.min_membership_age_days
        && v != NumberField(Some(s.min_membership_age_days))
    {
        s.min_membership_age_days = check_limit("Minimum time in this server", v, 0)?;
    }
    if let Some(v) = p.sprout_enabled {
        s.sprout_enabled = v;
    }
    if let Some(v) = p.sprout_threshold
        && v != NumberField(Some(s.sprout_threshold))
    {
        s.sprout_threshold = check_limit("Days before a sprout is published", v, 1)?;
    }
    Ok(())
}

/// Whether `channel_id` is a text or announcement channel of this guild.
/// `None` when Discord could not supply the channel list.
///
/// A channel missing from the cached list may simply be newer than it, so
/// the list is asked for once more before the answer is "no".
async fn is_guild_text_channel<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    channel_id: &str,
) -> Option<bool> {
    let mut channels = guild_channels(st, gid).await?;
    if !channels.iter().any(|c| c.id == channel_id)
        && state::refresh_allowed(state::GuildList::Channels, gid).await
    {
        channels = fresh_guild_channels(st, gid).await?;
    }
    Some(channels.iter().any(|c| c.id == channel_id && c.is_text()))
}

/// Applies the role and channel parts of a settings patch, each checked
/// against this guild when it changes.
async fn apply_id_settings<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    s: &mut GuildSettings,
    p: &SettingsPatch,
) -> Result<(), ApiError> {
    if let Some(v) = &p.creator_role_id {
        let role = none_if_empty(v);
        if role != s.creator_role_id {
            if let Some(id) = &role {
                require_role(st, gid, id).await?;
            }
            s.creator_role_id = role;
        }
    }
    if let Some(v) = &p.log_channel_id {
        let channel = none_if_empty(v);
        if channel != s.log_channel_id {
            if let Some(id) = &channel {
                match is_guild_text_channel(st, gid, id).await {
                    Some(true) => {}
                    Some(false) => {
                        return Err(ApiError::unprocessable(
                            "unknown_channel",
                            "That isn't a text channel of this server that leaf can see. \
                             Pick a channel from the list.",
                        ));
                    }
                    None => return Err(ApiError::discord_unavailable()),
                }
            }
            s.log_channel_id = channel;
        }
    }
    Ok(())
}

/// Posts one quiet line in the guild's log channel, if it has one and that
/// channel is still a text channel of this guild. Never fails the request:
/// the change it reports is already stored.
async fn log_line<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    settings: &GuildSettings,
    line: &str,
) {
    let Some(channel) = &settings.log_channel_id else {
        return;
    };
    // A stored id from before validation could point into another server.
    if is_guild_text_channel(st, gid, channel).await != Some(true) {
        tracing::warn!(
            guild = gid,
            channel,
            "log line skipped: the log channel could not be confirmed as this server's"
        );
        return;
    }
    match tokio::time::timeout(LOG_LINE_TIMEOUT, st.discord.send_message(channel, line)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(guild = gid, channel, error = %e, "log line not sent"),
        Err(_) => tracing::warn!(guild = gid, channel, "log line timed out"),
    }
}

/// How a log line names a series: by name only when everyone may see it.
fn log_subject(s: &Series) -> String {
    if s.privacy == Privacy::Public && s.state == SeriesState::Active {
        format!("**{}**", series_ops::display_name(&s.name))
    } else {
        "A private series".to_owned()
    }
}

/// `PATCH /api/admin/guilds/{gid}/settings` — partial settings update.
/// Answers the stored settings plus `sprouts_published`: how many sprouts
/// this save released (the stage was switched off, or the threshold now sits
/// at or below what they have archived).
async fn patch_settings<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(gid): Path<String>,
    admin: AdminUser,
    ApiJson(p): ApiJson<SettingsPatch>,
) -> Result<Json<AdminSettingsDto>, ApiError> {
    admin.require_guild(&gid)?;
    let read = st.guilds.get(&gid).await?.ok_or(ApiError::NotFound)?;
    let mut edited = read.clone();
    apply_plain_settings(&mut edited, &p)?;
    apply_id_settings(&st, &gid, &mut edited, &p).await?;
    // Checking a role or channel can take Discord seconds, and `/setup` may
    // be saved in chat meanwhile. Only what this save changed is written
    // (never the series channels or "setup complete"), and the answer is
    // the row as it now stands.
    st.guilds
        .apply_panel_change(&gid, &PanelChange::between(&read, &edited))
        .await?;
    let settings = st.guilds.get(&gid).await?.ok_or(ApiError::NotFound)?;

    let mut published = 0;
    if p.sprout_enabled.is_some() || p.sprout_threshold.is_some() {
        let threshold = settings.sprout_enabled.then_some(settings.sprout_threshold);
        published = st.series.promote_sprouts(&gid, threshold).await?;
        if published > 0 {
            let line = format!(
                "🌿 {published} series published: <@{}> changed the sprout settings",
                admin.0.user_id
            );
            log_line(&st, &gid, &settings, &line).await;
        }
    }

    let mut dto = AdminSettingsDto::from(&settings);
    dto.sprouts_published = Some(published);
    Ok(Json(dto))
}

#[derive(Deserialize)]
struct SeriesPatch {
    privacy: Option<String>,
    /// Empty string clears the role.
    privacy_role_id: Option<String>,
    state: Option<String>,
}

/// What a state change did, for the log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StateChange {
    Revoked,
    Restored,
    Published,
}

/// The state an admin's request leads to, and what to call the change.
///
/// Asking for `active` on a revoked series restores it, back into the sprout
/// stage when the guild uses one and the series is still short of the
/// threshold; on a sprout it publishes, whatever the threshold. `None` means
/// the series is already there.
fn next_state(
    current: SeriesState,
    wanted: SeriesState,
    settings: &GuildSettings,
    archived_days: i64,
) -> Option<(SeriesState, Option<StateChange>)> {
    match (current, wanted) {
        (from, to) if from == to => None,
        (_, SeriesState::Revoked) => Some((SeriesState::Revoked, Some(StateChange::Revoked))),
        (SeriesState::Revoked, SeriesState::Active) => {
            let short = settings.sprout_enabled && archived_days < settings.sprout_threshold;
            let to = if short {
                SeriesState::Sprout
            } else {
                SeriesState::Active
            };
            Some((to, Some(StateChange::Restored)))
        }
        (SeriesState::Sprout, SeriesState::Active) => {
            Some((SeriesState::Active, Some(StateChange::Published)))
        }
        (_, to) => Some((to, None)),
    }
}

/// `PATCH /api/admin/guilds/{gid}/series/{sid}` — edit privacy, revoke,
/// restore or publish.
async fn patch_series<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    admin: AdminUser,
    ApiJson(p): ApiJson<SeriesPatch>,
) -> Result<Json<AdminSeriesDto>, ApiError> {
    admin.require_guild(&gid)?;
    let mut series = st.series.get(sid).await?.ok_or(ApiError::NotFound)?;
    if series.guild_id != gid {
        return Err(ApiError::NotFound);
    }
    let settings = st.guilds.get(&gid).await?.ok_or(ApiError::NotFound)?;

    // Parse everything before writing anything.
    let wanted_state = p
        .state
        .as_deref()
        .map(str::parse::<SeriesState>)
        .transpose()
        .map_err(|_| ApiError::BadRequest)?;

    // privacy / role live on the series row (`update`); state via `set_state`.
    let edits_row = p.privacy.is_some() || p.privacy_role_id.is_some();
    if let Some(privacy) = &p.privacy {
        series.privacy = privacy
            .parse::<Privacy>()
            .map_err(|_| ApiError::BadRequest)?;
    }
    if let Some(role) = p.privacy_role_id {
        let role = none_if_empty(&role);
        if role != series.privacy_role_id
            && let Some(id) = &role
        {
            require_role(&st, &gid, id).await?;
        }
        series.privacy_role_id = role;
    }
    // Only an edit of privacy or role is checked: a stored value from before
    // this rule existed must not block a revoke, restore or publish.
    if edits_row && series.privacy == Privacy::RoleGated && series.privacy_role_id.is_none() {
        // Saved without a role, nobody but the creator could see the series.
        return Err(ApiError::unprocessable(
            "role_required",
            "Choose the role that can see this series.",
        ));
    }
    if edits_row {
        st.series.update(&series).await?;
    }

    let archived_days = st.posts.count(series.id).await?;
    if let Some(wanted) = wanted_state
        && let Some((to, change)) = next_state(series.state, wanted, &settings, archived_days)
    {
        // A revoke is named as the series was known until now.
        let before = log_subject(&series);
        st.series.set_state(series.id, to).await?;
        series.state = to;
        let actor = &admin.0.user_id;
        let line = match change {
            Some(StateChange::Revoked) => Some(format!("🚫 {before} · revoked by <@{actor}>")),
            Some(StateChange::Restored) => Some(format!(
                "🌿 {} · restored by <@{actor}>",
                log_subject(&series)
            )),
            Some(StateChange::Published) => Some(format!(
                "🌿 {} · published by <@{actor}>",
                log_subject(&series)
            )),
            None => None,
        };
        if let Some(line) = line {
            log_line(&st, &gid, &settings, &line).await;
        }
    }

    let names = creator_names(&st, &gid, std::slice::from_ref(&series)).await;
    let roles = if series.privacy_role_id.is_some() {
        guild_roles(&st, &gid).await
    } else {
        None
    };
    Ok(Json(AdminSeriesDto::build(
        &series,
        archived_days,
        &names,
        roles.as_ref().map(|r| r.as_slice()),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(sprout_enabled: bool, threshold: i64) -> GuildSettings {
        let mut s = GuildSettings::defaults_for("g");
        s.sprout_enabled = sprout_enabled;
        s.sprout_threshold = threshold;
        s
    }

    #[test]
    fn restore_goes_back_to_sprout_only_when_short_of_the_threshold() {
        use SeriesState::{Active, Revoked, Sprout};
        // Sprout stage on, 1 of 3 days: back to probation.
        assert_eq!(
            next_state(Revoked, Active, &settings(true, 3), 1),
            Some((Sprout, Some(StateChange::Restored)))
        );
        // At the threshold, or with the stage off: straight to active.
        assert_eq!(
            next_state(Revoked, Active, &settings(true, 3), 3),
            Some((Active, Some(StateChange::Restored)))
        );
        assert_eq!(
            next_state(Revoked, Active, &settings(false, 3), 0),
            Some((Active, Some(StateChange::Restored)))
        );
    }

    #[test]
    fn publish_ignores_the_threshold_and_same_state_is_a_no_op() {
        use SeriesState::{Active, Revoked, Sprout};
        assert_eq!(
            next_state(Sprout, Active, &settings(true, 30), 0),
            Some((Active, Some(StateChange::Published)))
        );
        assert_eq!(next_state(Active, Active, &settings(true, 3), 0), None);
        assert_eq!(next_state(Revoked, Revoked, &settings(true, 3), 0), None);
        assert_eq!(
            next_state(Active, Revoked, &settings(true, 3), 9),
            Some((Revoked, Some(StateChange::Revoked)))
        );
    }

    #[test]
    fn limits_are_range_checked_and_the_message_names_the_field() {
        let mut s = GuildSettings::defaults_for("g");
        let low = SettingsPatch {
            max_series_per_user: Some(NumberField(Some(0))),
            ..SettingsPatch::default()
        };
        let e = apply_plain_settings(&mut s, &low).err();
        assert_eq!(e.as_ref().map(ApiError::code), Some("invalid_limit"));
        assert!(format!("{e:?}").contains("Series per member"));

        let ages = SettingsPatch {
            min_account_age_days: Some(NumberField(Some(0))),
            min_membership_age_days: Some(NumberField(Some(-1))),
            ..SettingsPatch::default()
        };
        assert_eq!(
            apply_plain_settings(&mut s, &ages).err().map(|e| e.code()),
            Some("invalid_limit")
        );
        let high = SettingsPatch {
            sprout_threshold: Some(NumberField(Some(MAX_NUMBER + 1))),
            ..SettingsPatch::default()
        };
        assert!(apply_plain_settings(&mut s, &high).is_err());
    }

    #[test]
    fn a_null_number_is_refused_and_a_missing_one_is_left_alone() {
        let absent: SettingsPatch = serde_json::from_str("{}").unwrap_or_default();
        assert_eq!(absent.max_series_per_user, None);
        let mut s = GuildSettings::defaults_for("g");
        let before = s.max_series_per_user;
        assert!(apply_plain_settings(&mut s, &absent).is_ok());
        assert_eq!(s.max_series_per_user, before);

        for (body, label) in [
            (r#"{"max_series_per_user":null}"#, "Series per member"),
            (
                r#"{"min_account_age_days":null}"#,
                "Minimum Discord account age",
            ),
            (
                r#"{"min_membership_age_days":null}"#,
                "Minimum time in this server",
            ),
            (
                r#"{"sprout_threshold":null}"#,
                "Days before a sprout is published",
            ),
        ] {
            let patch: SettingsPatch = serde_json::from_str(body).unwrap_or_default();
            let e = apply_plain_settings(&mut s, &patch).err();
            assert_eq!(
                e.as_ref().map(ApiError::code),
                Some("invalid_limit"),
                "{body}"
            );
            assert!(format!("{e:?}").contains(label), "{body}");
        }
    }

    #[test]
    fn timezone_is_stored_in_canonical_case_or_refused() {
        let mut s = GuildSettings::defaults_for("g");
        let ok = SettingsPatch {
            timezone: Some("america/chicago".to_owned()),
            ..SettingsPatch::default()
        };
        assert!(apply_plain_settings(&mut s, &ok).is_ok());
        assert_eq!(s.timezone, "America/Chicago");

        let bad = SettingsPatch {
            timezone: Some("Mars/Olympus".to_owned()),
            ..SettingsPatch::default()
        };
        assert_eq!(
            apply_plain_settings(&mut s, &bad).err().map(|e| e.code()),
            Some("invalid_timezone")
        );
        assert_eq!(s.timezone, "America/Chicago");
    }

    #[test]
    fn log_lines_name_only_public_active_series() {
        let mut s = Series {
            id: 1,
            guild_id: "g".to_owned(),
            creator_id: "u".to_owned(),
            name: "daily_art".to_owned(),
            description: String::new(),
            channels: vec![],
            cadence: leaf_core::domain::Cadence::Daily,
            detection_mode: leaf_core::domain::DetectionMode::ContextMenu,
            privacy: Privacy::Public,
            privacy_role_id: None,
            start_day: 1,
            reminder_enabled: false,
            reminder_time: None,
            reminder_timezone: None,
            reminder_dm: true,
            milestone_template: None,
            emoji: "🍃".to_owned(),
            state: SeriesState::Active,
            created_at: 0,
        };
        assert_eq!(log_subject(&s), r"**daily\_art**");
        s.state = SeriesState::Sprout;
        assert_eq!(log_subject(&s), "A private series");
        s.state = SeriesState::Active;
        s.privacy = Privacy::CreatorOnly;
        assert_eq!(log_subject(&s), "A private series");
    }

    #[test]
    fn sign_in_problems_redirect_to_fixed_fragments() {
        for (problem, code) in [
            (SignInProblem::Denied, "denied"),
            (SignInProblem::Expired, "expired"),
            (SignInProblem::ExchangeFailed, "exchange_failed"),
            (SignInProblem::DiscordUnavailable, "discord_unavailable"),
            (SignInProblem::NoGuilds, "no_guilds"),
            (SignInProblem::Internal, "internal"),
        ] {
            assert_eq!(problem.code(), code);
        }
    }
}
