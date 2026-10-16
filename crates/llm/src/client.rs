use async_trait::async_trait;

use crate::error::LlmError;

/// Abstraction du fournisseur de modèles (Ollama, OpenAI, mock de test).
/// `chat_stream` arrivera avec le chat RAG (RAG-03).
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Un vecteur par texte, dans le même ordre que `texts`.
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError>;

    /// Taille des vecteurs produits par `embed` (768 pour `nomic-embed-text`).
    fn embedding_dimensions(&self) -> usize;
}
