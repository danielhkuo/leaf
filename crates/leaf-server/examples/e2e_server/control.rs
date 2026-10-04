//! The `/__e2e/…` routes: what a suite uses to put the server in the state a
//! test needs. Each one is described in the crate's top-level doc comment.
//!
//! They do nothing leaf cannot do by itself: tokens come from the public
//! [`SessionKey`](leaf_server::api::auth::SessionKey) API and rows go through
//! the repositories.

use std::sync::Arc;

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use leaf_core::db::LaunchIntentRepo;
use leaf_core::db::launch::LAUNCH_INTENT_TTL_SECS;
use leaf_core::domain::{Series, SeriesState};
use leaf_core::localtime;
use leaf_server::api::auth::{SESSION_MAX_AGE_SECS, SESSION_TTL_SECS, now_unix};
use object_store::path::Path as ObjectPath;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::discord::{ACCESS_TOKEN_PREFIX, Behaviour, CODE_PREFIX, SentMessage};
use crate::seed::{self, Media, Persona, PlannedDay};
use crate::{Harness, World, setup};

const DAY_SECS: i64 = 86_400;

/// How long an `expiring` session stays valid.
const EXPIRING_TTL_SECS: i64 = 5;
/// How long a `capped` session stays valid: long enough to load a gallery
/// with, while its sign-in is already past the renewal cap.
const CAPPED_TTL_SECS: i64 = 300;
/// How long an admin token minted here lasts, as one from a real login does.
const ADMIN_TTL_SECS: i64 = 3_600;
/// How long the `state` handed to the admin callback is good for.
const OAUTH_STATE_TTL_SECS: i64 = 600;

/// Where `GET /__e2e/elsewhere` points. `.invalid` is a name that never
/// resolves (RFC 6761), so even a browser that followed the redirect would
/// reach nothing.
const ELSEWHERE: &str = "http://leaf-e2e.invalid/landed";

/// The control routes, to be nested under `/__e2e`.
pub fn router() -> Router<Arc<Harness>> {
    Router::new()
        .route("/state", get(manifest))
        .route("/reset", post(reset))
        .route("/caches/clear", post(clear_caches))
        .route("/discord", get(discord_status).post(set_discord))
        .route("/discord/messages", get(discord_messages))
        .route("/launch-intent", post(put_launch_intent))
        .route("/series/{series}/days", post(add_day))
        .route("/series/{series}/days/{day}", delete(remove_day))
        .route("/session", post(mint_session))
        .route("/admin-session", post(mint_admin_session))
        .route("/admin/login", get(admin_login))
        .route("/setup", get(setup_status).post(open_setup))
        .route("/elsewhere", get(elsewhere))
        // Without this an unknown control path would fall through to the
        // gallery and answer 200 with its HTML.
        .fallback(|| async { Refusal::new(StatusCode::NOT_FOUND, "no such control route") })
}

/// A control route's refusal: a status and a sentence saying what was wrong
/// with the request.
struct Refusal {
    status: StatusCode,
    message: String,
}

impl Refusal {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}

impl From<anyhow::Error> for Refusal {
    fn from(e: anyhow::Error) -> Self {
        tracing::error!(error = %format!("{e:#}"), "control route failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"))
    }
}

/// A body that is not the JSON a route takes, said in the same shape as
/// every other refusal.
impl From<JsonRejection> for Refusal {
    fn from(rejection: JsonRejection) -> Self {
        Self::new(rejection.status(), rejection.body_text())
    }
}

impl From<leaf_core::db::DbError> for Refusal {
    fn from(e: leaf_core::db::DbError) -> Self {
        anyhow::Error::from(e).into()
    }
}

fn find_persona(key: &str) -> Result<&'static Persona, Refusal> {
    seed::persona(key).ok_or_else(|| Refusal::bad_request(format!("no persona called {key:?}")))
}

/// The series named by a seed key or a numeric id, as it is stored now.
async fn find_series(world: &World, wanted: &str) -> Result<Series, Refusal> {
    let id = world
        .series()
        .iter()
        .find(|seeded| seeded.key == wanted)
        .map(|seeded| seeded.series.id)
        .or_else(|| wanted.parse().ok())
        .ok_or_else(|| Refusal::bad_request(format!("no series called {wanted:?}")))?;
    world
        .state
        .series
        .get(id)
        .await?
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, format!("no series with id {id}")))
}

