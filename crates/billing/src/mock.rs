use std::sync::Mutex;

use async_trait::async_trait;

use crate::{
    error::BillingError,
    provider::{CheckoutRequest, CheckoutSession, PaymentProvider},
};

/// Fournisseur de test : pas de réseau, garde les demandes reçues.
#[derive(Default)]
pub struct MockPaymentProvider {
    requests: Mutex<Vec<CheckoutRequest>>,
    fail: bool,
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
}
