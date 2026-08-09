use axum::{Json, extract::State, response::IntoResponse};

use serde_json::json;
use validator::Validate;

use crate::dto::auth::RefreshRequest;
use axum::http::StatusCode;

use crate::{
    dto::auth::{LoginRequest, RegisterRequest},
    error::AppError,
    services,
    state::AppState,
};

pub async fn register(
    State(state): State<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<impl IntoResponse, AppError> {
    body.validate().map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let tokens = services::auth::register(
        &state.db,
        &state.config.jwt_secret,
        body.email,
        body.password,
    )
        .await?;

    Ok((axum::http::StatusCode::CREATED, Json(tokens)))
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

use crate::{
    dto::auth::UserResponse,
    extractors::auth_user::AuthUser,
    repositories::users,
};

pub async fn me(
    State(state): State<AppState>,
    user: AuthUser,                       // <-- la magie : exige un token valide
) -> Result<impl IntoResponse, AppError> {
    // L'extracteur nous a donné l'id ; on recharge l'utilisateur frais depuis la base.
    let user = users::find_by_id(&state.db, user.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;   // token valide mais user supprimé -> 401

    Ok(Json(UserResponse::from(user)))
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    let tokens = services::auth::refresh(&state.db, &state.config.jwt_secret, body.refresh_token).await?;
    Ok(Json(tokens))
}

pub async fn logout(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    services::auth::logout(&state.db, body.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}