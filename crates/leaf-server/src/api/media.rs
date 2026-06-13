//! Media proxy: streams an attachment's bytes from R2 behind a signed URL.
//!
//! No Bearer auth here — the `sig`/`exp` query pair is the capability (see
//! `auth::MediaSigner`), because the gallery loads these via `<img src>`.
//! Bytes stream from object storage rather than buffering, and responses
//! carry immutable cache headers so Discord's proxy and Cloudflare's edge
//! serve repeat views without touching R2 (see PLAN.md § caching).

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use object_store::path::Path as ObjectPath;
use serde::Deserialize;

use crate::api::auth::{DiscordApi, now_unix};
use crate::api::error::ApiError;
use crate::api::state::ApiState;

/// Signed-URL query parameters.
#[derive(Debug, Deserialize)]
pub struct MediaQuery {
    /// Expiry (unix seconds) the signature was minted for.
    exp: i64,
    /// Base64 HMAC over `media\0{attachment_id}\0{exp}`.
    sig: String,
    /// Any value selects the thumbnail variant.
    thumb: Option<String>,
}

/// Immutable, year-long cache: archive media never changes under a key.
const CACHE_CONTROL: &str = "public, max-age=31536000, immutable";

/// `GET /api/media/:attachment_id` — verify the signature, then stream the
/// original (or `?thumb=1` the thumbnail) from R2.
pub async fn media<D: DiscordApi>(
    State(st): State<ApiState<D>>,
    Path(attachment_id): Path<String>,
    Query(q): Query<MediaQuery>,
) -> Result<Response, ApiError> {
    if !st
        .key
        .verify_media(&attachment_id, q.exp, &q.sig, now_unix())
    {
        return Err(ApiError::Forbidden);
    }

    let Some((original_key, thumb_key, content_type)) =
        st.posts.media_location(&attachment_id).await?
    else {
        return Err(ApiError::NotFound);
    };

    let want_thumb = q.thumb.is_some();
    // Missing bytes (imported placeholder) → 404. A future enhancement can
    // 302 to a refreshed Discord CDN URL when the source still exists.
    let (key, ct) = if want_thumb {
        (thumb_key, "image/webp".to_owned())
    } else {
        (original_key, content_type)
    };
    let Some(key) = key else {
        return Err(ApiError::NotFound);
    };

    let got = match st.store.get(&ObjectPath::from(key)).await {
        Ok(got) => got,
        Err(object_store::Error::NotFound { .. }) => return Err(ApiError::NotFound),
        Err(e) => {
            tracing::error!(error = %e, attachment = %attachment_id, "media fetch from store failed");
            return Err(ApiError::Internal);
        }
    };

    let body = Body::from_stream(got.into_stream());
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, ct),
            (header::CACHE_CONTROL, CACHE_CONTROL.to_owned()),
        ],
        body,
    )
        .into_response())
}
