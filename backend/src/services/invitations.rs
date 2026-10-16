use chrono::{Duration, Utc};
use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use crate::{
    dto::{invitations::InvitationCreatedResponse, organizations::MyOrgResponse},
    error::AppError,
    repositories::{self, invitations::NewInvitation},
};
use lodestone_auth::refresh;
use lodestone_core::role::Role;

const INVITATION_TTL_DAYS: i64 = 7;

/// Invite `email` dans l'org. Règles :
/// - seul un owner peut inviter à un autre rôle que `member`
///   (sinon un admin contournerait « changer un rôle = owner ») ;
/// - inviter quelqu'un déjà membre -> 409 ;
/// - ré-inviter le même email remplace l'invitation en attente.
pub async fn invite(
    db: &DatabaseConnection,
    org_id: Uuid,
    inviter_id: Uuid,
    inviter_role: Role,
    email: String,
    role: Role,
) -> Result<InvitationCreatedResponse, AppError> {
    if role != Role::Member && inviter_role != Role::Owner {
        return Err(AppError::Forbidden);
    }
    let email = email.trim().to_lowercase();

    let txn = db.begin().await?;

    if let Some(user) = repositories::users::find_by_email_ci(&txn, &email).await?
        && repositories::memberships::find(&txn, user.id, org_id)
            .await?
            .is_some()
    {
        return Err(AppError::Conflict(
            "cet utilisateur est déjà membre de l'organisation".into(),
        ));
    }

    repositories::invitations::delete_pending(&txn, org_id, &email).await?;

    // Le jeton en clair part au client ; seul son hash est stocké.
    let token = refresh::generate_opaque_token();
    let invitation = repositories::invitations::create(
        &txn,
        NewInvitation {
            org_id,
            email,
            role,
            token_hash: refresh::hash_opaque_token(&token),
            invited_by: inviter_id,
            expires_at: Utc::now() + Duration::days(INVITATION_TTL_DAYS),
        },
    )
    .await?;

    txn.commit().await?;

    Ok(InvitationCreatedResponse {
        id: invitation.id,
        email: invitation.email,
        role,
        expires_at: invitation.expires_at,
        token,
    })
}

/// L'utilisateur connecté accepte une invitation. Il doit être connecté avec
/// l'email invité : un lien qui fuite ne sert à personne d'autre.
pub async fn accept(
    db: &DatabaseConnection,
    user_id: Uuid,
    token: String,
) -> Result<MyOrgResponse, AppError> {
    let hash = refresh::hash_opaque_token(&token);

    let txn = db.begin().await?;

    // Inconnue, déjà utilisée ou expirée : même réponse.
    let invitation = repositories::invitations::find_by_hash(&txn, &hash)
        .await?
        .filter(|i| i.accepted_at.is_none() && i.expires_at.with_timezone(&Utc) > Utc::now())
        .ok_or(AppError::NotFound)?;

    let user = repositories::users::find_by_id(&txn, user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    if user.email.to_lowercase() != invitation.email {
        return Err(AppError::Forbidden);
    }

    if repositories::memberships::find(&txn, user_id, invitation.org_id)
        .await?
        .is_some()
    {
        return Err(AppError::Conflict(
            "vous êtes déjà membre de l'organisation".into(),
        ));
    }

    let role: Role = invitation.role.parse().map_err(|_| AppError::Internal)?;
    let org_id = invitation.org_id;
    repositories::memberships::create(&txn, user_id, org_id, role).await?;
    repositories::invitations::mark_accepted(&txn, invitation).await?;
    let org = repositories::organizations::find_by_id(&txn, org_id)
        .await?
        .ok_or(AppError::NotFound)?;

    txn.commit().await?;
    Ok(MyOrgResponse::new(org, role))
}
