use async_trait::async_trait;
use futures::stream::BoxStream;
use serde::Serialize;

use crate::error::LlmError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ChatRole::User,
            content: content.into(),
        }
    }
}

/// Morceaux de réponse, dans l'ordre. Une erreur termine le flux.
pub type ChatStream = BoxStream<'static, Result<String, LlmError>>;

/// Abstraction du fournisseur de modèles (Ollama, OpenAI, mock de test).
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Génère une réponse en streaming. L'erreur immédiate (serveur
    /// injoignable, modèle inconnu…) est renvoyée avant tout morceau.
    async fn chat_stream(&self, messages: &[ChatMessage]) -> Result<ChatStream, LlmError>;

    /// Un vecteur par texte, dans le même ordre que `texts`, sans transformation.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError>;

    /// Taille des vecteurs produits par `embed` (768 pour `nomic-embed-text`).
    fn embedding_dimensions(&self) -> usize;

    /// Préfixe ajouté aux fragments de documents avant l'embedding. Certains
    /// modèles en ont besoin pour distinguer document et question
    /// (`search_document: ` pour nomic-embed-text).
    fn document_prefix(&self) -> &str {
        ""
    }

    /// Préfixe ajouté aux questions (`search_query: ` pour nomic-embed-text).
    fn query_prefix(&self) -> &str {
        ""
    }

    /// Embeddings de fragments de documents (avec `document_prefix`).
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        let prefix = self.document_prefix();
        if prefix.is_empty() {
            return self.embed(texts).await;
        }
        let prefixed: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
        self.embed(&prefixed).await
    }

    /// Embedding d'une question (avec `query_prefix`).
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, LlmError> {
        let mut vectors = self
            .embed(&[format!("{}{text}", self.query_prefix())])
            .await?;
        vectors
            .pop()
            .ok_or_else(|| LlmError::InvalidResponse("aucun vecteur pour la question".into()))
    }
}
