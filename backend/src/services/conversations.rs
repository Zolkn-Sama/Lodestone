use sea_orm::DatabaseConnection;
use uuid::Uuid;

use std::collections::HashMap;

use crate::{
    dto::conversations::{
        ConversationDetailResponse, ConversationResponse, ConversationSummaryResponse,
        MessageResponse, SourceResponse,
    },
    error::AppError,
    repositories,
};
use lodestone_entity::conversations;

pub const DEFAULT_TITLE: &str = "Nouvelle conversation";

/// Crée une conversation de `user_id` dans `org_id`. Elle lui reste privée :
/// les autres membres de l'org ne la voient pas.
pub async fn create(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    title: Option<String>,
) -> Result<ConversationResponse, AppError> {
    let title = match title {
        Some(t) => {
            let t = t.trim().to_string();
            if t.is_empty() {
                return Err(AppError::InvalidInput("titre : 1 à 200 caractères".into()));
            }
            t
        }
        None => DEFAULT_TITLE.to_string(),
    };

    let conversation = repositories::conversations::create(db, org_id, user_id, title).await?;
    Ok(conversation.into())
}

/// Historique : les conversations de l'utilisateur dans l'org.
pub async fn list(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ConversationSummaryResponse>, AppError> {
    let rows = repositories::conversations::list_for_user(db, org_id, user_id).await?;
    Ok(rows
        .into_iter()
        .map(|c| ConversationSummaryResponse {
            id: c.id,
            title: c.title,
            created_at: c.created_at,
            updated_at: c.updated_at,
        })
        .collect())
}

/// Messages de la conversation, chacun avec ses sources.
pub async fn detail(
    db: &DatabaseConnection,
    conversation: conversations::Model,
) -> Result<ConversationDetailResponse, AppError> {
    let messages = repositories::messages::list_for_conversation(db, conversation.id).await?;

    let mut sources: HashMap<Uuid, Vec<SourceResponse>> = HashMap::new();
    for s in repositories::messages::sources_for_conversation(db, conversation.id).await? {
        sources
            .entry(s.message_id)
            .or_default()
            .push(SourceResponse {
                document_id: s.document_id,
                title: s.title,
                score: s.score,
            });
    }

    Ok(ConversationDetailResponse {
        id: conversation.id,
        title: conversation.title,
        created_at: conversation.created_at,
        messages: messages
            .into_iter()
            .map(|m| MessageResponse {
                sources: sources.remove(&m.id).unwrap_or_default(),
                id: m.id,
                role: m.role,
                content: m.content,
                created_at: m.created_at,
            })
            .collect(),
    })
}

pub async fn delete(db: &DatabaseConnection, conversation_id: Uuid) -> Result<(), AppError> {
    if repositories::conversations::delete(db, conversation_id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}
