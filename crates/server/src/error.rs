use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("You need to sign in.")]
    Unauthorized,
    #[error("Wrong username or password.")]
    InvalidCredentials,
    #[error("You don't have permission to do that.")]
    Forbidden,
    /// An age-restricted channel the member hasn't said they're 18+ for.
    #[error("This channel is age-restricted. You must be 18+ to view it.")]
    AgeRestricted,
    #[error("Not found.")]
    NotFound,
    /// A file held back pending a safety review (HTTP 451, for everyone).
    #[error("This file was removed pending review.")]
    Removed,
    /// A picture the image detector is holding back (HTTP 451 too).
    #[error("This picture is held for review.")]
    Held,
    #[error("{0}")]
    Conflict(String),
    /// Not allowed, with a reason to show (e.g. sign-ups are closed).
    #[error("{0}")]
    Refused(String),
    /// Gone for good, with a reason to show (an outhouse that was shut down).
    #[error("{0}")]
    Gone(String),
    /// Too many tries (sign-in, sign-up, invite look-ups): slow down.
    #[error("{0}")]
    TooManyRequests(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = match &self {
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized | AppError::InvalidCredentials => StatusCode::UNAUTHORIZED,
            AppError::Forbidden | AppError::AgeRestricted | AppError::Refused(_) => {
                StatusCode::FORBIDDEN
            }
            AppError::NotFound | AppError::Db(sqlx::Error::RowNotFound) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::Removed | AppError::Held => StatusCode::UNAVAILABLE_FOR_LEGAL_REASONS,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::Gone(_) => StatusCode::GONE,
            AppError::Db(_) | AppError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = ?self, "internal error");
            "Something went wrong on the server.".to_string()
        } else if status == StatusCode::NOT_FOUND {
            "Not found.".to_string()
        } else {
            self.to_string()
        };
        if matches!(self, AppError::AgeRestricted) {
            return (
                status,
                Json(json!({ "error": message, "code": "age_restricted" })),
            )
                .into_response();
        }
        (status, Json(json!({ "error": message }))).into_response()
    }
}
