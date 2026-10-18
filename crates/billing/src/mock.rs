use std::sync::Mutex;

use async_trait::async_trait;

use crate::{
    error::BillingError,
    provider::{CheckoutRequest, CheckoutSession, PaymentProvider, PriceInfo, PriceTier},
};

/// Fournisseur de test : pas de réseau, garde les demandes reçues.
pub struct MockPaymentProvider {
    requests: Mutex<Vec<CheckoutRequest>>,
    seat_updates: Mutex<Vec<(String, u32)>>,
    price: PriceInfo,
    fail: bool,
}

impl Default for MockPaymentProvider {
    fn default() -> Self {
        Self {
            requests: Mutex::default(),
            seat_updates: Mutex::default(),
            price: Self::tiered_price(5, 14900, 2900),
            fail: false,
        }
    }
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

    pub fn with_price(mut self, price: PriceInfo) -> Self {
        self.price = price;
        self
    }

    /// Prix mensuel par paliers en volume : forfait `floor` jusqu'à
    /// `up_to` sièges, puis `seat` par siège.
    pub fn tiered_price(up_to: u64, floor: u64, seat: u64) -> PriceInfo {
        PriceInfo {
            currency: "eur".into(),
            interval: Some("month".into()),
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
}

#[async_trait]
impl PaymentProvider for MockPaymentProvider {
    async fn create_checkout(
        &self,
        request: &CheckoutRequest,
    ) -> Result<CheckoutSession, BillingError> {
        if self.fail {
            return Err(BillingError::Transport(
                "MockPaymentProvider::failing".into(),
            ));
        }
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| BillingError::Transport("verrou empoisonné".into()))?;
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
        if self.fail {
            return Err(BillingError::Transport(
                "MockPaymentProvider::failing".into(),
            ));
        }
        self.seat_updates
            .lock()
            .map_err(|_| BillingError::Transport("verrou empoisonné".into()))?
            .push((subscription_item_id.to_string(), seats));
        Ok(())
    }

    async fn pro_price(&self) -> Result<PriceInfo, BillingError> {
        if self.fail {
            return Err(BillingError::Transport(
                "MockPaymentProvider::failing".into(),
            ));
        }
        Ok(self.price.clone())
    }
}
