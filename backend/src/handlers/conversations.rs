use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::conversations::CreateConversationRequest,
    error::AppError,
    extractors::{conversation_access::ConversationAccess, org_member::OrgMember},
    services,
    state::AppState,
};

/// POST /api/orgs/{id}/conversations — membre. Corps optionnel : `{"title": "..."}`.
pub async fn create(
    State(state): State<AppState>,
    member: OrgMember,
    body: Option<Json<CreateConversationRequest>>,
) -> Result<impl IntoResponse, AppError> {
    let Json(body) = body.unwrap_or_default();
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let conversation =
        services::conversations::create(&state.db, member.org_id, member.user_id, body.title)
            .await?;
    Ok((StatusCode::CREATED, Json(conversation)))
}

/// GET /api/orgs/{id}/conversations — membre : SES conversations dans l'org.
pub async fn list(
    State(state): State<AppState>,
    member: OrgMember,
) -> Result<impl IntoResponse, AppError> {
    let conversations =
        services::conversations::list(&state.db, member.org_id, member.user_id).await?;
    Ok(Json(conversations))
}

/// GET /api/conversations/{id} — auteur : messages + sources citées.
pub async fn get(
    State(state): State<AppState>,
    access: ConversationAccess,
) -> Result<impl IntoResponse, AppError> {
    let detail = services::conversations::detail(&state.db, access.conversation).await?;
    Ok(Json(detail))
}

/// DELETE /api/conversations/{id} — auteur : supprime messages et sources avec.
pub async fn delete(
    State(state): State<AppState>,
    access: ConversationAccess,
) -> Result<impl IntoResponse, AppError> {
    services::conversations::delete(&state.db, access.conversation.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
