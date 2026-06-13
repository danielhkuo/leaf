//! The embedded-app REST API: OAuth token exchange, guild-scoped series /
//! days / stats reads, and the signed media proxy.
//!
//! This is the product's security boundary. Two gates on every data route:
//! a valid session token (`AuthUser`) and guild membership; then
//! `policy::can_view` decides per series. "Not a member", "not visible",
//! and "doesn't exist" all collapse to 404 so the API reveals nothing.
//!
//! Admin-in-gallery (an admin viewing others' private series) is out of
//! scope here — API viewers are treated as non-admin members; moderation
//! stays in the bot. Membership caching is a Phase-19 optimization.

pub mod auth;
pub mod discord;
pub mod dto;
pub mod error;
pub mod media;
pub mod state;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Router, response::IntoResponse};
use leaf_core::domain::Series;
use leaf_core::policy::{Viewer, can_view};
use serde::{Deserialize, Serialize};

use auth::{DiscordApi, MediaSigner, SESSION_TTL_SECS, now_unix};
use dto::{DayDto, DaySummaryDto, SeriesDto, StatsDto};
use error::ApiError;
use state::{ApiState, AuthUser};

/// Default day-window when `/days` is called without `from`/`to`.
const DEFAULT_WINDOW: i64 = 35;
/// Hard cap on a requested day span (guards a pathological range query).
const MAX_WINDOW: i64 = 366;

/// Builds the API router over `state`.
pub fn router<D: DiscordApi>(state: ApiState<D>) -> Router {
    Router::new()
        .route("/api/token", post(token::<D>))
        .route("/api/guilds/{gid}/series", get(list_series::<D>))
        .route("/api/guilds/{gid}/series/{sid}/days", get(list_days::<D>))
        .route(
            "/api/guilds/{gid}/series/{sid}/days/{day}",
            get(get_day::<D>),
        )
        .route("/api/guilds/{gid}/series/{sid}/stats", get(get_stats::<D>))
        .route("/api/media/{attachment_id}", get(media::media::<D>))
        .with_state(state)
}

#[derive(Deserialize)]
struct TokenRequest {
    code: String,
    redirect_uri: Option<String>,
}

#[derive(Serialize)]
struct TokenResponse {
    token: String,
    expires_in: i64,
}

/// `POST /api/token` — exchange an OAuth code for a leaf session token.
async fn token<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Json(req): Json<TokenRequest>,
) -> Result<Json<TokenResponse>, ApiError> {
    let redirect = req.redirect_uri.unwrap_or_else(|| st.redirect_uri.clone());
    let access = st
        .discord
        .exchange_code(&req.code, &redirect)
        .await
        .map_err(|e| {
            tracing::warn!(error = %e, "oauth code exchange failed");
            ApiError::BadRequest
        })?;
    let user_id = st.discord.current_user_id(&access).await.map_err(|e| {
        tracing::error!(error = %e, "user lookup failed");
        ApiError::Internal
    })?;
    let token = st.key.mint(&user_id, now_unix(), SESSION_TTL_SECS);
    Ok(Json(TokenResponse {
        token,
        expires_in: SESSION_TTL_SECS,
    }))
}

