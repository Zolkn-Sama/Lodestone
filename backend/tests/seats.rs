//! BILL-05 : socle minimum Pro, tarif configurable, sièges synchronisés avec Stripe.

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser};
use lodestone_backend::repositories::organizations;
use lodestone_core::plan::Plan;

const T0: i64 = 1_760_000_000;

/// Passe l'org en Pro par webhook, avec la ligne d'abonnement `si_1` à `seats` sièges.
async fn subscribe(app: &TestApp, org: Uuid, seats: u32) {
    let event = json!({
        "id": format!("evt_{org}"), "type": "customer.subscription.created", "created": T0,
        "data": { "object": {
            "id": "sub_1", "customer": "cus_1", "status": "active",
            "metadata": { "org_id": org },
            "items": { "data": [ { "id": "si_1", "quantity": seats } ] }
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

/// Attend que le mock ait reçu `n` mises à jour de sièges (synchro en tâche de fond).
/// Attend que la dernière quantité envoyée à Stripe vaille `seats`. Les synchros
/// tournent en tâche de fond : deux arrivées rapprochées peuvent donner une
/// seule mise à jour (directement la valeur finale) ; seule la valeur finale compte.
async fn wait_last_seats(app: &TestApp, seats: u32) {
    for _ in 0..100 {
        if app.billing.seat_updates().last() == Some(&("si_1".to_string(), seats)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "dernière quantité attendue : {seats}, reçu {:?}",
        app.billing.seat_updates()
    );
}

async fn add_members(app: &TestApp, org: Uuid, owner: &TestUser, n: usize) -> Vec<TestUser> {
    let mut users = Vec::new();
    for i in 0..n {
        let user = app.register(&format!("m{i}")).await;
        app.add_member(org, owner, &user, "member").await;
        users.push(user);
    }
    users
}

// ----- Socle et tarif ---------------------------------------------------------------

#[tokio::test]
async fn estimation_en_free_avec_le_socle() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let body = billing(&app, org, &owner).await;
    assert_eq!(
        body["pro_pricing"],
        json!({ "seat_price_cents": 2900, "floor_cents": 14900, "annual_discount_percent": 15 })
    );
    assert_eq!(
        body["current_quote"],
        json!({
            "interval": "month", "seats": 1, "seats_total_cents": 2900,
            "total_cents": 14900, "floor_applied": true
        }),
        "1 siège : le socle s'applique"
    );
}

#[tokio::test]
async fn en_pro_le_prix_par_siege_prend_le_relais() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscribe(&app, org, 1).await;

    // Les sièges suivent les membres : on en ajoute jusqu'à passer le socle.
    add_members(&app, org, &owner, 3).await;
    for (seats, (total, floor)) in (4..).zip([(14900, true), (14900, true), (17400, false)]) {
        wait_last_seats(&app, seats).await;
        let quote = billing(&app, org, &owner).await["current_quote"].clone();
        assert_eq!(quote["seats"], seats, "{quote}");
        assert_eq!(quote["total_cents"], total, "{seats} sièges");
        assert_eq!(quote["floor_applied"], floor, "{seats} sièges");
        let user = app.register(&format!("extra{seats}")).await;
        app.add_member(org, &owner, &user, "member").await;
    }
}

#[tokio::test]
async fn un_webhook_realigne_les_sieges_sur_les_membres() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    add_members(&app, org, &owner, 1).await;

    // Stripe annonce 7 sièges (ex. après une bascule programmée) pour 2 membres.
    subscribe(&app, org, 7).await;
    wait_last_seats(&app, 2).await;
    assert_eq!(billing(&app, org, &owner).await["subscription"]["seats"], 2);
}

#[tokio::test]
async fn sovereign_sans_socle() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    organizations::set_plan(&app.db, org, Plan::Sovereign)
        .await
        .expect("plan");
    assert_eq!(
        billing(&app, org, &owner).await["current_quote"],
        Value::Null
    );
}

// ----- Synchronisation des sièges ------------------------------------------------------

#[tokio::test]
async fn arrivees_et_departs_mettent_a_jour_stripe() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscribe(&app, org, 1).await;
    assert!(
        app.billing.seat_updates().is_empty(),
        "rien à synchroniser au départ"
    );

    // Deux arrivées : 3 sièges.
    let members = add_members(&app, org, &owner, 2).await;
    wait_last_seats(&app, 3).await;

    // Un départ : 2 sièges.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/orgs/{org}/members/{}", members[0].id),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    wait_last_seats(&app, 2).await;

    // Les sièges enregistrés suivent : le prix affiché aussi.
    let body = billing(&app, org, &owner).await;
    assert_eq!(body["subscription"]["seats"], 2);
    assert_eq!(body["current_quote"]["total_cents"], 14900);
}

#[tokio::test]
async fn pas_de_synchro_hors_abonnement_pro() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    // Free : rien n'est envoyé à Stripe.
    add_members(&app, org, &owner, 1).await;

    // Abonnement résilié : rien non plus.
    subscribe(&app, org, 2).await;
    let (status, _) = app
        .send_webhook(&json!({
            "id": "evt_cancel", "type": "customer.subscription.deleted", "created": T0 + 100,
            "data": { "object": { "id": "sub_1", "status": "canceled", "metadata": { "org_id": org } } }
        }))
        .await;
    assert_eq!(status, StatusCode::OK);
    add_members(&app, org, &owner, 1).await;

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        app.billing.seat_updates().is_empty(),
        "{:?}",
        app.billing.seat_updates()
    );
}

#[tokio::test]
async fn arrivees_simultanees_quantite_finale_juste() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    subscribe(&app, org, 1).await;

    // 4 invitations, puis 4 acceptations simultanées.
    let mut accepts = Vec::new();
    for i in 0..4 {
        let user = app.register(&format!("u{i}")).await;
        let token = app.invite(org, &owner, &user.email, "member").await;
        accepts.push((user, token));
    }
    let results = futures::future::join_all(accepts.iter().map(|(user, token)| {
        app.post(
            "/api/invitations/accept",
            &user.token,
            json!({ "token": token }),
        )
    }))
    .await;
    assert!(
        results.iter().all(|(s, _)| *s == StatusCode::OK),
        "{results:?}"
    );

    // Quelle que soit l'ordre des synchros, la dernière quantité envoyée = 5.
    for _ in 0..100 {
        if app.billing.seat_updates().last().map(|(_, n)| *n) == Some(5) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await; // laisse finir d'éventuelles synchros en retard
    assert_eq!(
        app.billing.seat_updates().last(),
        Some(&("si_1".to_string(), 5))
    );
    assert_eq!(billing(&app, org, &owner).await["subscription"]["seats"], 5);
}
