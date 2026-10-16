use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, patch, post},
};
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
        .route(
            "/api/orgs",
            post(handlers::organizations::create).get(handlers::organizations::list_mine),
        )
        .route(
            "/api/orgs/{id}",
            get(handlers::organizations::get)
                .patch(handlers::organizations::rename)
                .delete(handlers::organizations::delete),
        )
        .route("/api/orgs/{id}/members", get(handlers::members::list))
        .route(
            "/api/orgs/{id}/invitations",
            post(handlers::invitations::create),
        )
        .route(
            "/api/invitations/accept",
            post(handlers::invitations::accept),
        )
        .route(
            "/api/orgs/{id}/documents",
            post(handlers::documents::upload)
                .layer(DefaultBodyLimit::max(handlers::documents::MAX_UPLOAD_BYTES)),
        )
        .route(
            "/api/orgs/{id}/members/{user_id}",
            patch(handlers::members::change_role).delete(handlers::members::remove),
        )
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive()) // à restreindre plus tard
        .with_state(state)
}