/// `GET /__e2e/state`
async fn manifest(State(harness): State<Arc<Harness>>) -> Json<Value> {
    Json(harness.manifest())
}

/// `POST /__e2e/reset`
async fn reset(State(harness): State<Arc<Harness>>) -> Result<Json<Value>, Refusal> {
    harness.reset().await?;
    Ok(Json(harness.manifest()))
}

/// `POST /__e2e/caches/clear`
async fn clear_caches(State(harness): State<Arc<Harness>>) -> StatusCode {
    harness.clear_caches();
    StatusCode::NO_CONTENT
}

/// `POST /__e2e/setup`
async fn open_setup(
    State(harness): State<Arc<Harness>>,
    body: Result<Option<Json<setup::Script>>, JsonRejection>,
) -> Result<Json<Value>, Refusal> {
    let script = body?.map(|Json(script)| script).unwrap_or_default();
    Ok(Json(harness.open_setup(script)?.opening()))
}

/// `GET /__e2e/setup`
async fn setup_status(State(harness): State<Arc<Harness>>) -> Result<Json<Value>, Refusal> {
    let stage = harness
        .setup()
        .ok_or_else(|| Refusal::new(StatusCode::NOT_FOUND, "no setup mode is open"))?;
    // Reads a few small files: file work, so off the runtime.
    let status = tokio::task::spawn_blocking(move || stage.status())
        .await
        .map_err(anyhow::Error::from)??;
    Ok(Json(status))
}

/// `GET /__e2e/discord`
async fn discord_status(State(harness): State<Arc<Harness>>) -> Json<Value> {
    let Behaviour {
        mode,
        delay_ms,
        only,
    } = harness.discord.behaviour();
    Json(json!({
        "mode": mode,
        "delay_ms": delay_ms,
        "only": only,
        "calls": harness.discord.calls(),
        "held": *harness.discord.held().borrow(),
    }))
}

/// `POST /__e2e/discord`
async fn set_discord(
    State(harness): State<Arc<Harness>>,
    body: Result<Json<Behaviour>, JsonRejection>,
) -> Result<Json<Value>, Refusal> {
    let Json(behaviour) = body?;
    harness.discord.set_behaviour(behaviour);
    Ok(discord_status(State(harness)).await)
}

/// `GET /__e2e/discord/messages`
async fn discord_messages(State(harness): State<Arc<Harness>>) -> Json<Vec<SentMessage>> {
    Json(harness.discord.sent())
}

#[derive(Deserialize)]
struct LaunchIntent {
    persona: String,
    series: String,
    day: Option<i64>,
    /// A guild id; the series' own when left out.
    guild: Option<String>,
}

/// `POST /__e2e/launch-intent`
async fn put_launch_intent(
    State(harness): State<Arc<Harness>>,
    body: Result<Json<LaunchIntent>, JsonRejection>,
) -> Result<Json<Value>, Refusal> {
    let Json(intent) = body?;
    let world = harness.world();
    let persona = find_persona(&intent.persona)?;
    let series = find_series(&world, &intent.series).await?;
    let guild_id = intent.guild.unwrap_or_else(|| series.guild_id.clone());
    LaunchIntentRepo::new(world.state.series.pool().clone())
        .put(persona.id, &guild_id, series.id, intent.day, now_unix())
        .await?;
    Ok(Json(json!({
        "user_id": persona.id,
        "guild_id": guild_id,
        "series_id": series.id,
        "day": intent.day,
        "expires_in": LAUNCH_INTENT_TTL_SECS,
    })))
}

/// What a day added through the control route holds.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NewDayMedia {
    /// One still.
    #[default]
    Image,
    /// Three stills.
    Images,
    /// The MP4 clip.
    Mp4,
    /// The `WebM` clip.
    Webm,
    /// One media row with no file behind it.
    Missing,
    /// No media rows at all.
    None,
}

impl NewDayMedia {
    fn planned(self, day: i64) -> Vec<Media> {
        match self {
            Self::Image => vec![Media::Stored(seed::still(day))],
            Self::Images => (0..3)
                .map(|n| Media::Stored(seed::still(day + n)))
                .collect(),
            Self::Mp4 => vec![Media::Stored(&seed::MP4)],
            Self::Webm => vec![Media::Stored(&seed::WEBM)],
            Self::Missing => vec![Media::Missing],
            Self::None => Vec::new(),
        }
    }
}

