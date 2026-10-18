//! Chat RAG : retrouver les fragments pertinents pour une question.
//! La génération (RAG-03) et la persistance (RAG-04) viendront s'y ajouter.

use std::collections::HashMap;

use sea_orm::{DatabaseConnection, DbErr};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    repositories::{self, chunks::RetrievedChunk},
    ws::protocol::Citation,
};
use lodestone_entity::conversations;
use lodestone_llm::{LlmClient, LlmError};

/// Fragments retenus par question.
pub const TOP_K: u64 = 5;

/// Longueur maximale d'une question (caractères).
pub const MAX_QUESTION_CHARS: usize = 4000;

#[derive(Debug, Error)]
pub enum ChatError {
    /// Inexistante, d'un autre utilisateur, ou dans une org quittée : même
    /// réponse, on ne révèle rien.
    #[error("conversation introuvable")]
    ConversationNotFound,

    #[error("la question est vide")]
    EmptyQuestion,

    #[error("question trop longue ({MAX_QUESTION_CHARS} caractères maximum)")]
    QuestionTooLong,

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Database(#[from] DbErr),
}

impl ChatError {
    /// Message montré au client : les détails techniques restent dans les logs.
    pub fn user_message(&self) -> String {
        match self {
            ChatError::Llm(_) => "le service de recherche est momentanément indisponible".into(),
            ChatError::Database(_) => "erreur interne".into(),
            other => other.to_string(),
        }
    }
}

pub struct Retrieval {
    pub conversation: conversations::Model,
    /// Du plus pertinent au moins pertinent.
    pub chunks: Vec<RetrievedChunk>,
}

/// Vérifie l'accès à la conversation, puis cherche les `TOP_K` fragments les
/// plus proches de la question **dans l'org de la conversation uniquement**.
pub async fn retrieve(
    db: &DatabaseConnection,
    llm: &dyn LlmClient,
    user_id: Uuid,
    conversation_id: Uuid,
    question: &str,
) -> Result<Retrieval, ChatError> {
    let question = question.trim();
    if question.is_empty() {
        return Err(ChatError::EmptyQuestion);
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        return Err(ChatError::QuestionTooLong);
    }

    // Une conversation est privée à son auteur…
    let conversation = repositories::conversations::find_by_id(db, conversation_id)
        .await?
        .filter(|c| c.user_id == user_id)
        .ok_or(ChatError::ConversationNotFound)?;
    // … et n'est utilisable que tant qu'il est membre de l'org.
    repositories::memberships::find(db, user_id, conversation.org_id)
        .await?
        .ok_or(ChatError::ConversationNotFound)?;

    let embedding = llm.embed_query(question).await?;
    let chunks = repositories::chunks::search(db, conversation.org_id, embedding, TOP_K).await?;

    Ok(Retrieval {
        conversation,
        chunks,
    })
}

/// Une citation par document (meilleur score de ses fragments), triées par
/// score décroissant.
pub fn citations(chunks: &[RetrievedChunk]) -> Vec<Citation> {
    let mut best: HashMap<Uuid, Citation> = HashMap::new();
    for chunk in chunks {
        best.entry(chunk.document_id)
            .and_modify(|c| c.score = c.score.max(chunk.score))
            .or_insert_with(|| Citation {
                document_id: chunk.document_id,
                title: chunk.title.clone(),
                score: chunk.score,
            });
    }
    let mut citations: Vec<Citation> = best.into_values().collect();
    citations.sort_by(|a, b| b.score.total_cmp(&a.score));
    citations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(document: u128, title: &str, score: f32) -> RetrievedChunk {
        RetrievedChunk {
            chunk_id: Uuid::new_v4(),
            document_id: Uuid::from_u128(document),
            title: title.into(),
            chunk_index: 0,
            content: String::new(),
            score,
        }
    }

    #[test]
    fn une_citation_par_document_triees_par_score() {
        let chunks = [
            chunk(1, "RH.pdf", 0.71),
            chunk(2, "Frais.md", 0.80),
            chunk(1, "RH.pdf", 0.92),
            chunk(3, "IT.txt", 0.40),
            chunk(2, "Frais.md", 0.10),
        ];
        let got: Vec<(String, f32)> = citations(&chunks)
            .into_iter()
            .map(|c| (c.title, c.score))
            .collect();
        assert_eq!(
            got,
            vec![
                ("RH.pdf".into(), 0.92),
                ("Frais.md".into(), 0.80),
                ("IT.txt".into(), 0.40)
            ]
        );
    }

    #[test]
    fn aucun_fragment_aucune_citation() {
        assert!(citations(&[]).is_empty());
    }

    #[test]
    fn messages_utilisateur_sans_details_techniques() {
        let llm = ChatError::Llm(LlmError::Transport(
            "connexion refusée 127.0.0.1:11434".into(),
        ));
        assert!(!llm.user_message().contains("127.0.0.1"));
        assert_eq!(
            ChatError::ConversationNotFound.user_message(),
            "conversation introuvable"
        );
    }
}
