use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use uuid::Uuid;

use crate::{
    dto::members::ChangeRoleRequest,
    error::AppError,
    extractors::org_member::{Admin, OrgMember, Owner},
    services,
    state::AppState,
};

/// GET /api/orgs/{id}/members — membre.
pub async fn list(
    State(state): State<AppState>,
    member: OrgMember,
) -> Result<impl IntoResponse, AppError> {
    let members = services::members::list(&state.db, member.org_id).await?;
    Ok(Json(members))
}

/// DELETE /api/orgs/{id}/members/{user_id} — admin.
pub async fn remove(
    State(state): State<AppState>,
    member: OrgMember<Admin>,
    Path((_, user_id)): Path<(Uuid, Uuid)>,
) -> Result<impl IntoResponse, AppError> {
    services::members::remove(&state.db, member.org_id, member.role, user_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// PATCH /api/orgs/{id}/members/{user_id} — owner : changer un rôle.
pub async fn change_role(
    State(state): State<AppState>,
    member: OrgMember<Owner>,
    Path((_, user_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<ChangeRoleRequest>,
) -> Result<impl IntoResponse, AppError> {
    let updated =
        services::members::change_role(&state.db, member.org_id, user_id, body.role).await?;
    Ok(Json(updated))
}
