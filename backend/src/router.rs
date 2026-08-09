use axum::{routing::get, routing::post, Router};
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::{handlers, state::AppState};

// ----- Router (ira plus tard dans src/router.rs) -----------------------------
pub fn app_router(state: AppState) -> Router {
    Router::new()
        .route("/", get(handlers::health::root)) // <-- nouvelle ligne
        .route("/health", get(handlers::health::health))
        .route("/health/ready", get(handlers::health::ready))
        .route("/boom", get(handlers::health::boom))

        .route("/api/auth/register", post(handlers::auth::register))
        .route("/api/auth/login", post(handlers::auth::login))
        .route("/api/auth/me", get(handlers::auth::me))

        .route("/api/auth/refresh", post(handlers::auth::refresh))
        .route("/api/auth/logout", post(handlers::auth::logout))

        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive()) // à restreindre plus tard
        .with_state(state)
}

