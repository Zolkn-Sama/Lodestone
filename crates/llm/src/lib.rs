//! lodestone-llm : accès aux modèles (embeddings, génération en streaming) derrière
//! le trait `LlmClient`, pour basculer Ollama / OpenAI / mock sans toucher au reste.

pub mod client;
pub mod error;
pub mod mock;
pub mod openai_compatible;

pub use client::{ChatMessage, ChatRole, ChatStream, LlmClient};
pub use error::LlmError;
pub use mock::{ChatFailure, MockLlmClient};
pub use openai_compatible::OpenAiCompatibleClient;
