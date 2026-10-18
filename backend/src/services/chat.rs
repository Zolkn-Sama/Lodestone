//! Chat RAG : retrouver les fragments pertinents pour une question, générer la
//! réponse en streaming, puis enregistrer l'échange et ses sources.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use futures::StreamExt;
use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use thiserror::Error;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::{
    repositories::{
        self,
        chunks::RetrievedChunk,
        messages::{MessageRole, NewMessage},
    },
    ws::protocol::{Citation, ServerMsg},
};
use lodestone_entity::conversations;
use lodestone_llm::{ChatMessage, LlmClient, LlmError};
use lodestone_rag::{PromptSource, build_prompt, prompt::DEFAULT_MAX_CONTEXT_TOKENS};

/// Réponse quand aucun document de l'org ne peut servir : on ne demande rien
/// au modèle, qui n'aurait que son imagination pour répondre.
pub const NO_SOURCE_REPLY: &str = "Je n'ai trouvé aucun document de votre organisation qui permette de répondre à cette question.";

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

/// Traite une question de bout en bout et pousse les messages dans `outbox` :
/// `sources`, puis un `token` par morceau de réponse, puis `done` — envoyé
/// seulement une fois l'échange (question, réponse, sources) enregistré.
/// En cas d'échec, un `error` termine l'échange (sans `done`) et rien n'est
/// enregistré. S'arrête dès que le client n'écoute plus (`outbox` fermé).
pub async fn answer(
    db: &DatabaseConnection,
    llm: &dyn LlmClient,
    user_id: Uuid,
    conversation_id: Uuid,
    question: &str,
    outbox: &mpsc::Sender<ServerMsg>,
) {
    // Client parti -> on arrête tout (et on lâche le flux du modèle).
    macro_rules! send {
        ($msg:expr) => {
            if outbox.send($msg).await.is_err() {
                return;
            }
        };
    }

    let asked_at = Utc::now();
    let retrieval = match retrieve(db, llm, user_id, conversation_id, question).await {
        Ok(retrieval) => retrieval,
        Err(e) => {
            match &e {
                ChatError::Llm(_) | ChatError::Database(_) => {
                    tracing::error!(%user_id, %conversation_id, error = %e, "échec de la recherche");
                }
                _ => tracing::debug!(%user_id, %conversation_id, error = %e, "question refusée"),
            }
            send!(error(e.user_message()));
            return;
        }
    };

    let citations = citations(&retrieval.chunks);
    send!(ServerMsg::Sources {
        citations: citations.clone(),
    });

    // Identifiant du message de l'assistant, annoncé dans `done`.
    let message_id = Uuid::new_v4();
    let exchange = |answer: String| Exchange {
        conversation_id,
        question: question.trim().to_string(),
        asked_at,
        message_id,
        answer,
        sources: retrieval
            .chunks
            .iter()
            .map(|c| (c.chunk_id, c.score))
            .collect(),
    };

    if retrieval.chunks.is_empty() {
        send!(ServerMsg::Token {
            delta: NO_SOURCE_REPLY.to_string(),
        });
        send!(finish(db, exchange(NO_SOURCE_REPLY.to_string())).await);
        return;
    }

    let sources = prompt_sources(&citations, &retrieval.chunks);
    let prompt = build_prompt(question, &sources, DEFAULT_MAX_CONTEXT_TOKENS);
    let messages = [
        ChatMessage::system(prompt.system),
        ChatMessage::user(prompt.user),
    ];

    let mut stream = match llm.chat_stream(&messages).await {
        Ok(stream) => stream,
        Err(e) => {
            tracing::error!(%user_id, %conversation_id, error = %e, "génération impossible");
            send!(error(
                "la génération de la réponse est momentanément indisponible"
            ));
            return;
        }
    };

    let mut answer = String::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(delta) => {
                answer.push_str(&delta);
                send!(ServerMsg::Token { delta });
            }
            Err(e) => {
                tracing::error!(%user_id, %conversation_id, error = %e, "génération interrompue");
                send!(error("la génération de la réponse a été interrompue"));
                return;
            }
        }
    }

    if answer.is_empty() {
        send!(error("le modèle n'a produit aucune réponse"));
    } else {
        send!(finish(db, exchange(answer)).await);
    }
}

