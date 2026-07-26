use axum::{
    extract::State, http::StatusCode, response::IntoResponse, routing::get, Json, Router,
};

use tower_http::{cors::CorsLayer, trace::TraceLayer};

use serde_json::json;
use tracing::warn;

use crate::state::AppState;




/// Readiness : prêt à servir ? Pingue la base.
async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    match state.db.ping().await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ready", "db": "up" }))),
        Err(e) => {
            warn!(error = %e, "base injoignable");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "status": "degraded", "db": "down" })),
            )
        }
    }
}

async fn root() -> impl IntoResponse {
    Json(json!({ "service": "lodestone", "status": "running" }))
}

async fn boom() -> crate::error::Result<Json<serde_json::Value>> {
    Err(crate::error::AppError::InvalidInput("exemple".into()))
}

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

// ----- Router (ira plus tard dans src/router.rs) -----------------------------
pub fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))          // <-- nouvelle ligne
        .route("/health", get(health))
        .route("/health/ready", get(ready))
        .route("/boom", get(boom))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())   // à restreindre plus tard
        .with_state(state)
}

