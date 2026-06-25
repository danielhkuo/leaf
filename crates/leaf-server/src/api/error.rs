//! API error type → HTTP status + JSON problem body.
//!
//! Deliberate ambiguity: "not a member", "series not visible to you", and
//! "no such series" all surface as 404 so the API never reveals the
//! existence of something the caller may not see (mirrors the bot's
//! not-found-equals-forbidden stance).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// An API failure with a status and a stable machine-readable code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    /// Missing or invalid session token.
    Unauthorized,
    /// Authenticated but not permitted (non-member of the guild).
    Forbidden,
    /// Resource missing — or hidden from this caller.
    NotFound,
    /// Malformed request (bad body, bad path value).
    BadRequest,
    /// Upstream (Discord/R2) or internal failure.
    Internal,
    /// A specific failure carrying its own status and machine code (used by
    /// the creator API, where the client distinguishes e.g. `name_taken`
    /// from `invalid_channel`).
    Coded(StatusCode, &'static str),
}

impl ApiError {
    const fn parts(self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::BadRequest => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            Self::Coded(status, code) => (status, code),
        }
    }
}

#[derive(Serialize)]
struct Body {
    error: &'static str,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        (status, Json(Body { error: code })).into_response()
    }
}

/// Repository errors are internal; never leak their detail to the client.
impl From<leaf_core::db::DbError> for ApiError {
    fn from(e: leaf_core::db::DbError) -> Self {
        tracing::error!(error = %e, "db error in API handler");
        Self::Internal
    }
}
