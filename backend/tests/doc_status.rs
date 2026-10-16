//! ING-03 : statut des documents poussé par WebSocket (`doc_status`).

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::Value;
use uuid::Uuid;

use common::{TestApp, WsSocket, ws_connect, ws_recv, ws_send, ws_try_recv};
use lodestone_backend::{
    repositories::documents::{self, NewDocument},
    services::ingestion,
    ws::protocol::ServerMsg,
};
use lodestone_core::document_status::DocumentStatus;
use serde_json::json;

fn long_text() -> String {
    (0..20)
        .map(|i| {
            let words: Vec<String> = (0..60).map(|w| format!("mot{i}x{w}")).collect();
            format!("Paragraphe {i}. {}", words.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Garantit que la connexion est prête (abonnement actif) avant de continuer.
async fn ready(socket: &mut WsSocket) {
    ws_send(socket, json!({ "type": "ping" })).await;
    assert_eq!(ws_recv(socket).await, json!({ "type": "pong" }));
}

/// Statuts reçus pour `doc`, jusqu'à `ready` ou `failed` inclus.
async fn statuses_until_done(socket: &mut WsSocket, doc: Uuid) -> Vec<String> {
    let mut statuses = Vec::new();
    loop {
        let msg = ws_recv(socket).await;
        assert_eq!(msg["type"], "doc_status", "{msg}");
        if msg["document_id"] != doc.to_string() {
            continue;
        }
        let status = msg["status"].as_str().expect("status").to_string();
        let done = status == "ready" || status == "failed";
        statuses.push(status);
        if done {
            return statuses;
        }
    }
}

async fn upload(app: &TestApp, org: Uuid, token: &str, name: &str, content: &[u8]) -> Uuid {
    let (status, body) = app.upload(org, token, name, content, None).await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    body["id"].as_str().expect("id").parse().expect("uuid")
}

#[tokio::test]
async fn un_membre_suit_toutes_les_transitions() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;

    let mut member_ws = ws_connect(addr, &member.token).await;
    let mut owner_ws = ws_connect(addr, &owner.token).await;
    ready(&mut member_ws).await;
    ready(&mut owner_ws).await;

    let doc = upload(&app, org, &owner.token, "rh.txt", long_text().as_bytes()).await;

    let expected = vec!["pending", "processing", "ready"];
    assert_eq!(statuses_until_done(&mut member_ws, doc).await, expected);
    assert_eq!(
        statuses_until_done(&mut owner_ws, doc).await,
        expected,
        "l'auteur aussi"
    );
}

#[tokio::test]
async fn echec_d_ingestion_remonte_failed() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let mut socket = ws_connect(addr, &owner.token).await;
    ready(&mut socket).await;

    let doc = upload(&app, org, &owner.token, "casse.pdf", b"pas un pdf").await;
    assert_eq!(
        statuses_until_done(&mut socket, doc).await,
        vec!["pending", "processing", "failed"]
    );
}

#[tokio::test]
async fn rien_pour_les_autres_orgs() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&owner).await;
    app.create_org(&outsider).await; // membre d'une autre org

    let mut socket = ws_connect(addr, &outsider.token).await;
    ready(&mut socket).await;

    let doc = upload(
        &app,
        org,
        &owner.token,
        "secret.txt",
        long_text().as_bytes(),
    )
    .await;
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");

    let leaked = ws_try_recv(&mut socket, Duration::from_millis(300)).await;
    assert_eq!(leaked, None, "aucun statut d'une autre org");
}

#[tokio::test]
async fn un_membre_retire_ne_recoit_plus_rien() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;

    // Connecté pendant qu'il était membre…
    let mut socket = ws_connect(addr, &member.token).await;
    ready(&mut socket).await;

    // … puis retiré : l'appartenance est revérifiée à chaque événement.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/orgs/{org}/members/{}", member.id),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let doc = upload(&app, org, &owner.token, "apres.txt", long_text().as_bytes()).await;
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    assert_eq!(
        ws_try_recv(&mut socket, Duration::from_millis(300)).await,
        None
    );
}

#[tokio::test]
async fn membre_de_plusieurs_orgs() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let org_a = app.create_org(&alice).await;
    let org_b = app.create_org(&bob).await;
    app.add_member(org_b, &bob, &alice, "member").await;

    let mut socket = ws_connect(addr, &alice.token).await;
    ready(&mut socket).await;

    let doc_a = upload(&app, org_a, &alice.token, "a.txt", b"Document A.").await;
    assert_eq!(
        statuses_until_done(&mut socket, doc_a)
            .await
            .last()
            .map(String::as_str),
        Some("ready")
    );
    let doc_b = upload(&app, org_b, &bob.token, "b.txt", b"Document B.").await;
    assert_eq!(
        statuses_until_done(&mut socket, doc_b)
            .await
            .last()
            .map(String::as_str),
        Some("ready")
    );
}

#[tokio::test]
async fn le_service_publie_chaque_transition_ecrite() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let doc = documents::create(
        &app.db,
        NewDocument {
            org_id: org,
            title: "a.txt".into(),
            source_type: "txt".into(),
            uploaded_by: owner.id,
        },
    )
    .await
    .expect("document");

    let mut rx = app.events.subscribe();
    ingestion::ingest(
        &app.db,
        &app.events,
        app.llm.as_ref(),
        doc.id,
        b"Bonjour.".to_vec(),
    )
    .await
    .expect("ingestion");

    let mut received = Vec::new();
    while let Ok(event) = rx.try_recv() {
        assert_eq!(event.org_id, org);
        received.push(event.message);
    }
    let status = |status| ServerMsg::DocStatus {
        document_id: doc.id,
        status,
    };
    assert_eq!(
        received,
        vec![
            status(DocumentStatus::Processing),
            status(DocumentStatus::Ready)
        ]
    );

    // Document supprimé avant le lancement de l'ingestion : rien n'est publié
    // pour un document qui n'existe plus.
    let gone = documents::create(
        &app.db,
        NewDocument {
            org_id: org,
            title: "b.txt".into(),
            source_type: "txt".into(),
            uploaded_by: owner.id,
        },
    )
    .await
    .expect("document");
    documents::delete(&app.db, gone.id)
        .await
        .expect("suppression");
    let mut rx = app.events.subscribe();
    let _ = ingestion::ingest(
        &app.db,
        &app.events,
        app.llm.as_ref(),
        gone.id,
        b"x".to_vec(),
    )
    .await;
    let leftovers: Vec<Value> = std::iter::from_fn(|| rx.try_recv().ok())
        .map(|e| serde_json::to_value(&e.message).expect("json"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
