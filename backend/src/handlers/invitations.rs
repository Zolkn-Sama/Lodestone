use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::invitations::{AcceptInvitationRequest, CreateInvitationRequest},
    error::AppError,
    extractors::{
        auth_user::AuthUser,
        org_member::{Admin, OrgMember},
    },
    services,
    state::AppState,
};

/// POST /api/orgs/{id}/invitations — admin : inviter un employé.
pub async fn create(
    State(state): State<AppState>,
    member: OrgMember<Admin>,
    Json(body): Json<CreateInvitationRequest>,
) -> Result<impl IntoResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let invitation = services::invitations::invite(
        &state.db,
        member.org_id,
        member.user_id,
        member.role,
        body.email,
        body.role,
    )
    .await?;
    Ok((StatusCode::CREATED, Json(invitation)))
}

/// POST /api/invitations/accept — connecté, avec l'email invité.
pub async fn accept(
    State(state): State<AppState>,
    user: AuthUser,
    Json(body): Json<AcceptInvitationRequest>,
) -> Result<impl IntoResponse, AppError> {
    let org = services::invitations::accept(&state.db, user.user_id, body.token).await?;
    // Un membre de plus : la quantité de l'abonnement Pro suit.
    services::billing::spawn_seat_sync(state.db.clone(), state.billing.clone(), org.id);
    Ok(Json(org))
}
