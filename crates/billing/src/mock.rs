use std::{collections::HashMap, sync::Mutex};

use async_trait::async_trait;

use crate::{
    error::BillingError,
    provider::{CheckoutRequest, CheckoutSession, PaymentProvider, PriceInfo, PriceTier},
};

/// Date de bascule renvoyée par `schedule_price_change` dans le mock.
pub const MOCK_PERIOD_END: i64 = 1_900_000_000;

/// Changement de prix reçu par le mock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PriceChange {
    Now {
        item_id: String,
        price_id: String,
        seats: u32,
    },
    Scheduled {
        subscription_id: String,
        from_price_id: String,
        to_price_id: String,
        seats: u32,
    },
}

/// Fournisseur de test : pas de réseau, garde les demandes reçues.
#[derive(Default)]
pub struct MockPaymentProvider {
    requests: Mutex<Vec<CheckoutRequest>>,
    seat_updates: Mutex<Vec<(String, u32)>>,
    price_changes: Mutex<Vec<PriceChange>>,
    /// Prix renvoyés par `price` ; un id absent donne une erreur 404.
    prices: HashMap<String, PriceInfo>,
    fail: bool,
}

fn failing_error() -> BillingError {
    BillingError::Transport("MockPaymentProvider::failing".into())
}

fn poisoned() -> BillingError {
    BillingError::Transport("verrou empoisonné".into())
}

impl MockPaymentProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// Variante dont chaque appel échoue (Stripe indisponible).
    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    /// Déclare le prix `price_id` (pour `price`).
    pub fn with_price(mut self, price_id: impl Into<String>, price: PriceInfo) -> Self {
        self.prices.insert(price_id.into(), price);
        self
    }

    pub fn checkout_requests(&self) -> Vec<CheckoutRequest> {
        self.requests.lock().map(|r| r.clone()).unwrap_or_default()
    }

    /// Les mises à jour de sièges reçues : (ligne d'abonnement, sièges).
    pub fn seat_updates(&self) -> Vec<(String, u32)> {
        self.seat_updates
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    pub fn price_changes(&self) -> Vec<PriceChange> {
        self.price_changes
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    /// Prix par paliers en volume : forfait `floor` jusqu'à `up_to` sièges,
    /// puis `seat` par siège, facturé tous les `interval` (`month` / `year`).
    pub fn tiered_price(interval: &str, up_to: u64, floor: u64, seat: u64) -> PriceInfo {
        PriceInfo {
            currency: "eur".into(),
            interval: Some(interval.into()),
            tiered_volume: true,
            tiers: vec![
                PriceTier {
                    up_to: Some(up_to),
                    flat_amount: Some(floor),
                    unit_amount: Some(0),
                },
                PriceTier {
                    up_to: None,
                    flat_amount: None,
                    unit_amount: Some(seat),
                },
            ],
        }
    }

    fn check(&self) -> Result<(), BillingError> {
        if self.fail {
            Err(failing_error())
        } else {
            Ok(())
        }
    }

    fn record_change(&self, change: PriceChange) -> Result<(), BillingError> {
        self.price_changes
            .lock()
            .map_err(|_| poisoned())?
            .push(change);
        Ok(())
    }
}

#[async_trait]
impl PaymentProvider for MockPaymentProvider {
    async fn create_checkout(
        &self,
        request: &CheckoutRequest,
    ) -> Result<CheckoutSession, BillingError> {
        self.check()?;
        let mut requests = self.requests.lock().map_err(|_| poisoned())?;
        requests.push(request.clone());
        let id = format!("cs_mock_{}", requests.len());
        Ok(CheckoutSession {
            url: format!("https://checkout.mock/{id}"),
            id,
        })
    }

    async fn update_seats(
        &self,
        subscription_item_id: &str,
        seats: u32,
    ) -> Result<(), BillingError> {
        self.check()?;
        self.seat_updates
            .lock()
            .map_err(|_| poisoned())?
            .push((subscription_item_id.to_string(), seats));
        Ok(())
    }

    async fn change_price_now(
        &self,
        subscription_item_id: &str,
        price_id: &str,
        seats: u32,
    ) -> Result<(), BillingError> {
        self.check()?;
        self.record_change(PriceChange::Now {
            item_id: subscription_item_id.into(),
            price_id: price_id.into(),
            seats,
        })
    }

    async fn schedule_price_change(
        &self,
        subscription_id: &str,
        from_price_id: &str,
        to_price_id: &str,
        seats: u32,
    ) -> Result<i64, BillingError> {
        self.check()?;
        self.record_change(PriceChange::Scheduled {
            subscription_id: subscription_id.into(),
            from_price_id: from_price_id.into(),
            to_price_id: to_price_id.into(),
            seats,
        })?;
        Ok(MOCK_PERIOD_END)
    }

    async fn price(&self, price_id: &str) -> Result<PriceInfo, BillingError> {
        self.check()?;
        self.prices
            .get(price_id)
            .cloned()
            .ok_or_else(|| BillingError::Api {
                status: 404,
                body: format!("No such price: '{price_id}'"),
            })
    }
}
