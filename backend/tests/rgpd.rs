//! RGPD : suppression du compte (effacement) et export des données (portabilité).

mod common;

use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, Statement, Value as DbValue};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser};

const PASSWORD: &str = "password123";

async fn delete_me(app: &TestApp, user: &TestUser, password: &str) -> (StatusCode, Value) {
    app.request(
        Method::DELETE,
        "/api/auth/me",
        Some(&user.token),
        Some(json!({ "password": password })),
    )
    .await
}

async fn count(app: &TestApp, sql: &str, values: Vec<DbValue>) -> i64 {
    app.db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            sql,
            values,
        ))
        .await
        .expect("requête")
        .expect("ligne")
        .try_get("", "n")
        .expect("n")
}

async fn upload_ready(app: &TestApp, org: Uuid, user: &TestUser, name: &str) -> Uuid {
    let (status, body) = app
        .upload(org, &user.token, name, b"Vingt-cinq jours de conges.", None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let doc = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    doc
}

#[tokio::test]
async fn suppression_garde_les_donnees_de_l_org() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let leaver = app.register("leaver").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &leaver, "admin").await;

    // Le partant a apporté un document, envoyé une invitation, discuté.
    let doc = upload_ready(&app, org, &leaver, "procedure.txt").await;
    app.invite(org, &leaver, "recrue@test.io", "member").await;
    let (_, conv) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &leaver.token,
            json!({}),
        )
        .await;
    let conv: Uuid = conv["id"].as_str().expect("id").parse().expect("uuid");
    let (_, tokens) = app
        .request(
            Method::POST,
            "/api/auth/login",
            None,
            Some(json!({ "email": leaver.email, "password": PASSWORD })),
        )
        .await;
    let refresh = tokens["refresh_token"]
        .as_str()
        .expect("refresh")
        .to_string();

    let (status, body) = delete_me(&app, &leaver, PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Le compte et tout ce qui lui est personnel ont disparu.
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM users WHERE id = $1",
            vec![leaver.id.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM conversations WHERE id = $1",
            vec![conv.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM refresh_tokens WHERE user_id = $1",
            vec![leaver.id.into()]
        )
        .await,
        0
    );
    let (status, _) = app
        .request(
            Method::POST,
            "/api/auth/refresh",
            None,
            Some(json!({ "refresh_token": refresh })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = app.get("/api/auth/me", &leaver.token).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "jeton encore valide mais compte supprimé"
    );

    // Le document et l'invitation restent à l'org, sans auteur.
    let (status, document) = app
        .get(&format!("/api/documents/{doc}"), &owner.token)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(document["status"], "ready");
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM documents WHERE id = $1 AND uploaded_by IS NULL",
            vec![doc.into()]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM document_chunks WHERE document_id = $1",
            vec![doc.into()]
        )
        .await,
        1,
        "les fragments restent cherchables"
    );
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM invitations WHERE email = 'recrue@test.io' AND invited_by IS NULL", vec![]).await,
        1
    );
    let (_, members) = app
        .get(&format!("/api/orgs/{org}/members"), &owner.token)
        .await;
    assert_eq!(members.as_array().map(Vec::len), Some(1));

    // L'adresse peut resservir.
    let again = app
        .request(
            Method::POST,
            "/api/auth/register",
            None,
            Some(json!({ "email": leaver.email, "password": PASSWORD })),
        )
        .await;
    assert_eq!(again.0, StatusCode::CREATED);
}