#[derive(Default, Deserialize)]
struct NewDay {
    day: Option<i64>,
    #[serde(default)]
    media: NewDayMedia,
    caption: Option<String>,
    posted_at: Option<i64>,
}

/// `POST /__e2e/series/{series}/days`
async fn add_day(
    State(harness): State<Arc<Harness>>,
    Path(wanted): Path<String>,
    body: Result<Option<Json<NewDay>>, JsonRejection>,
) -> Result<Json<Value>, Refusal> {
    let world = harness.world();
    let series = find_series(&world, &wanted).await?;
    let new = body?.map(|Json(new)| new).unwrap_or_default();
    let posts = &world.state.posts;

    // The bot offers a revoked series to nobody, so a day archived into one
    // would be a state leaf never leaves behind.
    if series.state == SeriesState::Revoked {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("series {} is revoked and takes no new days", series.id),
        ));
    }
    let day = match new.day {
        Some(day) => day,
        None => posts
            .max_day(series.id)
            .await?
            .map_or(series.start_day, |newest| newest + 1),
    };
    if posts.exists(series.id, day).await? {
        return Err(Refusal::new(
            StatusCode::CONFLICT,
            format!("day {day} of series {} is already archived", series.id),
        ));
    }
    // A day after the newest post rather than "now": the seed's dates are
    // fixed, and so is the clock of a browser looking at them.
    let posted_at = match new.posted_at {
        Some(posted_at) => posted_at,
        None => posts
            .latest_posted_at(series.id)
            .await?
            .map_or(seed::FIRST_POST_AT, |newest| newest + DAY_SECS),
    };
    let planned = PlannedDay {
        day,
        posted_at,
        caption: new.caption.unwrap_or_else(|| format!("Day {day}")),
        media: new.media.planned(day),
    };
    seed::archive(
        posts,
        world.state.store.as_ref(),
        &series,
        std::slice::from_ref(&planned),
    )
    .await?;

    let settings = world.state.guilds.get(&series.guild_id).await?;
    // The bot's own step after every archive: a sprout that now has the days
    // its guild asks for is published. `set_state_if` as there, so a series
    // revoked in the meantime stays revoked.
    let threshold = settings.as_ref().map(|settings| settings.sprout_threshold);
    let promoted = match threshold {
        Some(threshold)
            if series.state == SeriesState::Sprout
                && posts.count(series.id).await? >= threshold =>
        {
            let (from, to) = (SeriesState::Sprout, SeriesState::Active);
            world.state.series.set_state_if(series.id, from, to).await?
        }
        _ => false,
    };
    let state = if promoted {
        SeriesState::Active
    } else {
        series.state
    };

    let timezone = settings.map_or_else(|| "UTC".to_owned(), |settings| settings.timezone);
    Ok(Json(json!({
        "series_id": series.id,
        "day": day,
        "posted_at": posted_at,
        "local_date": localtime::local_date(posted_at, localtime::tz_or_utc(&timezone)),
        "state": state.as_str(),
        "promoted": promoted,
    })))
}

