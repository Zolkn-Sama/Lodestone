use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::auth::{LoginRequest, MeResponse, RefreshRequest, RegisterRequest, UserResponse},
    error::AppError,
    extractors::auth_user::AuthUser,
    repositories::users,
    services,
    state::AppState,
};

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<impl IntoResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let tokens = services::auth::register(
        &state.db,
        &state.config.jwt_secret,
        body.email,
        body.password,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(tokens)))
}

pub async fn login(
    State(state): State<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let tokens = services::auth::login(
        &state.db,
        &state.config.jwt_secret,
        body.email,
        body.password,
    )
    .await?;

    Ok(Json(tokens))
}

/// GET /api/auth/me — profil + organisations de l'utilisateur (avec son rôle).
pub async fn me(
    State(state): State<AppState>,
    user: AuthUser, // exige un token valide
) -> Result<impl IntoResponse, AppError> {
    // L'extracteur nous a donné l'id ; on recharge l'utilisateur frais depuis la base.
    let profile = users::find_by_id(&state.db, user.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?; // token valide mais user supprimé -> 401
    let organizations = services::organizations::list_for_user(&state.db, user.user_id).await?;

    Ok(Json(MeResponse {
        user: UserResponse::from(profile),
        organizations,
    }))
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    let tokens =
        services::auth::refresh(&state.db, &state.config.jwt_secret, body.refresh_token).await?;
    Ok(Json(tokens))
}

pub async fn logout(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    services::auth::logout(&state.db, body.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}