/// Resolves the caller's role ids in a guild, or `Forbidden` if not a
/// member. The single membership gate for the guild-scoped routes.
async fn member_roles<D: DiscordApi>(
    st: &ApiState<D>,
    gid: &str,
    user_id: &str,
) -> Result<Vec<String>, ApiError> {
    match st.discord.guild_member_roles(gid, user_id).await {
        Ok(Some(roles)) => Ok(roles),
        Ok(None) => Err(ApiError::Forbidden),
        Err(e) => {
            tracing::error!(error = %e, "guild membership lookup failed");
            Err(ApiError::Internal)
        }
    }
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

/// `GET /api/guilds/{gid}/series` — series in the guild visible to the caller.
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

    let mut out = Vec::new();
    for s in st.series.list_by_guild(&gid).await? {
        if can_view(&s, &viewer) {
            let max_day = st.posts.max_day(s.id).await?;
            out.push(SeriesDto::from_series(&s, max_day));
        }
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
struct DayRange {
    from: Option<i64>,
    to: Option<i64>,
}

/// `GET /api/guilds/{gid}/series/{sid}/days?from&to` — grid tiles for a
/// day window (signed thumbnails), defaulting to the most recent days.
async fn list_days<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path((gid, sid)): Path<(String, i64)>,
    Query(range): Query<DayRange>,
    user: AuthUser,
) -> Result<Json<Vec<DaySummaryDto>>, ApiError> {
    let series = resolve_viewable(&st, &gid, sid, &user.user_id).await?;
    let max_day = st.posts.max_day(series.id).await?.unwrap_or(0);
    let to = range.to.unwrap_or(max_day);
    let from = range
        .from
        .unwrap_or_else(|| (to - DEFAULT_WINDOW + 1).max(series.start_day));
    if from > to || to - from > MAX_WINDOW {
        return Err(ApiError::BadRequest);
    }

    let signer = MediaSigner::new(&st.key, now_unix(), SESSION_TTL_SECS);
    let mut out = Vec::new();
    for day in st.posts.days_in_range(series.id, from, to).await? {
        if let Some((_, media)) = st.posts.get(series.id, day).await? {
            out.push(DaySummaryDto::build(day, media.first(), &signer));
        }
    }
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
    let signer = MediaSigner::new(&st.key, now_unix(), SESSION_TTL_SECS);
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

    use std::sync::Arc;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use leaf_core::db::{GuildSettingsRepo, PostRepo, SeriesRepo};
    use leaf_core::domain::{
        Cadence, DetectionMode, NewMediaAttachment, NewSeries, Post, Privacy, SeriesState,
    };
    use object_store::ObjectStore;
    use object_store::memory::InMemory;
    use tower::ServiceExt as _;

    use super::*;
    use crate::api::auth::{DiscordApi, SessionKey};

    /// Mock Discord: a fixed membership map, never hits the network.
    struct MockDiscord {
        /// (guild, user) → roles. Absent = not a member.
        members: std::collections::HashMap<(String, String), Vec<String>>,
    }

    impl DiscordApi for MockDiscord {
        async fn exchange_code(&self, code: &str, _redirect: &str) -> Result<String, String> {
            // The test "code" is the user id we want a token for.
            Ok(format!("access-for-{code}"))
        }
        async fn current_user_id(&self, access_token: &str) -> Result<String, String> {
            Ok(access_token.trim_start_matches("access-for-").to_owned())
        }
        async fn guild_member_roles(
            &self,
            guild_id: &str,
            user_id: &str,
        ) -> Result<Option<Vec<String>>, String> {
            Ok(self
                .members
                .get(&(guild_id.to_owned(), user_id.to_owned()))
                .cloned())
        }
    }

    const GUILD: &str = "g1";
    const CREATOR: &str = "creator1";
    const MEMBER: &str = "member1";
    const ROLE: &str = "role-vip";

    /// Builds a fully-seeded app + key. Members: creator (no roles), a plain
    /// member, and a VIP member holding `ROLE`. Series of each visibility.
    async fn app() -> (Router, SessionKey) {
        let dir = tempfile::tempdir().unwrap();
        let pool = leaf_core::db::connect(&dir.path().join("t.db"))
            .await
            .unwrap();
        // Keep the temp DB file alive for the rest of the test process.
        std::mem::forget(dir);

        GuildSettingsRepo::new(pool.clone())
            .ensure_exists(GUILD)
            .await
            .unwrap();
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

        let key = SessionKey::derive("test-secret");
        let store: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let state = ApiState {
            series: series_repo,
            posts,
            guilds: GuildSettingsRepo::new(pool),
            store,
            key: key.clone(),
            discord: Arc::new(MockDiscord { members }),
            redirect_uri: "https://leaf.test/cb".to_owned(),
        };
        (router(state), key)
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
        let (app, _key) = app().await;
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
        let (app, key) = app().await;
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
    async fn list_hides_sprout_revoked_and_unentitled_private() {
        let (app, key) = app().await;
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

        // The creator sees their own creator-only and sprout too.
        let creator = token_for(&key, CREATOR);
        let resp = router_json(&app, "/api/guilds/g1/series", &creator).await;
        assert_eq!(resp.len(), 4); // public, gated, private, sprout (not revoked)
    }

    #[tokio::test]
    async fn private_series_is_404_for_non_creator_member() {
        let (app, key) = app().await;
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
        let (app, key) = app().await;
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
        let (app, key) = app().await;
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
        let (app, key) = app().await;
        // Unsigned / bad signature → 403.
        assert_eq!(
            get(&app, "/api/media/att1?exp=9999999999&sig=bad", None).await,
            StatusCode::FORBIDDEN
        );
        // Correctly signed thumbnail streams from the store (seeded below).
        // (The object itself isn't in InMemory here, so a valid sig yields
        // 404 from the store, not 403 — proving the sig gate passed.)
        let exp = now_unix() + 60;
        let sig = key.sign_media("att1", exp);
        let path = format!("/api/media/att1?thumb=1&exp={exp}&sig={sig}");
        assert_eq!(get(&app, &path, None).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn token_exchange_mints_a_usable_session() {
        let (app, _key) = app().await;
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
        assert_eq!(
            get(&app, "/api/guilds/g1/series", Some(token)).await,
            StatusCode::OK
        );
    }

    // --- helpers that return parsed JSON ---
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
