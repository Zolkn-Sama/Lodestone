use sea_orm::DatabaseConnection;
use uuid::Uuid;

use crate::{dto::conversations::ConversationResponse, error::AppError, repositories};

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
