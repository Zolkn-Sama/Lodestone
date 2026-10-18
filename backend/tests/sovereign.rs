//! BILL-07 : activation Sovereign — licence annuelle + mise en service
//! facturée une seule fois.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{SOVEREIGN_LICENSE_PRICE, SOVEREIGN_SETUP_PRICE, TestApp, TestUser};
use lodestone_backend::repositories::organizations;
use lodestone_core::plan::Plan;

const T0: i64 = 1_760_000_000;

/// Une org passée en Sovereign par l'équipe Lodestone (contrat signé).
async fn sovereign_org(app: &TestApp, owner: &TestUser) -> Uuid {
    let org = app.create_org(owner).await;
    organizations::set_plan(&app.db, org, Plan::Sovereign)
        .await
        .expect("plan");
    org
}

async fn checkout(app: &TestApp, org: Uuid, user: &TestUser) -> (StatusCode, Value) {
    app.request(
        Method::POST,
        &format!("/api/orgs/{org}/billing/sovereign/checkout"),
        Some(&user.token),
        None,
    )
    .await
}

async fn billing(app: &TestApp, org: Uuid, user: &TestUser) -> Value {
    let (status, body) = app
        .get(&format!("/api/orgs/{org}/billing"), &user.token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

/// Webhook de paiement d'une session Sovereign.
async fn paid(app: &TestApp, event_id: &str, at: i64, org: Uuid, sub: &str, setup_fee: &str) {
    let (status, body) = app
        .send_webhook(&json!({
            "id": event_id, "type": "checkout.session.completed", "created": at,
            "data": { "object": {
                "id": "cs_1", "customer": "cus_1", "subscription": sub,
                "metadata": { "org_id": org, "product": "sovereign", "setup_fee": setup_fee }
            } }
        }))
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

/// Webhook d'abonnement Sovereign (renouvellement, résiliation…).
async fn sovereign_subscription(
    app: &TestApp,
    event_id: &str,
    at: i64,
    org: Uuid,
    sub: &str,
    status: &str,
) {
    let (code, body) = app
        .send_webhook(&json!({
            "id": event_id, "type": "customer.subscription.updated", "created": at,
            "data": { "object": {
                "id": sub, "customer": "cus_1", "status": status,
                "metadata": { "org_id": org, "product": "sovereign" },
                "items": { "data": [ {
                    "id": "si_lic", "quantity": 1,
                    "price": { "id": SOVEREIGN_LICENSE_PRICE, "recurring": { "interval": "year" } }
                } ] }
            } }
        }))
        .await;
    assert_eq!(code, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn premiere_activation_licence_plus_mise_en_service() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = sovereign_org(&app, &owner).await;

    let body = billing(&app, org, &owner).await;
    assert_eq!(
        body["sovereign"],
        json!({
            "pricing": { "license_cents_per_year": 1_800_000, "setup_cents": 600_000 },
            "checkout_available": true,
            "setup_fee_paid_at": null,
            "next_checkout_cents": 2_400_000
        })
    );
    assert_eq!(
        body["current_quote"],
        Value::Null,
        "pas de socle Pro en Sovereign"
    );

    let (status, response) = checkout(&app, org, &owner).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["setup_fee_included"], true);

    let req = app.billing.checkout_requests().pop().expect("demande");
    let lines: Vec<(&str, u32)> = req
        .items
        .iter()
        .map(|i| (i.price_id.as_str(), i.quantity))
        .collect();
    assert_eq!(
        lines,
        vec![(SOVEREIGN_LICENSE_PRICE, 1), (SOVEREIGN_SETUP_PRICE, 1)],
        "licence récurrente + mise en service ponctuelle, dans la même session"
    );
    assert!(
        req.metadata
            .contains(&("product".into(), "sovereign".into()))
    );
    assert!(
        req.metadata
            .contains(&("setup_fee".into(), "included".into()))
    );
}

#[tokio::test]
async fn paiement_puis_renouvellements_sans_nouvelle_mise_en_service() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = sovereign_org(&app, &owner).await;

    paid(&app, "evt_1", T0, org, "sub_sov", "included").await;
    let body = billing(&app, org, &owner).await;
    assert_eq!(
        body["plan"], "sovereign",
        "le plan reste Sovereign (pas Pro)"
    );
    assert_eq!(body["subscription"]["status"], "active");
    let paid_at = body["sovereign"]["setup_fee_paid_at"]
        .as_i64()
        .expect("payée");
    assert_eq!(body["sovereign"]["next_checkout_cents"], 1_800_000);

    // Licence active : pas de seconde souscription.
    let (status, _) = checkout(&app, org, &owner).await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Renouvellement annuel : la date de mise en service ne bouge pas.
    sovereign_subscription(&app, "evt_2", T0 + 365 * 86_400, org, "sub_sov", "active").await;
    let body = billing(&app, org, &owner).await;
    assert_eq!(body["sovereign"]["setup_fee_paid_at"], paid_at);

    // Résiliation puis réactivation : licence seule, JAMAIS la mise en service.
    sovereign_subscription(&app, "evt_3", T0 + 400 * 86_400, org, "sub_sov", "canceled").await;
    assert_eq!(billing(&app, org, &owner).await["plan"], "sovereign");
    let (status, response) = checkout(&app, org, &owner).await;
    assert_eq!(status, StatusCode::OK, "{response}");
    assert_eq!(response["setup_fee_included"], false);
    let req = app.billing.checkout_requests().pop().expect("demande");
    let prices: Vec<&str> = req.items.iter().map(|i| i.price_id.as_str()).collect();
    assert_eq!(prices, vec![SOVEREIGN_LICENSE_PRICE]);
    assert!(
        req.metadata
            .contains(&("setup_fee".into(), "already_paid".into()))
    );

    // Même si un webhook prétend l'inclure à nouveau, la date d'origine reste.
    paid(
        &app,
        "evt_4",
        T0 + 401 * 86_400,
        org,
        "sub_sov2",
        "included",
    )
    .await;
    assert_eq!(
        billing(&app, org, &owner).await["sovereign"]["setup_fee_paid_at"],
        paid_at
    );
}

#[tokio::test]
async fn sovereign_sans_effet_pro() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = sovereign_org(&app, &owner).await;
    paid(&app, "evt_1", T0, org, "sub_sov", "included").await;

    // Ni synchro de sièges, ni changement de périodicité sur une licence Sovereign.
    let member = app.register("member").await;
    app.add_member(org, &owner, &member, "member").await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        app.billing.seat_updates().is_empty(),
        "{:?}",
        app.billing.seat_updates()
    );
    let (status, _) = app
        .post(
            &format!("/api/orgs/{org}/billing/interval"),
            &owner.token,
            json!({ "interval": "month" }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
}

#[tokio::test]
async fn reserve_aux_orgs_sovereign_et_a_l_owner() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;

    // Org Free : la souscription Sovereign suppose un contrat préalable.
    let (status, body) = checkout(&app, org, &owner).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("Sovereign")),
        "{body}"
    );
    assert_eq!(billing(&app, org, &owner).await["sovereign"], Value::Null);

    organizations::set_plan(&app.db, org, Plan::Sovereign)
        .await
        .expect("plan");
    let (status, _) = checkout(&app, org, &admin).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(app.billing.checkout_requests().is_empty());
}

#[tokio::test]
async fn un_paiement_sovereign_ne_change_pas_une_org_non_sovereign() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    paid(&app, "evt_1", T0, org, "sub_sov", "included").await;
    let body = billing(&app, org, &owner).await;
    assert_eq!(body["plan"], "free");
    assert_eq!(body["subscription"], Value::Null);
    let paid_at = organizations::sovereign_setup_paid_at(&app.db, org)
        .await
        .expect("requête");
    assert!(paid_at.is_none());
}

#[tokio::test]
async fn sans_prix_sovereign_configures() {
    let app = TestApp::spawn_without_billing().await;
    let owner = app.register("owner").await;
    let org = sovereign_org(&app, &owner).await;

    let (status, _) = checkout(&app, org, &owner).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let body = billing(&app, org, &owner).await;
    assert_eq!(body["sovereign"]["checkout_available"], false);
}
