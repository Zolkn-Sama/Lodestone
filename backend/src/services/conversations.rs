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

/// Longueur maximale d'un titre déduit de la première question.
const AUTO_TITLE_MAX_CHARS: usize = 60;

/// Titre lisible tiré d'une question : espaces normalisés, coupé à
/// `AUTO_TITLE_MAX_CHARS` sur une fin de mot, avec « … » si tronqué.
pub fn title_from_question(question: &str) -> String {
    let words: Vec<&str> = question.split_whitespace().collect();
    let full = words.join(" ");
    if full.chars().count() <= AUTO_TITLE_MAX_CHARS {
        return full;
    }

    let mut title = String::new();
    for word in &words {
        let extra = usize::from(!title.is_empty()) + word.chars().count();
        if title.chars().count() + extra > AUTO_TITLE_MAX_CHARS {
            break;
        }
        if !title.is_empty() {
            title.push(' ');
        }
        title.push_str(word);
    }
    if title.is_empty() {
        // Un seul « mot » géant : coupe brute.
        title = full.chars().take(AUTO_TITLE_MAX_CHARS).collect();
    }
    title.push('…');
    title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn question_courte_gardee_telle_quelle() {
        assert_eq!(
            title_from_question("  Combien de jours\nde congés ?  "),
            "Combien de jours de congés ?"
        );
    }

    #[test]
    fn question_longue_coupee_sur_un_mot() {
        let question = "Quelle est la procédure exacte pour demander des congés payés pendant la période estivale ?";
        let title = title_from_question(question);
        assert!(title.ends_with('…'), "{title}");
        assert!(title.chars().count() <= AUTO_TITLE_MAX_CHARS + 1);
        let without_ellipsis = title.trim_end_matches('…');
        assert!(
            question.starts_with(without_ellipsis),
            "coupé sur une fin de mot : {title}"
        );
        assert!(question[without_ellipsis.len()..].starts_with(' '));
    }

    #[test]
    fn mot_geant() {
        let title = title_from_question(&"é".repeat(200));
        assert_eq!(title.chars().count(), AUTO_TITLE_MAX_CHARS + 1);
        assert!(title.ends_with('…'));
    }
}
