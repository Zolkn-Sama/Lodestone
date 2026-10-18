use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
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

    #[error("Forbidden")]
    Forbidden,

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Payload too large: {0}")]
    PayloadTooLarge(String),

    /// Fonctionnalité non configurée sur ce serveur (ex. facturation sans Stripe).
    #[error("Service unavailable: {0}")]
    Unavailable(String),

    /// Un service tiers (Stripe…) a échoué.
    #[error("Bad gateway: {0}")]
    Upstream(String),

    /// Limite du plan atteinte : 402, avec le détail pour proposer un upgrade.
    #[error("{}", .0.message())]
    QuotaExceeded(crate::services::quotas::QuotaExceeded),

    #[error("Internal server error")]
    Internal,

    #[error(transparent)]
    Database(#[from] sea_orm::DbErr),

    #[error(transparent)]
    Jwt(#[from] lodestone_auth::error::AuthError),
}

// ============================================================================
// LESSON 2: Implement IntoResponse for Custom Error
// ============================================================================

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
    /// Présent seulement pour un dépassement de quota.
    #[serde(skip_serializing_if = "Option::is_none")]
    quota: Option<crate::services::quotas::QuotaExceeded>,
}

impl From<crate::services::quotas::QuotaError> for AppError {
    fn from(e: crate::services::quotas::QuotaError) -> Self {
        match e {
            crate::services::quotas::QuotaError::Exceeded(q) => AppError::QuotaExceeded(q),
            crate::services::quotas::QuotaError::Database(e) => AppError::Database(e),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Not found".to_string()),
            AppError::InvalidInput(m) => (StatusCode::BAD_REQUEST, m.to_string()),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_string()),
            AppError::Forbidden => (StatusCode::FORBIDDEN, "Forbidden".to_string()),
            AppError::Conflict(m) => (StatusCode::CONFLICT, m.to_string()),
            AppError::PayloadTooLarge(m) => (StatusCode::PAYLOAD_TOO_LARGE, m.to_string()),
            AppError::QuotaExceeded(q) => (StatusCode::PAYMENT_REQUIRED, q.message()),
            AppError::Unavailable(m) => (StatusCode::SERVICE_UNAVAILABLE, m.to_string()),
            AppError::Upstream(m) => (StatusCode::BAD_GATEWAY, m.to_string()),
            AppError::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal server error".to_string(),
            ),

            AppError::Database(e) => {
                tracing::error!(error = %e, "Database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }

            AppError::Jwt(e) => {
                tracing::error!(error = %e, "erreur JWT");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Erreur interne".to_string(),
                )
            }
        };

        let quota = match self {
            AppError::QuotaExceeded(q) => Some(q),
            _ => None,
        };
        let body = ErrorResponse {
            error: message,
            code: status.as_u16(),
            quota,
        };

        (status, Json(body)).into_response()
    }
}
