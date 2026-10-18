use sea_orm::prelude::DateTimeWithTimeZone;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

/// Corps optionnel : sans titre, « Nouvelle conversation ».
#[derive(Debug, Default, Deserialize, Validate)]
pub struct CreateConversationRequest {
    #[validate(length(min = 1, max = 200, message = "titre : 1 à 200 caractères"))]
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConversationResponse {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTimeWithTimeZone,
}

impl From<lodestone_entity::conversations::Model> for ConversationResponse {
    fn from(c: lodestone_entity::conversations::Model) -> Self {
        Self {
            id: c.id,
            title: c.title,
            created_at: c.created_at,
        }
    }
}

/// Une conversation de l'historique.
#[derive(Debug, Serialize)]
pub struct ConversationSummaryResponse {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTimeWithTimeZone,
    /// Dernière activité (dernier message, ou création).
    pub updated_at: DateTimeWithTimeZone,
}

/// Une conversation rouverte : ses messages et leurs sources.
#[derive(Debug, Serialize)]
pub struct ConversationDetailResponse {
    pub id: Uuid,
    pub title: String,
    pub created_at: DateTimeWithTimeZone,
    pub messages: Vec<MessageResponse>,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub id: Uuid,
    /// `user` | `assistant`
    pub role: String,
    pub content: String,
    pub created_at: DateTimeWithTimeZone,
    /// Documents cités (réponses de l'assistant), meilleur score d'abord.
    pub sources: Vec<SourceResponse>,
}

/// Même forme que les citations envoyées en direct (`sources` sur la WebSocket).
#[derive(Debug, Serialize)]
pub struct SourceResponse {
    pub document_id: Uuid,
    pub title: String,
    pub score: f32,
}
