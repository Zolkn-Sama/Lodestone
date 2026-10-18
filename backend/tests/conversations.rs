//! CHAT-01 : créer une conversation (+ schéma messages / message_sources).

mod common;

use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, Statement, Value as DbValue};
use serde_json::{Value, json};
use uuid::Uuid;

use common::TestApp;

fn conv_id(body: &Value) -> Uuid {
    body["id"].as_str().expect("id").parse().expect("uuid")
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

async fn exec(app: &TestApp, sql: &str, values: Vec<DbValue>) {
    app.db
        .execute(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            sql,
            values,
        ))
        .await
        .expect("exécution");
}

#[tokio::test]
async fn tout_membre_peut_creer_une_conversation() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let member = app.register("member").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;
    app.add_member(org, &owner, &member, "member").await;
    let uri = format!("/api/orgs/{org}/conversations");

    for (user, expected) in [
        (&owner, StatusCode::CREATED),
        (&admin, StatusCode::CREATED),
        (&member, StatusCode::CREATED),
        (&outsider, StatusCode::NOT_FOUND),
    ] {
        let (status, _) = app.post(&uri, &user.token, json!({})).await;
        assert_eq!(status, expected, "{}", user.email);
    }
    let (status, _) = app.request(Method::POST, &uri, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Rattachée à l'org ET à son auteur.
    let (_, body) = app.post(&uri, &member.token, json!({})).await;
    let n = count(
        &app,
        "SELECT count(*) AS n FROM conversations WHERE id = $1 AND org_id = $2 AND user_id = $3",
        vec![conv_id(&body).into(), org.into(), member.id.into()],
    )
    .await;
    assert_eq!(n, 1);
}

#[tokio::test]
async fn titre_par_defaut_ou_fourni() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let uri = format!("/api/orgs/{org}/conversations");

    // Sans corps du tout.
    let (status, body) = app
        .request(Method::POST, &uri, Some(&owner.token), None)
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["title"], "Nouvelle conversation");
    assert!(body["created_at"].is_string());

    // Corps vide.
    let (_, body) = app.post(&uri, &owner.token, json!({})).await;
    assert_eq!(body["title"], "Nouvelle conversation");

    // Titre fourni, espaces retirés.
    let (status, body) = app
        .post(&uri, &owner.token, json!({ "title": "  Congés 2026  " }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["title"], "Congés 2026");
}

#[tokio::test]
async fn titres_invalides() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let uri = format!("/api/orgs/{org}/conversations");

    for title in [json!(""), json!("   "), json!("x".repeat(201)), json!(42)] {
        let (status, body) = app
            .post(&uri, &owner.token, json!({ "title": title }))
            .await;
        assert!(
            status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
            "titre {title} : {status} {body}"
        );
    }
    let n = count(&app, "SELECT count(*) AS n FROM conversations", vec![]).await;
    assert_eq!(n, 0, "aucune conversation créée");
}

#[tokio::test]
async fn suppressions_en_cascade() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;
    let uri = format!("/api/orgs/{org}/conversations");

    let (_, body) = app.post(&uri, &member.token, json!({})).await;
    let by_member = conv_id(&body);
    let (_, body) = app.post(&uri, &owner.token, json!({})).await;
    let by_owner = conv_id(&body);

    // Compte supprimé -> ses conversations aussi, pas celles des autres.
    exec(
        &app,
        "DELETE FROM users WHERE id = $1",
        vec![member.id.into()],
    )
    .await;
    let n = count(
        &app,
        "SELECT count(*) AS n FROM conversations WHERE id IN ($1, $2)",
        vec![by_member.into(), by_owner.into()],
    )
    .await;
    assert_eq!(n, 1);

    // Org supprimée -> toutes ses conversations.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/orgs/{org}"),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM conversations", vec![]).await,
        0
    );
}

/// Le schéma de RAG-04 est déjà là : messages et sources suivent leur
/// conversation, et une source disparaît avec le fragment cité.
#[tokio::test]
async fn schema_messages_et_sources() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let (status, body) = app
        .upload(
            org,
            &owner.token,
            "rh.txt",
            b"Vingt-cinq jours de conges par an.",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let doc: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");

    let (_, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &owner.token,
            json!({}),
        )
        .await;
    let conv = conv_id(&body);

    let msg = Uuid::new_v4();
    exec(
        &app,
        "INSERT INTO messages (id, conversation_id, role, content) VALUES ($1, $2, 'assistant', 'Vingt-cinq jours.')",
        vec![msg.into(), conv.into()],
    )
    .await;
    exec(
        &app,
        "INSERT INTO message_sources (id, message_id, chunk_id, score)
         SELECT gen_random_uuid(), $1, id, 0.87 FROM document_chunks WHERE document_id = $2",
        vec![msg.into(), doc.into()],
    )
    .await;
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM message_sources", vec![]).await,
        1
    );

    // Document supprimé -> fragment supprimé -> source supprimée ; le message reste.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/documents/{doc}"),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM message_sources", vec![]).await,
        0
    );
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM messages", vec![]).await,
        1
    );

    // Conversation supprimée -> ses messages.
    exec(
        &app,
        "DELETE FROM conversations WHERE id = $1",
        vec![conv.into()],
    )
    .await;
    assert_eq!(
        count(&app, "SELECT count(*) AS n FROM messages", vec![]).await,
        0
    );
}
