use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

use lodestone_auth::jwt::decode_access_token;

/// Utilisateur authentifié, extrait du JWT. Ne porte que l'id (les claims),
/// jamais tout le Model.
pub struct AuthUser {
    pub user_id: Uuid,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // "Authorization: Bearer <token>"
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;

        let claims = decode_access_token(token, &state.config.jwt_secret)
            .map_err(|_| AppError::Unauthorized)?;

        Ok(AuthUser { user_id: claims.sub })
    }
}