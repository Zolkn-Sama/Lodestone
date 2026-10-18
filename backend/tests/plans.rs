//! BILL-01 : plans, limites et consommation (`GET /api/orgs/{id}/usage`).

mod common;

use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser, WsSocket, ws_connect, ws_recv, ws_send};
use lodestone_backend::{repositories::organizations, services::ingestion::EMBEDDING_DIMENSIONS};
use lodestone_core::plan::Plan;
use lodestone_llm::{ChatFailure, MockLlmClient};

async fn usage(app: &TestApp, org: Uuid, user: &TestUser) -> Value {
    let (status, body) = app
        .get(&format!("/api/orgs/{org}/usage"), &user.token)
        .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body
}

async fn conversation(app: &TestApp, org: Uuid, user: &TestUser) -> Uuid {
    let (_, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &user.token,
            json!({}),
        )
        .await;
    body["id"].as_str().expect("id").parse().expect("uuid")
}

/// Pose une question et renvoie le type du dernier message (`done` ou `error`).
async fn ask(socket: &mut WsSocket, conversation: Uuid, question: &str) -> String {
    ws_send(
        socket,
        json!({ "type": "ask", "conversation_id": conversation, "question": question }),
    )
    .await;
    loop {
        let msg = ws_recv(socket).await;
        if msg["type"] == "done" || msg["type"] == "error" {
            return msg["type"].as_str().unwrap_or_default().to_string();
        }
    }
}

#[tokio::test]
async fn une_nouvelle_org_est_free() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;

    let (status, created) = app
        .post("/api/orgs", &owner.token, json!({ "name": "Acme" }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["plan"], "free");
    let org = created["id"].as_str().expect("id");

    let (_, detail) = app.get(&format!("/api/orgs/{org}"), &owner.token).await;
    assert_eq!(detail["plan"], "free");
    let (_, list) = app.get("/api/orgs", &owner.token).await;
    assert_eq!(list[0]["plan"], "free");
    let (_, me) = app.get("/api/auth/me", &owner.token).await;
    assert_eq!(me["organizations"][0]["plan"], "free");
}

#[tokio::test]
async fn consommation_face_aux_limites_free() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;

    let body = usage(&app, org, &owner).await;
    let period = chrono::Utc::now().format("%Y-%m").to_string();
    assert_eq!(
        body,
        json!({
            "plan": "free",
            "limits": {
                "max_members": 3,
                "max_documents": 50,
                "max_questions_per_month": 100,
                "max_owned_orgs": 1
            },
            "period": period,
            "members": { "used": 1, "limit": 3 },
            "documents": { "used": 0, "limit": 50 },
            "questions": { "used": 0, "limit": 100 }
        })
    );

    app.add_member(org, &owner, &member, "member").await;
    let (_, doc) = app
        .upload(
            org,
            &owner.token,
            "rh.txt",
            b"Vingt-cinq jours de conges.",
            None,
        )
        .await;
    let doc: Uuid = doc["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");

    // Deux questions répondues ; une question refusée (vide) ne compte pas.
    let conv = conversation(&app, org, &member).await;
    let mut socket = ws_connect(addr, &member.token).await;
    assert_eq!(ask(&mut socket, conv, "Combien de jours ?").await, "done");
    assert_eq!(ask(&mut socket, conv, "Et les RTT ?").await, "done");
    assert_eq!(ask(&mut socket, conv, "   ").await, "error");

    // Visible par tout membre.
    let body = usage(&app, org, &member).await;
    assert_eq!(body["members"], json!({ "used": 2, "limit": 3 }));
    assert_eq!(body["documents"], json!({ "used": 1, "limit": 50 }));
    assert_eq!(body["questions"], json!({ "used": 2, "limit": 100 }));

    // Supprimer la conversation ne « rend » pas de questions.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/conversations/{conv}"),
            Some(&member.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(usage(&app, org, &owner).await["questions"]["used"], 2);
}

#[tokio::test]
async fn une_question_qui_echoue_ne_compte_pas() {
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS).with_chat_failure(ChatFailure::MidStream);
    let app = TestApp::spawn_with_llm(llm).await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let (_, doc) = app
        .upload(
            org,
            &owner.token,
            "rh.txt",
            b"Vingt-cinq jours de conges.",
            None,
        )
        .await;
    let doc: Uuid = doc["id"].as_str().expect("id").parse().expect("uuid");
    app.wait_for_ingestion(doc).await;

    let conv = conversation(&app, org, &owner).await;
    let mut socket = ws_connect(addr, &owner.token).await;
    assert_eq!(ask(&mut socket, conv, "Combien de jours ?").await, "error");
    assert_eq!(usage(&app, org, &owner).await["questions"]["used"], 0);
}

#[tokio::test]
async fn compteur_par_mois_et_par_org() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let org_a = app.create_org(&alice).await;
    let org_b = app.create_org(&bob).await;

    // Le mois dernier, l'org A avait posé 99 questions : remis à zéro ce mois-ci.
    app.db
        .execute(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "INSERT INTO org_usage (org_id, period, questions)
             VALUES ($1, (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '1 month')::date, 99)",
            [org_a.into()],
        ))
        .await
        .expect("mois précédent");

    // Bob pose une question dans l'org B : rien ne change pour A.
    let conv = conversation(&app, org_b, &bob).await;
    let mut socket = ws_connect(addr, &bob.token).await;
    assert_eq!(ask(&mut socket, conv, "Une question ?").await, "done");

    assert_eq!(usage(&app, org_a, &alice).await["questions"]["used"], 0);
    assert_eq!(usage(&app, org_b, &bob).await["questions"]["used"], 1);
}

#[tokio::test]
async fn plans_pro_et_sovereign_illimites() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    for plan in [Plan::Pro, Plan::Sovereign] {
        organizations::set_plan(&app.db, org, plan)
            .await
            .expect("plan");
        let body = usage(&app, org, &owner).await;
        assert_eq!(body["plan"], plan.as_str());
        for quota in ["members", "documents", "questions"] {
            assert_eq!(body[quota]["limit"], Value::Null, "{plan:?} {quota}");
        }
        let (_, detail) = app.get(&format!("/api/orgs/{org}"), &owner.token).await;
        assert_eq!(detail["plan"], plan.as_str());
    }
}

#[tokio::test]
async fn acces_a_la_consommation() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&owner).await;
    let uri = format!("/api/orgs/{org}/usage");

    let (status, _) = app.get(&uri, &outsider.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app.request(Method::GET, &uri, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn la_base_refuse_un_plan_inconnu() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let res = app
        .db
        .execute(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "UPDATE organizations SET plan = 'enterprise' WHERE id = $1",
            [org.into()],
        ))
        .await;
    assert!(res.is_err(), "contrainte CHECK attendue");
    assert_eq!(usage(&app, org, &owner).await["plan"], "free");
}
