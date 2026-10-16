use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use uuid::Uuid;

use lodestone_core::role::Role;
use lodestone_entity::documents;

use crate::{
    error::AppError,
    extractors::{
        auth_user::AuthUser,
        org_member::{Member, RequiredRole, path_id, require_role},
    },
    repositories,
    state::AppState,
};

/// Pour les routes `/api/documents/{id}` : charge le document, puis exige que
/// l'appelant soit membre de l'org du document avec au moins le rôle `R`.
///
/// - pas de token / token invalide -> 401
/// - document inexistant, ou dans une org dont on n'est pas membre -> 404
///   (même réponse : on ne révèle pas qu'un document existe ailleurs)
/// - membre mais rôle insuffisant -> 403
pub struct DocumentAccess<R: RequiredRole = Member> {
    pub user_id: Uuid,
    pub role: Role,
    pub document: documents::Model,
    _required: PhantomData<R>,
}

impl<R: RequiredRole + Send + Sync> FromRequestParts<AppState> for DocumentAccess<R> {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let document_id = path_id(parts, state).await?;

        let document = repositories::documents::find_by_id(&state.db, document_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let role = require_role(state, user.user_id, document.org_id, R::ROLE).await?;

        Ok(DocumentAccess {
            user_id: user.user_id,
            role,
            document,
            _required: PhantomData,
        })
    }
}
