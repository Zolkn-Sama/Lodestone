use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    error::BillingError,
    provider::{CheckoutRequest, CheckoutSession, PaymentProvider},
};

pub const STRIPE_API_BASE: &str = "https://api.stripe.com/v1";

/// Client de l'API Stripe (sessions Checkout en mode abonnement).
pub struct StripeClient {
    http: reqwest::Client,
    base_url: String,
    secret_key: String,
    /// Prix Stripe de l'abonnement Pro par siège (`price_…`).
    pro_price_id: String,
}

#[derive(Deserialize)]
struct SessionResponse {
    id: String,
    url: Option<String>,
}

impl StripeClient {
    pub fn new(
        secret_key: impl Into<String>,
        pro_price_id: impl Into<String>,
    ) -> Result<Self, BillingError> {
        Self::with_base_url(STRIPE_API_BASE, secret_key, pro_price_id)
    }

    /// Pour les tests : pointe vers un faux serveur.
    pub fn with_base_url(
        base_url: impl Into<String>,
        secret_key: impl Into<String>,
        pro_price_id: impl Into<String>,
    ) -> Result<Self, BillingError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| BillingError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            secret_key: secret_key.into(),
            pro_price_id: pro_price_id.into(),
        })
    }
}

#[async_trait]
impl PaymentProvider for StripeClient {
    async fn create_checkout(
        &self,
        request: &CheckoutRequest,
    ) -> Result<CheckoutSession, BillingError> {
        let org_id = request.org_id.to_string();
        let seats = request.seats.to_string();
        // L'org est rattachée à la session ET à l'abonnement : les webhooks
        // d'abonnement (mise à jour, résiliation) la retrouvent sans requête.
        let form = [
            ("mode", "subscription"),
            ("line_items[0][price]", self.pro_price_id.as_str()),
            ("line_items[0][quantity]", seats.as_str()),
            ("client_reference_id", org_id.as_str()),
            ("metadata[org_id]", org_id.as_str()),
            ("subscription_data[metadata][org_id]", org_id.as_str()),
            ("customer_email", request.customer_email.as_str()),
            ("success_url", request.success_url.as_str()),
            ("cancel_url", request.cancel_url.as_str()),
        ];

        let res = self
            .http
            .post(format!("{}/checkout/sessions", self.base_url))
            .bearer_auth(&self.secret_key)
            .form(&form)
            .send()
            .await
            .map_err(|e| BillingError::Transport(e.to_string()))?;

        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(BillingError::Api {
                status: status.as_u16(),
                body,
            });
        }
        let session: SessionResponse = res
            .json()
            .await
            .map_err(|e| BillingError::InvalidResponse(e.to_string()))?;
        let url = session
            .url
            .ok_or_else(|| BillingError::InvalidResponse("session sans url".into()))?;
        Ok(CheckoutSession {
            id: session.id,
            url,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Form, Router, extract::State, http::HeaderMap, routing::post};
    use serde_json::json;
    use uuid::Uuid;

    use super::*;

    type Seen = Arc<Mutex<Vec<(HeaderMap, Vec<(String, String)>)>>>;

    async fn fake_stripe(status: u16, body: serde_json::Value) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let app = Router::new()
            .route(
                "/v1/checkout/sessions",
                post(
                    move |State(seen): State<Seen>,
                          headers: HeaderMap,
                          Form(form): Form<Vec<(String, String)>>| async move {
                        if let Ok(mut s) = seen.lock() {
                            s.push((headers, form));
                        }
                        (
                            axum::http::StatusCode::from_u16(status)
                                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
                            axum::Json(body),
                        )
                    },
                ),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}/v1"), seen)
    }

    fn request() -> CheckoutRequest {
        CheckoutRequest {
            org_id: Uuid::nil(),
            seats: 4,
            customer_email: "owner@acme.fr".into(),
            success_url: "https://app.lodestone.fr/billing/ok".into(),
            cancel_url: "https://app.lodestone.fr/billing".into(),
        }
    }

    #[tokio::test]
    async fn cree_une_session_d_abonnement() {
        let (url, seen) = fake_stripe(
            200,
            json!({ "id": "cs_test_123", "url": "https://checkout.stripe.com/c/pay/cs_test_123" }),
        )
        .await;
        let client = StripeClient::with_base_url(url, "sk_test_abc", "price_pro").expect("client");

        let session = client.create_checkout(&request()).await.expect("session");
        assert_eq!(session.id, "cs_test_123");
        assert_eq!(session.url, "https://checkout.stripe.com/c/pay/cs_test_123");

        let seen = seen.lock().expect("lock");
        let (headers, form) = &seen[0];
        assert_eq!(headers["authorization"], "Bearer sk_test_abc");
        let get = |k: &str| {
            form.iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.as_str())
        };
        let org = Uuid::nil().to_string();
        assert_eq!(get("mode"), Some("subscription"));
        assert_eq!(get("line_items[0][price]"), Some("price_pro"));
        assert_eq!(get("line_items[0][quantity]"), Some("4"));
        assert_eq!(get("client_reference_id"), Some(org.as_str()));
        assert_eq!(get("metadata[org_id]"), Some(org.as_str()));
        assert_eq!(
            get("subscription_data[metadata][org_id]"),
            Some(org.as_str())
        );
        assert_eq!(get("customer_email"), Some("owner@acme.fr"));
    }

    #[tokio::test]
    async fn erreur_stripe_remontee() {
        let (url, _) = fake_stripe(
            400,
            json!({ "error": { "message": "No such price: 'price_x'" } }),
        )
        .await;
        let client = StripeClient::with_base_url(url, "sk_test_abc", "price_x").expect("client");
        match client.create_checkout(&request()).await {
            Err(BillingError::Api { status: 400, body }) => assert!(body.contains("No such price")),
            other => panic!("attendu BillingError::Api 400, obtenu {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_sans_url_rejetee() {
        let (url, _) = fake_stripe(200, json!({ "id": "cs_test_123" })).await;
        let client = StripeClient::with_base_url(url, "sk", "price").expect("client");
        assert!(matches!(
            client.create_checkout(&request()).await,
            Err(BillingError::InvalidResponse(_))
        ));
    }
}
