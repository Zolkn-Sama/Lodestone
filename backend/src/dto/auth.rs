use serde::{Deserialize, Serialize};
use validator::Validate;

use sea_orm::prelude::DateTimeWithTimeZone;
use uuid::Uuid;

/// Ce que le client ENVOIE pour s'inscrire.
#[derive(Debug, Deserialize, Validate)]
pub struct RegisterRequest {
    #[validate(email(message = "email invalide"))]
    pub email: String,
    #[validate(length(min = 8, message = "mot de passe : 8 caractères minimum"))]
    pub password: String,
}

/// Ce que le client ENVOIE pour se connecter.
#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

/// Ce qu'on RENVOIE : jamais le hash, jamais le mot de passe.
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String, // toujours "Bearer"
}

#[derive(Debug, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

/// Vue publique d'un utilisateur : ce que le client a le droit de voir.
#[derive(Debug, Serialize)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub created_at: DateTimeWithTimeZone,
}

impl From<lodestone_entity::users::Model> for UserResponse {
    fn from(u: lodestone_entity::users::Model) -> Self {
        Self {
            id: u.id,
            email: u.email,
            created_at: u.created_at,
        }
    }
}

/// Réponse de `/api/auth/me` : le profil (champs à plat) et les orgs.
#[derive(Debug, Serialize)]
pub struct MeResponse {
    #[serde(flatten)]
    pub user: UserResponse,
    pub organizations: Vec<crate::dto::organizations::MyOrgResponse>,
}

#[derive(Debug, Deserialize, Validate)]
pub struct ForgotPasswordRequest {
    #[validate(email(message = "email invalide"))]
    pub email: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct ResetPasswordRequest {
    pub token: String,
    #[validate(length(min = 8, message = "mot de passe : 8 caractères minimum"))]
    pub password: String,
}
