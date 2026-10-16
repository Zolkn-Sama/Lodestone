//! lodestone-llm : accès aux modèles (embeddings, plus tard génération) derrière
//! le trait `LlmClient`, pour basculer Ollama / OpenAI / mock sans toucher au reste.

pub mod client;
pub mod error;
pub mod mock;
pub mod openai_compatible;

pub use client::LlmClient;
pub use error::LlmError;
pub use mock::MockLlmClient;
pub use openai_compatible::OpenAiCompatibleClient;
