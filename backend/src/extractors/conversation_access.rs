use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use lodestone_entity::conversations;

use crate::{
    error::AppError,
    extractors::{
        auth_user::AuthUser,
        org_member::{path_id, require_role},
    },
    repositories,
    state::AppState,
};
use lodestone_core::role::Role;

/// Pour les routes `/api/conversations/{id}` : une conversation est privée.
/// Il faut en être l'auteur ET être encore membre de son org.
///
/// - pas de token / token invalide -> 401
/// - conversation inexistante, d'un autre utilisateur (même l'owner de l'org),
///   ou dans une org quittée -> 404 (on ne révèle rien)
pub struct ConversationAccess {
    pub user_id: Uuid,
    pub conversation: conversations::Model,
}

impl FromRequestParts<AppState> for ConversationAccess {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let conversation_id = path_id(parts, state).await?;

        let conversation = repositories::conversations::find_by_id(&state.db, conversation_id)
            .await?
            .filter(|c| c.user_id == user.user_id)
            .ok_or(AppError::NotFound)?;
        // N'importe quel rôle suffit ; pas membre -> 404 (cf. require_role).
        require_role(state, user.user_id, conversation.org_id, Role::Member).await?;

        Ok(ConversationAccess {
            user_id: user.user_id,
            conversation,
        })
    }
}
