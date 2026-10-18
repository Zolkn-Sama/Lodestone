use sea_orm::DatabaseConnection;
use std::sync::Arc;

use lodestone_billing::PaymentProvider;
use lodestone_llm::LlmClient;

use crate::{config::Config, events::Events, mail::Mailer, rate_limit::Limits};

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseConnection,
    pub config: Arc<Config>,
    /// Ollama / OpenAI en vrai, `MockLlmClient` dans les tests.
    pub llm: Arc<dyn LlmClient>,
    /// Notifications temps réel (statuts de documents…) vers les WebSocket.
    pub events: Events,
    /// Stripe en vrai, mock en test ; `None` si la facturation n'est pas configurée.
    pub billing: Option<Arc<dyn PaymentProvider>>,
    /// SMTP en vrai, désactivé sans config, mock en test.
    pub mailer: Arc<dyn Mailer>,
    /// Limitation des tentatives (connexion, inscription, mot de passe oublié).
    pub limits: Arc<Limits>,
}
