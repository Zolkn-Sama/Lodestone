use std::time::Duration;

use async_trait::async_trait;
use serde::Deserialize;

use crate::{
    error::BillingError,
    provider::{CheckoutRequest, CheckoutSession, PaymentProvider, PriceInfo, PriceTier},
};

pub const STRIPE_API_BASE: &str = "https://api.stripe.com/v1";

/// Client de l'API Stripe (sessions Checkout, abonnements, prix).
pub struct StripeClient {
    http: reqwest::Client,
    base_url: String,
    secret_key: String,
}

#[derive(Deserialize)]
struct SessionResponse {
    id: String,
    url: Option<String>,
}

#[derive(Deserialize)]
struct PriceResponse {
    currency: String,
    billing_scheme: Option<String>,
    tiers_mode: Option<String>,
    recurring: Option<Recurring>,
    #[serde(default)]
    tiers: Vec<TierResponse>,
}

#[derive(Deserialize)]
struct Recurring {
    interval: String,
}

#[derive(Deserialize)]
struct TierResponse {
    up_to: Option<u64>,
    flat_amount: Option<u64>,
    unit_amount: Option<u64>,
}

#[derive(Deserialize)]
struct ScheduleResponse {
    id: String,
    #[serde(default)]
    phases: Vec<PhaseResponse>,
}

#[derive(Deserialize)]
struct PhaseResponse {
    start_date: i64,
    end_date: Option<i64>,
}

/// Statut d'erreur -> `BillingError::Api` (avec le corps, explicite chez Stripe).
async fn check_status(res: reqwest::Response) -> Result<reqwest::Response, BillingError> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    Err(BillingError::Api {
        status: status.as_u16(),
        body,
    })
}

impl StripeClient {
    pub fn new(secret_key: impl Into<String>) -> Result<Self, BillingError> {
        Self::with_base_url(STRIPE_API_BASE, secret_key)
    }

    /// Pour les tests : pointe vers un faux serveur.
    pub fn with_base_url(
        base_url: impl Into<String>,
        secret_key: impl Into<String>,
    ) -> Result<Self, BillingError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| BillingError::Transport(e.to_string()))?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            secret_key: secret_key.into(),
        })
    }

    /// POST de formulaire authentifié, statut vérifié.
    async fn post_form(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<reqwest::Response, BillingError> {
        let res = self
            .http
            .post(format!("{}{path}", self.base_url))
            .bearer_auth(&self.secret_key)
            .form(form)
            .send()
            .await
            .map_err(|e| BillingError::Transport(e.to_string()))?;
        check_status(res).await
    }
}

