//! API error type → HTTP status + JSON problem body.
//!
//! The body is `{"error": code, "message"?: sentence, "retryable"?: bool}`:
//! `error` is a stable machine code, `message` a sentence that is safe to
//! show to the person, and `retryable` says whether repeating the same
//! request can help (sent only where the status alone would mislead).
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// A coded failure that also tells the person what happened, and the
    /// client whether trying again can help. Build one with
    /// [`ApiError::with_message`] / [`ApiError::retryable`].
    Detailed(Problem),
}

/// The parts of an [`ApiError::Detailed`] response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// HTTP status.
    pub status: StatusCode,
    /// Stable machine code (the body's `error`).
    pub code: &'static str,
    /// A sentence for the person: what happened and what to do next.
    pub message: Option<String>,
    /// Whether repeating the same request may succeed.
    pub retryable: Option<bool>,
}

impl ApiError {
    /// Discord could not be reached (or answered with an error) while leaf
    /// needed it. A passing condition: 503, and worth retrying.
    #[must_use]
    pub fn discord_unavailable() -> Self {
        Self::Coded(StatusCode::SERVICE_UNAVAILABLE, "discord_unavailable")
            .with_message("leaf can't reach Discord right now. Try again in a moment.")
            .retryable(true)
    }

    /// A well-formed request whose values leaf will not accept: 422 with a
    /// code naming the rule and a sentence naming the field.
    #[must_use]
    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::Coded(StatusCode::UNPROCESSABLE_ENTITY, code).with_message(message)
    }

    /// Attaches the sentence shown to the person.
    #[must_use]
    pub fn with_message(self, message: impl Into<String>) -> Self {
        let mut problem = self.into_problem();
        problem.message = Some(message.into());
        Self::Detailed(problem)
    }

    /// States whether repeating the request may succeed.
    #[must_use]
    pub fn retryable(self, retryable: bool) -> Self {
        let mut problem = self.into_problem();
        problem.retryable = Some(retryable);
        Self::Detailed(problem)
    }

    /// The HTTP status this error answers with.
    #[must_use]
    pub const fn status(&self) -> StatusCode {
        self.parts().0
    }

    /// The stable machine code in the body's `error` field.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.parts().1
    }

    const fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            Self::NotFound => (StatusCode::NOT_FOUND, "not_found"),
            Self::BadRequest => (StatusCode::BAD_REQUEST, "bad_request"),
            Self::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
            Self::Coded(status, code) => (*status, *code),
            Self::Detailed(problem) => (problem.status, problem.code),
        }
    }

    fn into_problem(self) -> Problem {
        match self {
            Self::Detailed(problem) => problem,
            other => {
                let (status, code) = other.parts();
                Problem {
                    status,
                    code,
                    message: None,
                    retryable: None,
                }
            }
        }
    }
}

#[derive(Serialize)]
struct Body {
    error: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    retryable: Option<bool>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let problem = self.into_problem();
        let body = Body {
            error: problem.code,
            message: problem.message,
            retryable: problem.retryable,
        };
        (problem.status, Json(body)).into_response()
    }
}

/// Repository errors are internal; never leak their detail to the client.
impl From<leaf_core::db::DbError> for ApiError {
    fn from(e: leaf_core::db::DbError) -> Self {
        tracing::error!(error = %e, "db error in API handler");
        Self::Internal
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::indexing_slicing,
        reason = "tests may panic; JSON indexing is fine in assertions"
    )]

    use super::*;

    async fn body(e: ApiError) -> (StatusCode, serde_json::Value) {
        let resp = e.into_response();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 1 << 16)
            .await
            .unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn plain_errors_send_only_the_code() {
        for (e, status, code) in [
            (ApiError::Unauthorized, 401, "unauthorized"),
            (ApiError::Forbidden, 403, "forbidden"),
            (ApiError::NotFound, 404, "not_found"),
            (ApiError::BadRequest, 400, "bad_request"),
            (ApiError::Internal, 500, "internal"),
            (
                ApiError::Coded(StatusCode::CONFLICT, "name_taken"),
                409,
                "name_taken",
            ),
        ] {
            let (got, v) = body(e).await;
            assert_eq!(got.as_u16(), status);
            assert_eq!(v, serde_json::json!({ "error": code }));
        }
    }

    #[tokio::test]
    async fn message_and_retryable_are_added_to_the_body() {
        let (status, v) = body(ApiError::discord_unavailable()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(v["error"], "discord_unavailable");
        assert_eq!(v["retryable"], true);
        assert!(v["message"].as_str().unwrap().contains("Discord"));

        let (status, v) = body(ApiError::unprocessable("invalid_limit", "Too low.")).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            v,
            serde_json::json!({ "error": "invalid_limit", "message": "Too low." })
        );
    }

    #[test]
    fn builders_keep_status_and_code() {
        let e = ApiError::NotFound.with_message("gone").retryable(false);
        assert_eq!(e.status(), StatusCode::NOT_FOUND);
        assert_eq!(e.code(), "not_found");
        assert_eq!(
            e,
            ApiError::Detailed(Problem {
                status: StatusCode::NOT_FOUND,
                code: "not_found",
                message: Some("gone".to_owned()),
                retryable: Some(false),
            })
        );
    }
}
