use async_trait::async_trait;
use uuid::Uuid;

use crate::error::BillingError;

/// Demande de session de paiement pour passer une org en Pro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRequest {
    pub org_id: Uuid,
    /// Sièges facturés (quantité de l'abonnement).
    pub seats: u32,
    /// Email pré-rempli sur la page de paiement.
    pub customer_email: String,
    pub success_url: String,
    pub cancel_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutSession {
    pub id: String,
    /// Page de paiement hébergée par le fournisseur, où rediriger l'utilisateur.
    pub url: String,
}

/// Fournisseur de paiement (Stripe en vrai, mock en test).
#[async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn create_checkout(
        &self,
        request: &CheckoutRequest,
    ) -> Result<CheckoutSession, BillingError>;
}
