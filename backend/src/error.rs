
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json
};
use serde::Serialize;
use thiserror::Error;


/// Alias pratique : handlers renvoient `Result<Json<...>>`.
pub type Result<T> = std::result::Result<T, AppError>;

#[derive(Error, Debug)]
#[allow(dead_code)] // Variants shown for demonstration
pub enum AppError {
    #[error("Not found")]
    NotFound,

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Unauthorized")]
    Unauthorized,

    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),

    #[error("Internal server error")]
    Internal,
}

// ============================================================================
// LESSON 2: Implement IntoResponse for Custom Error
// ============================================================================

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Not found".to_string()),
            AppError::InvalidInput(m) => (StatusCode::BAD_REQUEST, m.to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()),
            AppError::Database(e) => {
                tracing::error!(error = %e, "Database error");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error".to_string())
            }
            AppError::Internal => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string()),
        };

        let body = ErrorResponse {
            error: message,
            code: status.as_u16(),
        };

        (status, Json(body)).into_response()
    }
}
