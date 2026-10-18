//! Durcissement du MVP : CORS, route de debug retirée, /me avec les orgs,
//! titre automatique des conversations, reprise après redémarrage.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser, ws_connect, ws_recv, ws_send};
use lodestone_backend::{
    repositories::documents::{self, NewDocument},
    services::ingestion,
};
use lodestone_core::document_status::DocumentStatus;

// ----- CORS -----------------------------------------------------------------------

async fn preflight(app: &TestApp, origin: &str) -> axum::response::Response {
    app.send(
        Request::builder()
            .method(Method::OPTIONS)
            .uri("/api/orgs")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
            .header(
                header::ACCESS_CONTROL_REQUEST_HEADERS,
                "authorization,content-type",
            )
            .body(Body::empty())
            .expect("requête"),
    )
    .await
}

#[tokio::test]
async fn cors_limite_aux_origines_configurees() {
    let app = TestApp::spawn().await;

    // Origine par défaut (front Next.js en développement) : autorisée.
    let res = preflight(&app, "http://localhost:3000").await;
    let headers = res.headers();
    assert_eq!(
        headers
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("http://localhost:3000")
    );
    let methods = headers
        .get(header::ACCESS_CONTROL_ALLOW_METHODS)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    for m in ["GET", "POST", "PATCH", "DELETE"] {
        assert!(methods.contains(m), "{methods}");
    }
    let allowed_headers = headers
        .get(header::ACCESS_CONTROL_ALLOW_HEADERS)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_lowercase();
    assert!(allowed_headers.contains("authorization") && allowed_headers.contains("content-type"));

    // Origine inconnue : aucun en-tête d'autorisation, le navigateur bloquera.
    let res = preflight(&app, "https://site-malveillant.example").await;
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );

    // Requête simple depuis une origine inconnue : la réponse ne l'autorise pas non plus.
    let res = app
        .send(
            Request::builder()
                .uri("/health")
                .header(header::ORIGIN, "https://site-malveillant.example")
                .body(Body::empty())
                .expect("requête"),
        )
        .await;
    assert!(
        res.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none()
    );
}

#[tokio::test]
async fn route_de_debug_retiree() {
    let app = TestApp::spawn().await;
    let (status, _) = app.request(Method::GET, "/boom", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ----- /api/auth/me ----------------------------------------------------------------

#[tokio::test]
async fn me_renvoie_le_profil_et_les_orgs() {
    let app = TestApp::spawn().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;

    let (status, me) = app.get("/api/auth/me", &alice.token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], alice.id.to_string());
    assert_eq!(me["email"], alice.email.as_str());
    assert_eq!(me["organizations"], json!([]));
    assert!(me.get("password_hash").is_none());

    let own = app.create_org(&alice).await;
    let other = app.create_org(&bob).await;
    app.add_member(other, &bob, &alice, "member").await;

    let (_, me) = app.get("/api/auth/me", &alice.token).await;
    let orgs: Vec<(String, String)> = me["organizations"]
        .as_array()
        .expect("organizations")
        .iter()
        .map(|o| {
            (
                o["id"].as_str().expect("id").to_string(),
                o["role"].as_str().expect("role").to_string(),
            )
        })
        .collect();
    assert_eq!(
        orgs,
        vec![
            (own.to_string(), "owner".to_string()),
            (other.to_string(), "member".to_string())
        ]
    );

    let (status, _) = app.request(Method::GET, "/api/auth/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ----- Titre automatique ------------------------------------------------------------

async fn ask_until_done(socket: &mut common::WsSocket, conversation: Uuid, question: &str) {
    ws_send(
        socket,
        json!({ "type": "ask", "conversation_id": conversation, "question": question }),
    )
    .await;
    loop {
        let msg = ws_recv(socket).await;
        assert_ne!(msg["type"], "error", "{msg}");
        if msg["type"] == "done" {
            return;
        }
    }
}

async fn conversation(app: &TestApp, org: Uuid, user: &TestUser, body: Value) -> Uuid {
    let (status, created) = app
        .post(&format!("/api/orgs/{org}/conversations"), &user.token, body)
        .await;
    assert_eq!(status, StatusCode::CREATED);
    created["id"].as_str().expect("id").parse().expect("uuid")
}

async fn title(app: &TestApp, user: &TestUser, conv: Uuid) -> String {
    let (_, detail) = app
        .get(&format!("/api/conversations/{conv}"), &user.token)
        .await;
    detail["title"].as_str().expect("title").to_string()
}

#[tokio::test]
async fn la_premiere_question_devient_le_titre() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let mut socket = ws_connect(addr, &owner.token).await;

    let auto = conversation(&app, org, &owner, json!({})).await;
    let named = conversation(&app, org, &owner, json!({ "title": "Mon dossier RH" })).await;
    assert_eq!(title(&app, &owner, auto).await, "Nouvelle conversation");

    ask_until_done(
        &mut socket,
        auto,
        "  Combien de jours\nde congés par an ?  ",
    )
    .await;
    assert_eq!(
        title(&app, &owner, auto).await,
        "Combien de jours de congés par an ?"
    );

    // La 2e question ne change plus le titre.
    ask_until_done(&mut socket, auto, "Et pour les cadres ?").await;
    assert_eq!(
        title(&app, &owner, auto).await,
        "Combien de jours de congés par an ?"
    );

    // Un titre choisi n'est jamais écrasé.
    ask_until_done(&mut socket, named, "Quelle mutuelle ?").await;
    assert_eq!(title(&app, &owner, named).await, "Mon dossier RH");

    // Question longue : titre raccourci.
    let long = conversation(&app, org, &owner, json!({})).await;
    let question = "Quelle est la procédure exacte pour demander des congés payés pendant la période estivale ?";
    ask_until_done(&mut socket, long, question).await;
    let t = title(&app, &owner, long).await;
    assert!(t.ends_with('…') && t.chars().count() <= 61, "{t}");
}

// ----- Reprise après redémarrage ----------------------------------------------------

#[tokio::test]
async fn documents_interrompus_passent_en_failed() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let mut ids = Vec::new();
    for status in [
        DocumentStatus::Pending,
        DocumentStatus::Processing,
        DocumentStatus::Ready,
        DocumentStatus::Failed,
    ] {
        let doc = documents::create(
            &app.db,
            NewDocument {
                org_id: org,
                title: format!("{}.txt", status.as_str()),
                source_type: "txt".into(),
                uploaded_by: owner.id,
            },
        )
        .await
        .expect("document");
        documents::set_status(&app.db, doc.id, status)
            .await
            .expect("statut");
        ids.push(doc.id);
    }

    assert_eq!(
        ingestion::recover_interrupted(&app.db)
            .await
            .expect("reprise"),
        2
    );

    let mut statuses = Vec::new();
    for id in ids {
        statuses.push(
            documents::find_by_id(&app.db, id)
                .await
                .expect("requête")
                .expect("document")
                .status,
        );
    }
    assert_eq!(statuses, vec!["failed", "failed", "ready", "failed"]);

    // Rien d'autre à reprendre ensuite.
    assert_eq!(
        ingestion::recover_interrupted(&app.db)
            .await
            .expect("reprise"),
        0
    );
}
