use async_trait::async_trait;
use uuid::Uuid;

use crate::error::BillingError;

/// Demande de session de paiement pour passer une org en Pro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckoutRequest {
    pub org_id: Uuid,
    /// Prix choisi (`price_…`) : mensuel ou annuel.
    pub price_id: String,
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

/// Un palier d'un prix « par paliers » (montants en centimes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceTier {
    /// Dernière quantité couverte ; `None` = sans limite (dernier palier).
    pub up_to: Option<u64>,
    pub flat_amount: Option<u64>,
    pub unit_amount: Option<u64>,
}

/// Description du prix Pro tel que configuré chez le fournisseur.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PriceInfo {
    pub currency: String,
    /// `month`, `year`…
    pub interval: Option<String>,
    /// Prix par paliers en mode « volume » (tout le volume au tarif du palier atteint).
    pub tiered_volume: bool,
    pub tiers: Vec<PriceTier>,
}

/// Fournisseur de paiement (Stripe en vrai, mock en test). Les identifiants de
/// prix viennent de la configuration de la plateforme, à chaque appel.
#[async_trait]
pub trait PaymentProvider: Send + Sync {
    async fn create_checkout(
        &self,
        request: &CheckoutRequest,
    ) -> Result<CheckoutSession, BillingError>;

    /// Met à jour le nombre de sièges d'un abonnement (avec prorata).
    async fn update_seats(
        &self,
        subscription_item_id: &str,
        seats: u32,
    ) -> Result<(), BillingError>;

    /// Change le prix d'un abonnement TOUT DE SUITE (mensuel -> annuel) :
    /// la période en cours est créditée au prorata, la nouvelle facturée aussitôt.
    async fn change_price_now(
        &self,
        subscription_item_id: &str,
        price_id: &str,
        seats: u32,
    ) -> Result<(), BillingError>;

    /// Programme le passage à `to_price_id` À LA FIN de la période en cours
    /// (annuel -> mensuel), sans prorata ni remboursement. Renvoie la date de
    /// bascule (horodatage Unix).
    async fn schedule_price_change(
        &self,
        subscription_id: &str,
        from_price_id: &str,
        to_price_id: &str,
        seats: u32,
    ) -> Result<i64, BillingError>;

    /// Un prix configuré chez le fournisseur (vérification au démarrage).
    async fn price(&self, price_id: &str) -> Result<PriceInfo, BillingError>;
}
