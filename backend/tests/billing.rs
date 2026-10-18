//! BILL-03 : passage en Pro via Stripe (session de paiement + webhooks signés).
//! Stripe est simulé : `MockPaymentProvider` et webhooks signés avec le vrai algorithme.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode},
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser};
use lodestone_backend::repositories::organizations;
use lodestone_core::plan::Plan;

/// Horodatage Stripe d'événement (secondes), croissant dans les tests.
const T0: i64 = 1_760_000_000;

fn checkout_completed(event_id: &str, created: i64, org: Uuid, subscription: &str) -> Value {
    json!({
        "id": event_id, "type": "checkout.session.completed", "created": created,
        "data": { "object": {
            "id": "cs_1", "mode": "subscription", "customer": "cus_1",
            "subscription": subscription, "metadata": { "org_id": org }
        } }
    })
}

fn subscription_event(
    event_id: &str,
    event_type: &str,
    created: i64,
    org: Option<Uuid>,
    subscription: &str,
    status: &str,
    seats: u32,
) -> Value {
    let metadata = org.map_or(json!({}), |o| json!({ "org_id": o }));
    json!({
        "id": event_id, "type": event_type, "created": created,
        "data": { "object": {
            "id": subscription, "customer": "cus_1", "status": status,
            "metadata": metadata,
            "items": { "data": [ { "quantity": seats } ] }
        } }
    })
}

async fn plan(app: &TestApp, org: Uuid, user: &TestUser) -> String {
    let (_, body) = app.get(&format!("/api/orgs/{org}"), &user.token).await;
    body["plan"].as_str().expect("plan").to_string()
}

async fn billing(app: &TestApp, org: Uuid, user: &TestUser) -> Value {
    let (status, body) = app
        .get(&format!("/api/orgs/{org}/billing"), &user.token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn webhook_ok(app: &TestApp, event: &Value) {
    let (status, body) = app.send_webhook(event).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// ----- Session de paiement ---------------------------------------------------------

#[tokio::test]
async fn l_owner_obtient_une_page_de_paiement() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;

    let (status, body) = app
        .request(
            Method::POST,
            &format!("/api/orgs/{org}/billing/checkout"),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!({ "url": "https://checkout.mock/cs_mock_1" }));

    let requests = app.billing.checkout_requests();
    assert_eq!(requests.len(), 1);
    let req = &requests[0];
    assert_eq!(req.org_id, org);
    assert_eq!(req.items.len(), 1, "une seule ligne : l'abonnement Pro");
    assert_eq!(req.items[0].quantity, 2, "un siège par membre");
    assert!(
        req.metadata
            .contains(&("product".to_string(), "pro".to_string()))
    );
    assert_eq!(req.customer_email, owner.email);
    assert_eq!(
        req.success_url,
        format!("https://app.test/orgs/{org}/billing?checkout=success")
    );
    assert_eq!(
        req.cancel_url,
        format!("https://app.test/orgs/{org}/billing?checkout=cancel")
    );
}

#[tokio::test]
async fn session_reservee_a_l_owner_d_une_org_free() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;
    let uri = format!("/api/orgs/{org}/billing/checkout");

    for (user, expected) in [
        (&admin, StatusCode::FORBIDDEN),
        (&outsider, StatusCode::NOT_FOUND),
    ] {
        let (status, _) = app
            .request(Method::POST, &uri, Some(&user.token), None)
            .await;
        assert_eq!(status, expected, "{}", user.email);
    }

    for (plan, message) in [
        (Plan::Pro, "déjà en Pro"),
        (Plan::Sovereign, "souscription Sovereign"),
    ] {
        organizations::set_plan(&app.db, org, plan)
            .await
            .expect("plan");
        let (status, body) = app
            .request(Method::POST, &uri, Some(&owner.token), None)
            .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(
            body["error"].as_str().is_some_and(|m| m.contains(message)),
            "{body}"
        );
    }
    assert!(app.billing.checkout_requests().is_empty());
}

#[tokio::test]
async fn sans_stripe_la_facturation_est_desactivee() {
    let app = TestApp::spawn_without_billing().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let (status, _) = app
        .request(
            Method::POST,
            &format!("/api/orgs/{org}/billing/checkout"),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    let (status, _) = app
        .send_webhook(&checkout_completed("evt_1", T0, org, "sub_1"))
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(billing(&app, org, &owner).await["billing_enabled"], false);
}

// ----- Webhooks ----------------------------------------------------------------------

#[tokio::test]
async fn paiement_reussi_passe_en_pro_et_leve_les_quotas() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let invite = |email: &'static str| {
        let app = &app;
        let token = owner.token.clone();
        async move {
            app.post(
                &format!("/api/orgs/{org}/invitations"),
                &token,
                json!({ "email": email }),
            )
            .await
            .0
        }
    };

    // Free : 3e siège refusé.
    assert_eq!(invite("a@test.io").await, StatusCode::CREATED);
    assert_eq!(invite("b@test.io").await, StatusCode::CREATED);
    assert_eq!(invite("c@test.io").await, StatusCode::PAYMENT_REQUIRED);

    webhook_ok(&app, &checkout_completed("evt_1", T0, org, "sub_1")).await;
    assert_eq!(plan(&app, org, &owner).await, "pro");
    // Effet immédiat sur les quotas.
    assert_eq!(invite("c@test.io").await, StatusCode::CREATED);

    let body = billing(&app, org, &owner).await;
    assert_eq!(body["plan"], "pro");
    assert_eq!(body["billing_enabled"], true);
    assert_eq!(body["subscription"]["status"], "active");

    // Les sièges arrivent avec la mise à jour de l'abonnement.
    webhook_ok(
        &app,
        &subscription_event(
            "evt_2",
            "customer.subscription.updated",
            T0 + 1,
            Some(org),
            "sub_1",
            "active",
            5,
        ),
    )
    .await;
    assert_eq!(billing(&app, org, &owner).await["subscription"]["seats"], 5);
}

