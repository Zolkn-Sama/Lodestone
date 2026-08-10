use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use validator::Validate;

use crate::{
    dto::organizations::{CreateOrgRequest, OrgResponse},
    error::AppError,
    extractors::auth_user::AuthUser,
    services,
    state::AppState,
};

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,                               // il faut être connecté
    Json(body): Json<CreateOrgRequest>,           // Json en DERNIER (il consomme le corps)
) -> Result<impl IntoResponse, AppError> {
    body.validate().map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let org = services::organizations::create_org(&state.db, user.user_id, body.name).await?;
    Ok((StatusCode::CREATED, Json(OrgResponse::from(org))))
}