//! BILL-06 : abonnement annuel avec remise, et changement de périodicité.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{ANNUAL_PRICE, MONTHLY_PRICE, TestApp, TestUser};
use lodestone_backend::repositories::organizations;
use lodestone_billing::{MOCK_PERIOD_END, MockPaymentProvider, PriceChange};
use lodestone_core::plan::Plan;

const T0: i64 = 1_760_000_000;

/// Webhook d'abonnement actif avec la périodicité `interval` (`month` / `year`).
async fn subscription(
    app: &TestApp,
    event_id: &str,
    at: i64,
    org: Uuid,
    interval: &str,
    seats: u32,
) {
    let price = if interval == "year" {
        ANNUAL_PRICE
    } else {
        MONTHLY_PRICE
    };
    let event = json!({
        "id": event_id, "type": "customer.subscription.updated", "created": at,
        "data": { "object": {
            "id": "sub_1", "customer": "cus_1", "status": "active",
            "metadata": { "org_id": org },
            "items": { "data": [ {
                "id": "si_1", "quantity": seats, "current_period_end": T0 + 30 * 86_400,
                "price": { "id": price, "recurring": { "interval": interval } }
            } ] }
        } }
    });
    let (status, body) = app.send_webhook(&event).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

async fn billing(app: &TestApp, org: Uuid, user: &TestUser) -> Value {
    let (status, body) = app
        .get(&format!("/api/orgs/{org}/billing"), &user.token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn change(app: &TestApp, org: Uuid, user: &TestUser, interval: &str) -> (StatusCode, Value) {
    app.post(
        &format!("/api/orgs/{org}/billing/interval"),
        &user.token,
        json!({ "interval": interval }),
    )
    .await
}

async fn checkout(
    app: &TestApp,
    org: Uuid,
    user: &TestUser,
    body: Option<Value>,
) -> (StatusCode, Value) {
    app.request(
        Method::POST,
        &format!("/api/orgs/{org}/billing/checkout"),
        Some(&user.token),
        body,
    )
    .await
}

// ----- Choix à la souscription -----------------------------------------------------

#[tokio::test]
async fn les_deux_formules_sont_proposees() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let body = billing(&app, org, &owner).await;
    assert_eq!(body["annual_available"], true);
    assert_eq!(body["pro_pricing"]["annual_discount_percent"], 15);
    assert_eq!(body["quotes"]["month"]["total_cents"], 14900);
    assert_eq!(body["quotes"]["year"]["interval"], "year");
    assert_eq!(
        body["quotes"]["year"]["total_cents"], 151_980,
        "socle ramené à l'année, moins 15 %"
    );
    assert_eq!(body["quotes"]["year"]["floor_applied"], true);
    for key in ["to_year", "to_month", "seats"] {
        assert!(
            body["switch_policy"][key]
                .as_str()
                .is_some_and(|p| !p.is_empty()),
            "{key}"
        );
    }
}

#[tokio::test]
async fn souscription_mensuelle_ou_annuelle() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    for (body, expected_price) in [
        (None, MONTHLY_PRICE),
        (Some(json!({})), MONTHLY_PRICE),
        (Some(json!({ "interval": "month" })), MONTHLY_PRICE),
        (Some(json!({ "interval": "year" })), ANNUAL_PRICE),
    ] {
        let (status, response) = checkout(&app, org, &owner, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{body:?} : {response}");
        let last = app.billing.checkout_requests().pop().expect("demande");
        assert_eq!(last.items[0].price_id, expected_price, "{body:?}");
    }

    let (status, _) = checkout(&app, org, &owner, Some(json!({ "interval": "week" }))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn sans_prix_annuel_seul_le_mensuel() {
    let app = TestApp::spawn_without_annual().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let body = billing(&app, org, &owner).await;
    assert_eq!(body["annual_available"], false);
    assert_eq!(body["quotes"]["year"], Value::Null);

    let (status, response) = checkout(&app, org, &owner, Some(json!({ "interval": "year" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        response["error"]
            .as_str()
            .is_some_and(|m| m.contains("annuel")),
        "{response}"
    );
    let (status, _) = checkout(&app, org, &owner, None).await;
    assert_eq!(status, StatusCode::OK, "le mensuel reste possible");
}

#[tokio::test]
async fn abonnement_annuel_facture_a_l_annee() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscription(&app, "evt_1", T0, org, "year", 1).await;

    let body = billing(&app, org, &owner).await;
    assert_eq!(body["plan"], "pro");
    assert_eq!(body["subscription"]["interval"], "year");
    assert_eq!(body["subscription"]["current_period_end"], T0 + 30 * 86_400);
    assert_eq!(body["current_quote"]["interval"], "year");
    assert_eq!(body["current_quote"]["total_cents"], 151_980);
}

// ----- Changement de périodicité ------------------------------------------------------

#[tokio::test]
async fn mensuel_vers_annuel_immediat() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscription(&app, "evt_1", T0, org, "month", 1).await;

    let (status, body) = change(&app, org, &owner, "year").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["interval"], "year");
    assert_eq!(body["effective"], "now");
    assert_eq!(body["effective_at"], Value::Null);
    assert!(
        body["policy"]
            .as_str()
            .is_some_and(|p| p.contains("prorata"))
    );

    assert_eq!(
        app.billing.price_changes(),
        vec![PriceChange::Now {
            item_id: "si_1".into(),
            price_id: ANNUAL_PRICE.into(),
            seats: 1
        }]
    );
    // Visible tout de suite, sans attendre le webhook.
    let status = billing(&app, org, &owner).await;
    assert_eq!(status["subscription"]["interval"], "year");
    assert_eq!(status["current_quote"]["interval"], "year");
}

#[tokio::test]
async fn annuel_vers_mensuel_a_l_echeance() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscription(&app, "evt_1", T0, org, "year", 1).await;

    let (status, body) = change(&app, org, &owner, "month").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["effective"], "at_period_end");
    assert_eq!(body["effective_at"], MOCK_PERIOD_END);
    assert!(
        body["policy"]
            .as_str()
            .is_some_and(|p| p.contains("remboursement"))
    );
    assert_eq!(
        app.billing.price_changes(),
        vec![PriceChange::Scheduled {
            subscription_id: "sub_1".into(),
            from_price_id: ANNUAL_PRICE.into(),
            to_price_id: MONTHLY_PRICE.into(),
            seats: 1
        }]
    );

    // Jusqu'à l'échéance : toujours annuel, bascule programmée.
    let status = billing(&app, org, &owner).await;
    assert_eq!(status["subscription"]["interval"], "year");
    assert_eq!(status["subscription"]["scheduled_interval"], "month");

    // Deuxième demande avant l'échéance : refusée.
    let (status, _) = change(&app, org, &owner, "month").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // À l'échéance, Stripe bascule l'abonnement et prévient.
    subscription(&app, "evt_2", T0 + 100, org, "month", 1).await;
    let status = billing(&app, org, &owner).await;
    assert_eq!(status["subscription"]["interval"], "month");
    assert_eq!(status["subscription"]["scheduled_interval"], Value::Null);
    assert_eq!(status["current_quote"]["total_cents"], 14900);
}

#[tokio::test]
async fn changements_refuses() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;

    // Pas d'abonnement (Free).
    let (status, _) = change(&app, org, &owner, "year").await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Abonné mais périodicité encore inconnue (seul le paiement est arrivé).
    let (status, _) = app
        .send_webhook(&json!({
            "id": "evt_co", "type": "checkout.session.completed", "created": T0,
            "data": { "object": { "id": "cs_1", "subscription": "sub_1", "metadata": { "org_id": org } } }
        }))
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, body) = change(&app, org, &owner, "year").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("réessayez")),
        "{body}"
    );

    // Déjà dans la périodicité demandée ; réservé à l'owner.
    subscription(&app, "evt_1", T0 + 1, org, "month", 2).await;
    let (status, _) = change(&app, org, &owner, "month").await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = change(&app, org, &admin, "year").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = change(&app, org, &owner, "week").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Sovereign : pas d'abonnement Stripe.
    organizations::set_plan(&app.db, org, Plan::Sovereign)
        .await
        .expect("plan");
    assert!(app.billing.price_changes().is_empty());
}

#[tokio::test]
async fn stripe_en_panne_rien_ne_change() {
    let app = TestApp::spawn_with_billing(MockPaymentProvider::failing()).await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscription(&app, "evt_1", T0, org, "month", 1).await;

    let (status, _) = change(&app, org, &owner, "year").await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    let body = billing(&app, org, &owner).await;
    assert_eq!(body["subscription"]["interval"], "month", "inchangé");
    assert_eq!(body["subscription"]["scheduled_interval"], Value::Null);
}

#[tokio::test]
async fn sans_prix_annuel_pas_de_changement() {
    let app = TestApp::spawn_without_annual().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscription(&app, "evt_1", T0, org, "month", 1).await;
    let (status, _) = change(&app, org, &owner, "year").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
