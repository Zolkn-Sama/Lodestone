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