#[tokio::test]
async fn cycle_de_vie_de_l_abonnement() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    webhook_ok(&app, &checkout_completed("evt_1", T0, org, "sub_1")).await;

    // Les événements suivants ne portent pas forcément l'org : on la retrouve par l'abonnement.
    let steps = [
        ("past_due", "pro"), // relances de paiement : on garde le Pro
        ("unpaid", "free"),  // relances épuisées
        ("active", "pro"),   // payé finalement
        ("canceled", "free"),
    ];
    for (i, (status, expected)) in steps.into_iter().enumerate() {
        let n = i as i64 + 1;
        webhook_ok(
            &app,
            &subscription_event(
                &format!("evt_s{n}"),
                "customer.subscription.updated",
                T0 + n,
                None,
                "sub_1",
                status,
                3,
            ),
        )
        .await;
        assert_eq!(plan(&app, org, &owner).await, expected, "après {status}");
    }

    // Résiliation définitive.
    webhook_ok(&app, &checkout_completed("evt_back", T0 + 10, org, "sub_1")).await;
    assert_eq!(plan(&app, org, &owner).await, "pro");
    webhook_ok(
        &app,
        &subscription_event(
            "evt_del",
            "customer.subscription.deleted",
            T0 + 11,
            None,
            "sub_1",
            "active",
            3,
        ),
    )
    .await;
    assert_eq!(plan(&app, org, &owner).await, "free");
    assert_eq!(
        billing(&app, org, &owner).await["subscription"]["status"],
        "canceled"
    );
}

