use thiserror::Error;

#[derive(Debug, Error)]
pub enum BillingError {
    #[error("appel au fournisseur de paiement impossible : {0}")]
    Transport(String),

    #[error("le fournisseur de paiement a répondu {status} : {body}")]
    Api { status: u16, body: String },

    #[error("réponse du fournisseur de paiement invalide : {0}")]
    InvalidResponse(String),
}

/// Rejet d'un webhook : on répond 400 et rien n'est appliqué.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum WebhookError {
    #[error("en-tête Stripe-Signature manquant ou mal formé")]
    MalformedHeader,

    #[error("horodatage hors tolérance (rejeu possible)")]
    TimestampOutOfTolerance,

    #[error("signature invalide")]
    InvalidSignature,

    #[error("événement illisible : {0}")]
    InvalidPayload(String),
}
