use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::organizations::{CreateOrgRequest, MyOrgResponse, OrgResponse, RenameOrgRequest},
    error::AppError,
    extractors::{
        auth_user::AuthUser,
        org_member::{Admin, OrgMember, Owner},
    },
    services,
    state::AppState,
};

pub async fn create(
    State(state): State<AppState>,
    user: AuthUser,                     // il faut être connecté
    Json(body): Json<CreateOrgRequest>, // Json en DERNIER (il consomme le corps)
) -> Result<impl IntoResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let org = services::organizations::create_org(
        &state.db,
        user.user_id,
        body.name,
        state.config.new_org_plan,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(OrgResponse::from(org))))
}

/// GET /api/orgs — mes orgs, avec mon rôle dans chacune.
pub async fn list_mine(
    State(state): State<AppState>,
    user: AuthUser,
) -> Result<impl IntoResponse, AppError> {
    let orgs = services::organizations::list_for_user(&state.db, user.user_id).await?;
    Ok(Json(orgs))
}

/// GET /api/orgs/{id} — membre.
pub async fn get(
    State(state): State<AppState>,
    member: OrgMember,
) -> Result<impl IntoResponse, AppError> {
    let org = services::organizations::get(&state.db, member.org_id).await?;
    Ok(Json(MyOrgResponse::new(org, member.role)))
}

/// PATCH /api/orgs/{id} — admin : renommer.
pub async fn rename(
    State(state): State<AppState>,
    member: OrgMember<Admin>,
    Json(body): Json<RenameOrgRequest>,
) -> Result<impl IntoResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let org = services::organizations::rename(&state.db, member.org_id, body.name).await?;
    Ok(Json(MyOrgResponse::new(org, member.role)))
}

/// DELETE /api/orgs/{id} — owner : supprime l'org et tout ce qui en dépend.
pub async fn delete(
    State(state): State<AppState>,
    member: OrgMember<Owner>,
) -> Result<impl IntoResponse, AppError> {
    services::organizations::delete(&state.db, member.org_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/orgs/{id}/usage — membre : plan, limites et consommation du mois.
pub async fn usage(
    State(state): State<AppState>,
    member: OrgMember,
) -> Result<impl IntoResponse, AppError> {
    let usage = services::organizations::usage(&state.db, member.org_id).await?;
    Ok(Json(usage))
}