async fn json<T: serde::de::DeserializeOwned>(res: reqwest::Response) -> Result<T, BillingError> {
    res.json()
        .await
        .map_err(|e| BillingError::InvalidResponse(e.to_string()))
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
            ("line_items[0][price]", request.price_id.as_str()),
            ("line_items[0][quantity]", seats.as_str()),
            ("client_reference_id", org_id.as_str()),
            ("metadata[org_id]", org_id.as_str()),
            ("subscription_data[metadata][org_id]", org_id.as_str()),
            ("customer_email", request.customer_email.as_str()),
            ("success_url", request.success_url.as_str()),
            ("cancel_url", request.cancel_url.as_str()),
        ];
        let session: SessionResponse =
            json(self.post_form("/checkout/sessions", &form).await?).await?;
        let url = session
            .url
            .ok_or_else(|| BillingError::InvalidResponse("session sans url".into()))?;
        Ok(CheckoutSession {
            id: session.id,
            url,
        })
    }

    async fn update_seats(
        &self,
        subscription_item_id: &str,
        seats: u32,
    ) -> Result<(), BillingError> {
        let seats = seats.to_string();
        self.post_form(
            &format!("/subscription_items/{subscription_item_id}"),
            &[
                ("quantity", seats.as_str()),
                // Ajout/retrait d'un siège en cours de période : facturé au prorata.
                ("proration_behavior", "create_prorations"),
            ],
        )
        .await?;
        Ok(())
    }

    async fn change_price_now(
        &self,
        subscription_item_id: &str,
        price_id: &str,
        seats: u32,
    ) -> Result<(), BillingError> {
        let seats = seats.to_string();
        self.post_form(
            &format!("/subscription_items/{subscription_item_id}"),
            &[
                ("price", price_id),
                ("quantity", seats.as_str()),
                // Crédit du reste de la période + facture de la nouvelle, tout de suite.
                ("proration_behavior", "always_invoice"),
            ],
        )
        .await?;
        Ok(())
    }

    async fn schedule_price_change(
        &self,
        subscription_id: &str,
        from_price_id: &str,
        to_price_id: &str,
        seats: u32,
    ) -> Result<i64, BillingError> {
        // 1. Planning créé depuis l'abonnement : sa phase 0 = la période en cours.
        let schedule: ScheduleResponse = json(
            self.post_form(
                "/subscription_schedules",
                &[("from_subscription", subscription_id)],
            )
            .await?,
        )
        .await?;
        let current = schedule
            .phases
            .first()
            .ok_or_else(|| BillingError::InvalidResponse("planning sans phase".into()))?;
        let end = current
            .end_date
            .ok_or_else(|| BillingError::InvalidResponse("phase en cours sans fin".into()))?;

        // 2. Phase 0 inchangée jusqu'à l'échéance, puis phase 1 au nouveau prix
        //    (une période, puis le planning se retire : l'abonnement continue seul).
        let (start, end_s, seats) = (
            current.start_date.to_string(),
            end.to_string(),
            seats.to_string(),
        );
        self.post_form(
            &format!("/subscription_schedules/{}", schedule.id),
            &[
                ("end_behavior", "release"),
                ("proration_behavior", "none"),
                ("phases[0][items][0][price]", from_price_id),
                ("phases[0][items][0][quantity]", seats.as_str()),
                ("phases[0][start_date]", start.as_str()),
                ("phases[0][end_date]", end_s.as_str()),
                ("phases[1][items][0][price]", to_price_id),
                ("phases[1][items][0][quantity]", seats.as_str()),
                ("phases[1][iterations]", "1"),
            ],
        )
        .await?;
        Ok(end)
    }

    async fn price(&self, price_id: &str) -> Result<PriceInfo, BillingError> {
        let res = self
            .http
            .get(format!("{}/prices/{price_id}", self.base_url))
            .bearer_auth(&self.secret_key)
            .query(&[("expand[]", "tiers")])
            .send()
            .await
            .map_err(|e| BillingError::Transport(e.to_string()))?;
        let price: PriceResponse = json(check_status(res).await?).await?;
        Ok(PriceInfo {
            currency: price.currency,
            interval: price.recurring.map(|r| r.interval),
            tiered_volume: price.billing_scheme.as_deref() == Some("tiered")
                && price.tiers_mode.as_deref() == Some("volume"),
            tiers: price
                .tiers
                .into_iter()
                .map(|t| PriceTier {
                    up_to: t.up_to,
                    flat_amount: t.flat_amount,
                    unit_amount: t.unit_amount,
                })
                .collect(),
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
            price_id: "price_pro".into(),
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
        let client = StripeClient::with_base_url(url, "sk_test_abc").expect("client");

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
        let client = StripeClient::with_base_url(url, "sk_test_abc").expect("client");
        match client.create_checkout(&request()).await {
            Err(BillingError::Api { status: 400, body }) => assert!(body.contains("No such price")),
            other => panic!("attendu BillingError::Api 400, obtenu {other:?}"),
        }
    }

    #[tokio::test]
    async fn session_sans_url_rejetee() {
        let (url, _) = fake_stripe(200, json!({ "id": "cs_test_123" })).await;
        let client = StripeClient::with_base_url(url, "sk").expect("client");
        assert!(matches!(
            client.create_checkout(&request()).await,
            Err(BillingError::InvalidResponse(_))
        ));
    }

    type SeenItem = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

    #[tokio::test]
    async fn met_a_jour_les_sieges_avec_prorata() {
        let seen: SeenItem = Arc::default();
        let app = Router::new()
            .route(
                "/v1/subscription_items/{id}",
                post(
                    move |State(seen): State<SeenItem>,
                          axum::extract::Path(id): axum::extract::Path<String>,
                          Form(form): Form<Vec<(String, String)>>| async move {
                        if let Ok(mut s) = seen.lock() {
                            s.push((id, form));
                        }
                        axum::Json(json!({ "id": "si_1", "quantity": 6 }))
                    },
                ),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });

        let client =
            StripeClient::with_base_url(format!("http://{addr}/v1"), "sk").expect("client");
        client.update_seats("si_1", 6).await.expect("mise à jour");

        let seen = seen.lock().expect("lock");
        assert_eq!(seen[0].0, "si_1");
        let form = &seen[0].1;
        assert!(form.contains(&("quantity".into(), "6".into())));
        assert!(form.contains(&("proration_behavior".into(), "create_prorations".into())));
    }

    #[tokio::test]
    async fn lit_les_paliers_du_prix() {
        let app = Router::new().route(
            "/v1/prices/{id}",
            axum::routing::get(
                |axum::extract::Path(id): axum::extract::Path<String>,
                 axum::extract::RawQuery(query): axum::extract::RawQuery| async move {
                    assert_eq!(id, "price_pro");
                    assert!(
                        query.unwrap_or_default().contains("tiers"),
                        "tiers demandés"
                    );
                    axum::Json(json!({
                        "id": "price_pro", "currency": "eur",
                        "billing_scheme": "tiered", "tiers_mode": "volume",
                        "recurring": { "interval": "month" },
                        "tiers": [
                            { "up_to": 5, "flat_amount": 14900, "unit_amount": 0 },
                            { "up_to": null, "flat_amount": null, "unit_amount": 2900 }
                        ]
                    }))
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });

        let client =
            StripeClient::with_base_url(format!("http://{addr}/v1"), "sk").expect("client");
        let price = client.price("price_pro").await.expect("prix");
        assert_eq!(
            price,
            crate::MockPaymentProvider::tiered_price("month", 5, 14900, 2900)
        );
    }

    type SeenForms = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

    /// Faux Stripe pour les changements de prix : enregistre (chemin, formulaire).
    async fn fake_stripe_plans() -> (String, SeenForms) {
        let seen: SeenForms = Arc::default();
        let record = |seen: SeenForms, path: String, form: Vec<(String, String)>| {
            if let Ok(mut s) = seen.lock() {
                s.push((path, form));
            }
        };
        let app = Router::new()
            .route(
                "/v1/subscription_items/{id}",
                post(
                    move |State(seen): State<SeenForms>,
                          axum::extract::Path(id): axum::extract::Path<String>,
                          Form(form): Form<Vec<(String, String)>>| async move {
                        record(seen, format!("items/{id}"), form);
                        axum::Json(json!({ "id": id }))
                    },
                ),
            )
            .route(
                "/v1/subscription_schedules",
                post(
                    move |State(seen): State<SeenForms>,
                          Form(form): Form<Vec<(String, String)>>| async move {
                        record(seen, "schedules".into(), form);
                        axum::Json(json!({
                            "id": "sub_sched_1",
                            "phases": [ { "start_date": 1_700_000_000, "end_date": 1_731_536_000 } ]
                        }))
                    },
                ),
            )
            .route(
                "/v1/subscription_schedules/{id}",
                post(
                    move |State(seen): State<SeenForms>,
                          axum::extract::Path(id): axum::extract::Path<String>,
                          Form(form): Form<Vec<(String, String)>>| async move {
                        record(seen, format!("schedules/{id}"), form);
                        axum::Json(json!({ "id": id }))
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

    fn has(form: &[(String, String)], key: &str, value: &str) -> bool {
        form.contains(&(key.to_string(), value.to_string()))
    }

    #[tokio::test]
    async fn passage_immediat_avec_facture_au_prorata() {
        let (url, seen) = fake_stripe_plans().await;
        let client = StripeClient::with_base_url(url, "sk").expect("client");
        client
            .change_price_now("si_1", "price_annual", 6)
            .await
            .expect("changement");

        let seen = seen.lock().expect("lock");
        let (path, form) = &seen[0];
        assert_eq!(path, "items/si_1");
        assert!(has(form, "price", "price_annual"));
        assert!(has(form, "quantity", "6"));
        assert!(has(form, "proration_behavior", "always_invoice"));
    }

    #[tokio::test]
    async fn passage_programme_a_l_echeance() {
        let (url, seen) = fake_stripe_plans().await;
        let client = StripeClient::with_base_url(url, "sk").expect("client");
        let switch_at = client
            .schedule_price_change("sub_1", "price_annual", "price_monthly", 4)
            .await
            .expect("planning");
        assert_eq!(switch_at, 1_731_536_000, "fin de la période en cours");

        let seen = seen.lock().expect("lock");
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0].0, "schedules");
        assert!(has(&seen[0].1, "from_subscription", "sub_1"));

        let (path, form) = &seen[1];
        assert_eq!(path, "schedules/sub_sched_1");
        for (k, v) in [
            ("end_behavior", "release"),
            ("proration_behavior", "none"),
            ("phases[0][items][0][price]", "price_annual"),
            ("phases[0][items][0][quantity]", "4"),
            ("phases[0][start_date]", "1700000000"),
            ("phases[0][end_date]", "1731536000"),
            ("phases[1][items][0][price]", "price_monthly"),
            ("phases[1][items][0][quantity]", "4"),
            ("phases[1][iterations]", "1"),
        ] {
            assert!(has(form, k, v), "{k}={v} absent de {form:?}");
        }
    }
}