/// Un échange question/réponse terminé, prêt à être enregistré.
struct Exchange {
    conversation_id: Uuid,
    question: String,
    asked_at: DateTime<Utc>,
    message_id: Uuid,
    answer: String,
    /// Fragments retenus et leur score.
    sources: Vec<(Uuid, f32)>,
}

/// Enregistre l'échange puis renvoie `done` ; `error` si l'écriture échoue
/// (ex. conversation supprimée pendant la génération).
async fn finish(db: &DatabaseConnection, exchange: Exchange) -> ServerMsg {
    let message_id = exchange.message_id;
    let conversation_id = exchange.conversation_id;
    match save_exchange(db, exchange).await {
        Ok(()) => ServerMsg::Done { message_id },
        Err(e) => {
            tracing::error!(%conversation_id, error = %e, "échange non enregistré");
            error("la réponse n'a pas pu être enregistrée")
        }
    }
}

/// Question + réponse + sources, tout ou rien.
async fn save_exchange(db: &DatabaseConnection, exchange: Exchange) -> Result<(), DbErr> {
    let txn = db.begin().await?;
    repositories::messages::create(
        &txn,
        NewMessage {
            id: Uuid::new_v4(),
            conversation_id: exchange.conversation_id,
            role: MessageRole::User,
            content: exchange.question,
            created_at: exchange.asked_at,
        },
    )
    .await?;
    repositories::messages::create(
        &txn,
        NewMessage {
            id: exchange.message_id,
            conversation_id: exchange.conversation_id,
            role: MessageRole::Assistant,
            content: exchange.answer,
            created_at: Utc::now(),
        },
    )
    .await?;
    repositories::messages::add_sources(&txn, exchange.message_id, &exchange.sources).await?;
    txn.commit().await
}

/// Regroupe les extraits par document, dans l'ordre des citations : le
/// document [1] du prompt est la première citation affichée au client.
fn prompt_sources<'a>(
    citations: &'a [Citation],
    chunks: &'a [RetrievedChunk],
) -> Vec<PromptSource<'a>> {
    citations
        .iter()
        .map(|citation| {
            let mut excerpts: Vec<&RetrievedChunk> = chunks
                .iter()
                .filter(|c| c.document_id == citation.document_id)
                .collect();
            excerpts.sort_by_key(|c| c.chunk_index);
            PromptSource {
                title: &citation.title,
                excerpts: excerpts.into_iter().map(|c| c.content.as_str()).collect(),
            }
        })
        .collect()
}

fn error(message: impl Into<String>) -> ServerMsg {
    ServerMsg::Error {
        message: message.into(),
    }
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
    fn extraits_groupes_dans_l_ordre_des_citations() {
        let mut a1 = chunk(1, "RH.pdf", 0.70);
        a1.chunk_index = 4;
        a1.content = "RH, fragment 4".into();
        let mut a0 = chunk(1, "RH.pdf", 0.60);
        a0.chunk_index = 2;
        a0.content = "RH, fragment 2".into();
        let mut b = chunk(2, "Frais.md", 0.90);
        b.content = "Frais".into();
        let chunks = [a1, b, a0];

        let cites = citations(&chunks);
        let sources = prompt_sources(&cites, &chunks);
        let got: Vec<(&str, Vec<&str>)> = sources
            .iter()
            .map(|s| (s.title, s.excerpts.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                ("Frais.md", vec!["Frais"]),
                ("RH.pdf", vec!["RH, fragment 2", "RH, fragment 4"]),
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