#[tokio::test]
async fn mot_de_passe_requis() {
    let app = TestApp::spawn().await;
    let user = app.register("user").await;

    let (status, _) = delete_me(&app, &user, "mauvais").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = app
        .request(
            Method::DELETE,
            "/api/auth/me",
            Some(&user.token),
            Some(json!({})),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = app
        .request(
            Method::DELETE,
            "/api/auth/me",
            None,
            Some(json!({ "password": PASSWORD })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Essais limités, comme à la connexion.
    for _ in 0..4 {
        delete_me(&app, &user, "mauvais").await;
    }
    let (status, _) = delete_me(&app, &user, PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM users WHERE id = $1",
            vec![user.id.into()]
        )
        .await,
        1
    );
}

#[tokio::test]
async fn seul_membre_l_org_part_avec_lui() {
    let app = TestApp::spawn().await;
    let solo = app.register("solo").await;
    let org = app.create_org(&solo).await;
    let doc = upload_ready(&app, org, &solo, "perso.txt").await;

    let (status, _) = delete_me(&app, &solo, PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM organizations WHERE id = $1",
            vec![org.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM documents WHERE id = $1",
            vec![doc.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM document_chunks", vec![]).await,
        0
    );
}

#[tokio::test]
async fn seul_owner_avec_des_membres_refuse() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;

    let (status, body) = delete_me(&app, &owner, PASSWORD).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let message = body["error"].as_str().expect("message");
    assert!(message.contains("seul owner de Acme"), "{message}");
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM users WHERE id = $1",
            vec![owner.id.into()]
        )
        .await,
        1
    );

    // Une fois un autre owner nommé, il peut partir.
    let (status, _) = app
        .request(
            Method::PATCH,
            &format!("/api/orgs/{org}/members/{}", member.id),
            Some(&owner.token),
            Some(json!({ "role": "owner" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = delete_me(&app, &owner, PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, org_after) = app.get(&format!("/api/orgs/{org}"), &member.token).await;
    assert_eq!(
        org_after["role"], "owner",
        "l'org continue avec son nouvel owner"
    );
}

#[tokio::test]
async fn abonnement_actif_refuse() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let (status, _) = app
        .send_webhook(&json!({
            "id": "evt_1", "type": "checkout.session.completed", "created": 1_760_000_000,
            "data": { "object": { "id": "cs_1", "subscription": "sub_1", "metadata": { "org_id": org } } }
        }))
        .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = delete_me(&app, &owner, PASSWORD).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("résiliez")),
        "{body}"
    );
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM organizations WHERE id = $1",
            vec![org.into()]
        )
        .await,
        1
    );
}

#[tokio::test]
async fn export_des_donnees() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let user = app.register("user").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &user, "admin").await;
    let doc = upload_ready(&app, org, &user, "mon-doc.txt").await;
    app.invite(org, &user, "ami@test.io", "member").await;

    // Une vraie question / réponse, pour avoir messages et sources.
    let (_, conv) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &user.token,
            json!({}),
        )
        .await;
    let conv = conv["id"].as_str().expect("id").to_string();
    let mut socket = common::ws_connect(addr, &user.token).await;
    common::ws_send(
        &mut socket,
        json!({ "type": "ask", "conversation_id": conv, "question": "Combien de jours de conges ?" }),
    )
    .await;
    loop {
        let msg = common::ws_recv(&mut socket).await;
        if msg["type"] == "done" {
            break;
        }
    }

    let res = app
        .send(
            axum::http::Request::builder()
                .uri("/api/auth/me/export")
                .header("authorization", format!("Bearer {}", user.token))
                .body(axum::body::Body::empty())
                .expect("requête"),
        )
        .await;
    assert_eq!(res.status(), StatusCode::OK);
    let disposition = res
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(disposition.starts_with("attachment"), "{disposition}");
    let bytes = http_body_util::BodyExt::collect(res.into_body())
        .await
        .expect("corps")
        .to_bytes();
    let text = String::from_utf8(bytes.to_vec()).expect("utf-8");
    let export: Value = serde_json::from_str(&text).expect("JSON");

    assert_eq!(export["profile"]["email"], user.email.as_str());
    assert_eq!(export["organizations"][0]["role"], "admin");
    let conversation = &export["conversations"][0];
    assert_eq!(conversation["org_id"], org.to_string());
    assert_eq!(
        conversation["messages"][0]["content"],
        "Combien de jours de conges ?"
    );
    assert_eq!(conversation["messages"][1]["role"], "assistant");
    assert_eq!(
        conversation["messages"][1]["sources"][0]["title"],
        "mon-doc.txt"
    );
    assert_eq!(export["documents_uploaded"][0]["id"], doc.to_string());
    assert_eq!(export["invitations_sent"][0]["email"], "ami@test.io");

    // Aucun secret : ni mot de passe, ni jeton.
    for secret in ["password", "token_hash", "argon2"] {
        assert!(
            !text.contains(secret),
            "« {secret} » ne doit pas apparaître"
        );
    }

    // Les données des autres n'y sont pas.
    assert!(
        !text.contains(&owner.email),
        "l'adresse d'un autre membre ne doit pas figurer dans l'export"
    );
    let (status, _) = app
        .request(Method::GET, "/api/auth/me/export", None, None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