/// `DELETE /__e2e/series/{series}/days/{day}`
async fn remove_day(
    State(harness): State<Arc<Harness>>,
    Path((wanted, day)): Path<(String, i64)>,
) -> Result<StatusCode, Refusal> {
    let world = harness.world();
    let series = find_series(&world, &wanted).await?;
    let posts = &world.state.posts;
    if !posts.exists(series.id, day).await? {
        return Err(Refusal::new(
            StatusCode::NOT_FOUND,
            format!("day {day} of series {} is not archived", series.id),
        ));
    }
    // The bot's own order when a post is deleted in chat: the row, then the
    // files no other day still holds.
    for key in posts.delete(series.id, day).await? {
        let gone = world.state.store.delete(&ObjectPath::from(key)).await;
        gone.map_err(anyhow::Error::from)?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// The states a minted gallery session can be in.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionKind {
    /// What a sign-in just now would have produced.
    #[default]
    Fresh,
    /// Signed in just now, lapsing in a few seconds.
    Expiring,
    /// Lapsed a minute ago.
    Expired,
    /// Still valid, but signed in longer ago than a session may be renewed.
    Capped,
}

impl SessionKind {
    /// `(age, ttl)`: how long ago the sign-in was, and how long from now the
    /// token stays valid, both in seconds.
    const fn lifetime(self) -> (i64, i64) {
        match self {
            Self::Fresh => (0, SESSION_TTL_SECS),
            Self::Expiring => (0, EXPIRING_TTL_SECS),
            Self::Expired => (SESSION_TTL_SECS + 60, -60),
            Self::Capped => (SESSION_MAX_AGE_SECS + 60, CAPPED_TTL_SECS),
        }
    }
}

#[derive(Deserialize)]
struct SessionRequest {
    persona: String,
    #[serde(default)]
    kind: SessionKind,
    age_secs: Option<i64>,
    ttl_secs: Option<i64>,
}

/// `POST /__e2e/session`
async fn mint_session(
    State(harness): State<Arc<Harness>>,
    body: Result<Json<SessionRequest>, JsonRejection>,
) -> Result<Json<Value>, Refusal> {
    let Json(request) = body?;
    let persona = find_persona(&request.persona)?;
    let (age, ttl) = request.kind.lifetime();
    let age = request.age_secs.unwrap_or(age);
    let ttl = request.ttl_secs.unwrap_or(ttl);
    if age < 0 {
        return Err(Refusal::bad_request("age_secs cannot be negative"));
    }
    // `mint` takes the sign-in time and a lifetime counted from it, so a
    // sign-in `age` seconds ago that lasts `ttl` more is this.
    let auth_at = now_unix() - age;
    let token = harness.key.mint(persona.id, auth_at, age + ttl);
    Ok(Json(json!({
        "token": token,
        "access_token": format!("{ACCESS_TOKEN_PREFIX}{}", persona.key),
        "expires_in": ttl,
        "user_id": persona.id,
        "auth_at": auth_at,
        "exp": auth_at + age + ttl,
    })))
}

#[derive(Deserialize)]
struct AdminSessionRequest {
    persona: String,
    ttl_secs: Option<i64>,
}

/// `POST /__e2e/admin-session`
async fn mint_admin_session(
    State(harness): State<Arc<Harness>>,
    body: Result<Json<AdminSessionRequest>, JsonRejection>,
) -> Result<Json<Value>, Refusal> {
    let Json(request) = body?;
    let persona = find_persona(&request.persona)?;
    let guild_ids: Vec<String> = persona.manages.iter().map(|&g| g.to_owned()).collect();
    let ttl = request.ttl_secs.unwrap_or(ADMIN_TTL_SECS);
    let token = harness
        .key
        .mint_admin(persona.id, &guild_ids, now_unix(), ttl);
    Ok(Json(json!({
        "token": token,
        "user_id": persona.id,
        "guild_ids": guild_ids,
        "expires_in": ttl,
    })))
}

#[derive(Deserialize)]
struct AdminLogin {
    /// Whose code to come back with.
    persona: Option<String>,
    /// A code to come back with as it is, for one Discord will refuse.
    code: Option<String>,
}

/// `GET /__e2e/admin/login?persona=…` (or `?code=…`)
async fn admin_login(
    State(harness): State<Arc<Harness>>,
    Query(login): Query<AdminLogin>,
) -> Result<Redirect, Refusal> {
    let code = match (login.persona, login.code) {
        (Some(key), None) => format!("{CODE_PREFIX}{}", find_persona(&key)?.key),
        (None, Some(code)) if is_url_safe(&code) => code,
        _ => {
            return Err(Refusal::bad_request(
                "give either persona=<key> or code=<letters, digits, - and _>",
            ));
        }
    };
    // The state is made of URL-safe characters too, so nothing needs escaping.
    let state = harness
        .key
        .sign_oauth_state(now_unix(), OAUTH_STATE_TTL_SECS);
    Ok(Redirect::to(&format!(
        "/admin/callback?code={code}&state={state}"
    )))
}

/// `GET /__e2e/elsewhere`
async fn elsewhere() -> Redirect {
    // `Redirect::to`, as leaf's own `/admin/login` answers.
    Redirect::to(ELSEWHERE)
}

fn is_url_safe(code: &str) -> bool {
    !code.is_empty()
        && code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
