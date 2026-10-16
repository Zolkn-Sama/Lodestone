use thiserror::Error;

#[derive(Debug, Error)]
pub enum LlmError {
    /// Serveur injoignable, timeout, etc.
    #[error("appel au serveur LLM impossible : {0}")]
    Transport(String),

    /// Le serveur a répondu, mais avec un statut d'erreur.
    #[error("le serveur LLM a répondu {status} : {body}")]
    Api { status: u16, body: String },

    /// Réponse illisible ou incohérente (mauvais nombre de vecteurs…).
    #[error("réponse du serveur LLM invalide : {0}")]
    InvalidResponse(String),
}
