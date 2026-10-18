use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::invitations::{AcceptInvitationRequest, CreateInvitationRequest},
    error::AppError,
    extractors::{
        auth_user::AuthUser,
        org_member::{Admin, OrgMember},
    },
    mail::{self, Email},
    repositories, services,
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

    let mut invitation = services::invitations::invite(
        &state.db,
        member.org_id,
        member.user_id,
        member.role,
        body.email,
        body.role,
    )
    .await?;

    if state.mailer.is_enabled() {
        let org = services::organizations::get(&state.db, member.org_id).await?;
        let inviter = repositories::users::find_by_id(&state.db, member.user_id)
            .await?
            .ok_or(AppError::Unauthorized)?;
        mail::send_in_background(
            state.mailer.clone(),
            Email {
                to: invitation.email.clone(),
                subject: format!("Invitation à rejoindre {} sur Lodestone", org.name),
                body: format!(
                    "Bonjour,\n\n\
                     {} vous invite à rejoindre l'organisation « {} » sur Lodestone.\n\n\
                     Pour accepter, connectez-vous (ou créez votre compte) avec cette adresse \
                     e-mail, puis ouvrez ce lien (valable 7 jours) :\n\n\
                     {}/invitations/accept?token={}\n",
                    inviter.email, org.name, state.config.app_base_url, invitation.token
                ),
            },
        );
        invitation.email_sent = true;
    }
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
