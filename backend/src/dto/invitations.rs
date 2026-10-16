use sea_orm::prelude::DateTimeWithTimeZone;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

use lodestone_core::role::Role;

fn default_role() -> Role {
    Role::Member
}

#[derive(Debug, Deserialize, Validate)]
pub struct CreateInvitationRequest {
    #[validate(email(message = "email invalide"))]
    pub email: String,
    /// `member` si absent.
    #[serde(default = "default_role")]
    pub role: Role,
}

/// Renvoyé UNE seule fois, à la création : seul le hash du jeton est stocké.
/// En attendant l'envoi d'e-mails, c'est à l'admin de transmettre le lien.
#[derive(Debug, Serialize)]
pub struct InvitationCreatedResponse {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
    pub expires_at: DateTimeWithTimeZone,
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct AcceptInvitationRequest {
    pub token: String,
}
