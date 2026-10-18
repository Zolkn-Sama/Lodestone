use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::conversations::CreateConversationRequest, error::AppError,
    extractors::org_member::OrgMember, services, state::AppState,
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
