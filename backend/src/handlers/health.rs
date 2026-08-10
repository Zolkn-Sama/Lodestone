use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

use serde_json::json;
use tracing::warn;
use crate::state::AppState;


pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.ping().await {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "status": "ready", "db": "up" })),
        ),
        Err(e) => {
            warn!(error = %e, "base injoignable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded", "db": "down" })),
            )
        }
    }
}

pub async fn root() -> impl IntoResponse {
    Json(json!({ "services": "lodestone", "status": "running" }))
}

pub async fn boom() -> crate::error::Result<Json<serde_json::Value>> {
    Err(crate::error::AppError::InvalidInput("exemple".into()))
}

pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}
