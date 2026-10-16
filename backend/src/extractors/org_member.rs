use std::{collections::HashMap, marker::PhantomData};

use axum::extract::{FromRequestParts, Path};
use axum::http::request::Parts;
use uuid::Uuid;

use lodestone_core::role::Role;

use crate::{error::AppError, extractors::auth_user::AuthUser, repositories, state::AppState};

/// Niveau minimal exigé par une route. Implémenté par les marqueurs
/// `Member`, `Admin` et `Owner` ci-dessous.
pub trait RequiredRole {
    const ROLE: Role;
}

/// N'importe quel membre de l'org.
pub struct Member;
/// Admin ou owner.
pub struct Admin;
/// Owner uniquement.
pub struct Owner;

impl RequiredRole for Member {
    const ROLE: Role = Role::Member;
}
impl RequiredRole for Admin {
    const ROLE: Role = Role::Admin;
}
impl RequiredRole for Owner {
    const ROLE: Role = Role::Owner;
}

/// Utilisateur authentifié ET membre de l'org ciblée par le segment `{id}`
/// de l'URL, avec au moins le rôle `R`. Toute l'autorisation « org » passe
/// par ici : un handler qui reçoit un `OrgMember<Admin>` sait déjà que
/// l'appelant a le droit d'agir.
///
/// - pas de token / token invalide -> 401
/// - pas membre de l'org (ou org inexistante) -> 404 (on ne révèle pas son existence)
/// - membre mais rôle insuffisant -> 403
pub struct OrgMember<R: RequiredRole = Member> {
    pub user_id: Uuid,
    pub org_id: Uuid,
    pub role: Role,
    _required: PhantomData<R>,
}

impl<R: RequiredRole + Send + Sync> FromRequestParts<AppState> for OrgMember<R> {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        let org_id = path_id(parts, state).await?;
        let role = require_role(state, user.user_id, org_id, R::ROLE).await?;

        Ok(OrgMember {
            user_id: user.user_id,
            org_id,
            role,
            _required: PhantomData,
        })
    }
}

/// Le segment `{id}` de l'URL, en UUID. Absent ou invalide -> 404.
/// HashMap : marche quelle que soit la forme de la route
/// (`/api/orgs/{id}` comme `/api/orgs/{id}/members/{user_id}`).
pub(crate) async fn path_id(parts: &mut Parts, state: &AppState) -> Result<Uuid, AppError> {
    let Path(params) = Path::<HashMap<String, String>>::from_request_parts(parts, state)
        .await
        .map_err(|_| AppError::NotFound)?;
    params
        .get("id")
        .and_then(|s| s.parse::<Uuid>().ok())
        .ok_or(AppError::NotFound)
}

/// Rôle de l'utilisateur dans l'org, s'il atteint `required`.
/// Pas membre -> 404 (on ne révèle pas l'existence), rôle insuffisant -> 403.
pub(crate) async fn require_role(
    state: &AppState,
    user_id: Uuid,
    org_id: Uuid,
    required: Role,
) -> Result<Role, AppError> {
    let membership = repositories::memberships::find(&state.db, user_id, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let role: Role = membership.role.parse().map_err(|e| {
        tracing::error!(error = %e, membership_id = %membership.id, "rôle invalide en base");
        AppError::Internal
    })?;

    if !role.at_least(required) {
        return Err(AppError::Forbidden);
    }
    Ok(role)
}