#[tokio::test]
async fn doublons_et_desordre() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let start = checkout_completed("evt_1", T0, org, "sub_1");
    webhook_ok(&app, &start).await;
    webhook_ok(
        &app,
        &subscription_event(
            "evt_2",
            "customer.subscription.deleted",
            T0 + 10,
            Some(org),
            "sub_1",
            "canceled",
            1,
        ),
    )
    .await;
    assert_eq!(plan(&app, org, &owner).await, "free");

    // Stripe renvoie le 1er événement : déjà traité, sans effet.
    webhook_ok(&app, &start).await;
    assert_eq!(plan(&app, org, &owner).await, "free");

    // Un événement plus ancien que le dernier appliqué arrive en retard : ignoré.
    webhook_ok(
        &app,
        &subscription_event(
            "evt_late",
            "customer.subscription.updated",
            T0 + 5,
            Some(org),
            "sub_1",
            "active",
            1,
        ),
    )
    .await;
    assert_eq!(plan(&app, org, &owner).await, "free");
}

#[tokio::test]
async fn la_fin_d_un_ancien_abonnement_ne_retrograde_pas() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    webhook_ok(&app, &checkout_completed("evt_1", T0, org, "sub_old")).await;
    // Nouvel abonnement (ex. après une résiliation puis un ré-abonnement).
    webhook_ok(&app, &checkout_completed("evt_2", T0 + 10, org, "sub_new")).await;
    // L'ancien est clôturé plus tard par Stripe.
    webhook_ok(
        &app,
        &subscription_event(
            "evt_3",
            "customer.subscription.deleted",
            T0 + 20,
            Some(org),
            "sub_old",
            "canceled",
            1,
        ),
    )
    .await;
    assert_eq!(plan(&app, org, &owner).await, "pro");
}

#[tokio::test]
async fn sovereign_et_orgs_inconnues_ignorees() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    organizations::set_plan(&app.db, org, Plan::Sovereign)
        .await
        .expect("plan");

    webhook_ok(
        &app,
        &subscription_event(
            "evt_1",
            "customer.subscription.deleted",
            T0,
            Some(org),
            "sub_1",
            "canceled",
            1,
        ),
    )
    .await;
    assert_eq!(
        plan(&app, org, &owner).await,
        "sovereign",
        "Sovereign géré hors Stripe"
    );

    // Org inexistante, abonnement inconnu, type non exploité : 200 (sinon Stripe réessaie).
    webhook_ok(
        &app,
        &checkout_completed("evt_2", T0, Uuid::new_v4(), "sub_x"),
    )
    .await;
    webhook_ok(
        &app,
        &subscription_event(
            "evt_3",
            "customer.subscription.updated",
            T0,
            None,
            "sub_inconnu",
            "active",
            1,
        ),
    )
    .await;
    webhook_ok(
        &app,
        &json!({ "id": "evt_4", "type": "invoice.paid", "created": T0, "data": { "object": {} } }),
    )
    .await;
}

#[tokio::test]
async fn webhooks_non_signes_refuses() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let body = checkout_completed("evt_1", T0, org, "sub_1").to_string();

    let now = chrono::Utc::now().timestamp();
    let cases = [
        None,                                            // sans signature
        Some(format!("t={now},v1={}", "00".repeat(32))), // fausse signature
        Some(lodestone_billing::sign_payload(
            body.as_bytes(),
            "whsec_autre",
            now,
        )), // mauvais secret
        Some(lodestone_billing::sign_payload(
            body.as_bytes(),
            common::WEBHOOK_SECRET,
            now - 3600,
        )), // rejeu
    ];
    for signature in cases {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/api/billing/webhook");
        if let Some(s) = &signature {
            req = req.header("stripe-signature", s);
        }
        let res = app
            .send(req.body(Body::from(body.clone())).expect("requête"))
            .await;
        let status = res.status();
        let text = res.into_body().collect().await.expect("corps").to_bytes();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{signature:?} : {text:?}");
    }
    assert_eq!(plan(&app, org, &owner).await, "free", "aucun effet");
}

#[tokio::test]
async fn statut_de_facturation_reserve_aux_admins() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;

    let body = billing(&app, org, &owner).await;
    assert_eq!(body["plan"], "free");
    assert_eq!(body["billing_enabled"], true);
    assert_eq!(body["subscription"], Value::Null);
    let (status, _) = app
        .get(&format!("/api/orgs/{org}/billing"), &member.token)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
