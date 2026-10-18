use axum::http::{Method, header};
use axum::{
    Router,
    extract::DefaultBodyLimit,
    routing::{get, patch, post},
};
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};

use crate::{handlers, state::AppState, ws};

pub fn app_router(state: AppState) -> Router {
    // Seules les origines configurées (le front) peuvent appeler l'API depuis
    // un navigateur. Le token passe en en-tête : pas besoin de cookies.
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(state.config.cors_allowed_origins.clone()))
        .allow_methods([Method::GET, Method::POST, Method::PATCH, Method::DELETE])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .route("/", get(handlers::health::root))
        .route("/health", get(handlers::health::health))
        .route("/health/ready", get(handlers::health::ready))
        .route("/ws", get(ws::handler::ws_handler))
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
        .route("/api/orgs/{id}/usage", get(handlers::organizations::usage))
        .route("/api/orgs/{id}/billing", get(handlers::billing::status))
        .route(
            "/api/orgs/{id}/billing/checkout",
            post(handlers::billing::checkout),
        )
        .route(
            "/api/orgs/{id}/billing/interval",
            post(handlers::billing::change_interval),
        )
        .route("/api/billing/webhook", post(handlers::billing::webhook))
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
                .layer(DefaultBodyLimit::max(handlers::documents::MAX_UPLOAD_BYTES))
                .get(handlers::documents::list),
        )
        .route(
            "/api/orgs/{id}/conversations",
            post(handlers::conversations::create).get(handlers::conversations::list),
        )
        .route(
            "/api/conversations/{id}",
            get(handlers::conversations::get).delete(handlers::conversations::delete),
        )
        .route(
            "/api/documents/{id}",
            get(handlers::documents::get).delete(handlers::documents::delete),
        )
        .route(
            "/api/orgs/{id}/members/{user_id}",
            patch(handlers::members::change_role).delete(handlers::members::remove),
        )
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .with_state(state)
}
