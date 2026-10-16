use sea_orm::DatabaseConnection;
use std::sync::Arc;

use lodestone_llm::LlmClient;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Arc<Config>,
    /// Ollama / OpenAI en vrai, `MockLlmClient` dans les tests.
    pub llm: Arc<dyn LlmClient>,
}
