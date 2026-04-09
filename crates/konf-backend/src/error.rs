//! Application-level error types.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Application error that converts to an HTTP response.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)] // Variants used as API grows in later phases
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("internal error: {0}")]
    Internal(String),

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("config error: {0}")]
    Config(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".into(),
            ),
            AppError::Database(e) => {
                tracing::error!(error = %e, "Database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error".into())
            }
            AppError::Config(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Configuration error".into(),
            ),
        };

        let body = axum::Json(json!({
            "error": message,
            "status": status.as_u16(),
        }));

        (status, body).into_response()
    }
}
