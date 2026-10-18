//! BILL-02 : blocage au-delà des limites du plan, erreur 402 explicite.

mod common;

use axum::http::{Method, StatusCode};
use futures::future::join_all;
use sea_orm::{ConnectionTrait, Statement, Value as DbValue};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser, ws_connect, ws_recv, ws_send};
use lodestone_backend::repositories::{
    documents::{self, NewDocument},
    organizations,
};
use lodestone_core::plan::Plan;

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

fn assert_quota(status: StatusCode, body: &Value, resource: &str, limit: u32) {
    assert_eq!(status, StatusCode::PAYMENT_REQUIRED, "{body}");
    assert_eq!(body["code"], 402);
    assert_eq!(
        body["quota"],
        json!({ "resource": resource, "limit": limit, "plan": "free", "upgrade_to": "pro" })
    );
    let message = body["error"].as_str().expect("message");
    assert!(
        message.contains("plan Free") && message.contains("plan Pro"),
        "{message}"
    );
}

async fn invite(app: &TestApp, org: Uuid, owner: &TestUser, email: &str) -> (StatusCode, Value) {
    app.post(
        &format!("/api/orgs/{org}/invitations"),
        &owner.token,
        json!({ "email": email }),
    )
    .await
}

// ----- Membres -------------------------------------------------------------------

#[tokio::test]
async fn trois_sieges_invitations_comprises() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    // Owner + 2 invitations en attente = 3 sièges.
    assert_eq!(
        invite(&app, org, &owner, "a@test.io").await.0,
        StatusCode::CREATED
    );
    assert_eq!(
        invite(&app, org, &owner, "b@test.io").await.0,
        StatusCode::CREATED
    );
    let (status, body) = invite(&app, org, &owner, "c@test.io").await;
    assert_quota(status, &body, "members", 3);

    // Ré-inviter une personne déjà invitée ne consomme pas de siège en plus.
    assert_eq!(
        invite(&app, org, &owner, "a@test.io").await.0,
        StatusCode::CREATED
    );

    // Une invitation expirée libère son siège.
    exec(
        &app,
        "UPDATE invitations SET expires_at = now() - interval '1 minute' WHERE email = 'b@test.io'",
        vec![],
    )
    .await;
    assert_eq!(
        invite(&app, org, &owner, "c@test.io").await.0,
        StatusCode::CREATED
    );

    // En Pro, plus de limite.
    organizations::set_plan(&app.db, org, Plan::Pro)
        .await
        .expect("plan");
    assert_eq!(
        invite(&app, org, &owner, "d@test.io").await.0,
        StatusCode::CREATED
    );
}

#[tokio::test]
async fn acceptation_refusee_si_l_org_est_pleine() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let late = app.register("late").await;
    let org = app.create_org(&owner).await;

    // En Pro : 3 membres, puis une invitation de plus.
    organizations::set_plan(&app.db, org, Plan::Pro)
        .await
        .expect("plan");
    for name in ["m1", "m2"] {
        let user = app.register(name).await;
        app.add_member(org, &owner, &user, "member").await;
    }
    let token = app.invite(org, &owner, &late.email, "member").await;

    // Retour en Free avant l'acceptation : l'org est déjà pleine.
    organizations::set_plan(&app.db, org, Plan::Free)
        .await
        .expect("plan");
    let (status, body) = app
        .post(
            "/api/invitations/accept",
            &late.token,
            json!({ "token": token }),
        )
        .await;
    assert_quota(status, &body, "members", 3);
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM memberships WHERE org_id = $1",
            vec![org.into()]
        )
        .await,
        3
    );

    // L'invitation reste utilisable si l'org repasse en Pro.
    organizations::set_plan(&app.db, org, Plan::Pro)
        .await
        .expect("plan");
    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &late.token,
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn invitations_simultanees_sans_depassement() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let emails: Vec<String> = (0..10).map(|i| format!("u{i}@test.io")).collect();
    let results = join_all(emails.iter().map(|e| invite(&app, org, &owner, e))).await;
    let created = results
        .iter()
        .filter(|(s, _)| *s == StatusCode::CREATED)
        .count();
    let refused = results
        .iter()
        .filter(|(s, _)| *s == StatusCode::PAYMENT_REQUIRED)
        .count();
    assert_eq!((created, refused), (2, 8), "{results:?}");
}

// ----- Documents -----------------------------------------------------------------

#[tokio::test]
async fn cinquante_documents_maximum() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let mut last = Uuid::nil();
    for i in 0..50 {
        last = documents::create(
            &app.db,
            NewDocument {
                org_id: org,
                title: format!("doc{i}.txt"),
                source_type: "txt".into(),
                uploaded_by: owner.id,
            },
        )
        .await
        .expect("document")
        .id;
    }

    let (status, body) = app
        .upload(org, &owner.token, "51.txt", b"Un de trop.", None)
        .await;
    assert_quota(status, &body, "documents", 50);
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM documents WHERE org_id = $1",
            vec![org.into()]
        )
        .await,
        50,
        "rien n'a été créé"
    );

    // Supprimer un document libère une place.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/documents/{last}"),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app
        .upload(org, &owner.token, "51.txt", b"Maintenant oui.", None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);

    organizations::set_plan(&app.db, org, Plan::Pro)
        .await
        .expect("plan");
    let (status, _) = app
        .upload(org, &owner.token, "52.txt", b"Pro : illimite.", None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
}

// ----- Questions -----------------------------------------------------------------

#[tokio::test]
async fn cent_questions_par_mois() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let (_, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &owner.token,
            json!({}),
        )
        .await;
    let conv = body["id"].as_str().expect("id").to_string();
    let mut socket = ws_connect(addr, &owner.token).await;
    let ask = json!({ "type": "ask", "conversation_id": conv, "question": "Une question ?" });

    // 100 questions déjà posées ce mois-ci (et 100 le mois dernier, sans effet).
    exec(
        &app,
        "INSERT INTO org_usage (org_id, period, questions) VALUES
            ($1, date_trunc('month', now() AT TIME ZONE 'UTC')::date, 100),
            ($1, (date_trunc('month', now() AT TIME ZONE 'UTC') - interval '1 month')::date, 100)",
        vec![org.into()],
    )
    .await;

    ws_send(&mut socket, ask.clone()).await;
    let reply = ws_recv(&mut socket).await;
    assert_eq!(reply["type"], "error", "{reply}");
    let message = reply["message"].as_str().expect("message");
    assert!(
        message.contains("100 questions par mois") && message.contains("plan Pro"),
        "{message}"
    );
    assert!(app.llm.chat_requests().is_empty(), "aucun appel au modèle");
    let (_, usage) = app
        .get(&format!("/api/orgs/{org}/usage"), &owner.token)
        .await;
    assert_eq!(usage["questions"]["used"], 100, "rien n'est compté en plus");

    // En Pro, la question passe.
    organizations::set_plan(&app.db, org, Plan::Pro)
        .await
        .expect("plan");
    ws_send(&mut socket, ask).await;
    let last = loop {
        let msg = ws_recv(&mut socket).await;
        if msg["type"] == "done" || msg["type"] == "error" {
            break msg;
        }
    };
    assert_eq!(last["type"], "done", "{last}");
}

// ----- Organisations -------------------------------------------------------------

#[tokio::test]
async fn une_seule_org_free_par_utilisateur() {
    let app = TestApp::spawn().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;

    let first = app.create_org(&alice).await;
    let (status, body) = app
        .post("/api/orgs", &alice.token, json!({ "name": "Deuxième" }))
        .await;
    assert_quota(status, &body, "owned_orgs", 1);

    // Être simple membre de l'org Free de quelqu'un d'autre ne compte pas.
    let bob_org = app.create_org(&bob).await;
    app.add_member(bob_org, &bob, &alice, "member").await;
    let (status, _) = app
        .post("/api/orgs", &alice.token, json!({ "name": "Encore" }))
        .await;
    assert_eq!(
        status,
        StatusCode::PAYMENT_REQUIRED,
        "toujours 1 org Free possédée"
    );

    // Une fois la première passée en Pro, Alice peut créer une org Free.
    organizations::set_plan(&app.db, first, Plan::Pro)
        .await
        .expect("plan");
    let (status, _) = app
        .post("/api/orgs", &alice.token, json!({ "name": "Deuxième" }))
        .await;
    assert_eq!(status, StatusCode::CREATED);
}

#[tokio::test]
async fn creations_d_org_simultanees_sans_depassement() {
    let app = TestApp::spawn().await;
    let alice = app.register("alice").await;
    let results = join_all((0..5).map(|i| {
        app.post(
            "/api/orgs",
            &alice.token,
            json!({ "name": format!("Org {i}") }),
        )
    }))
    .await;
    let created = results
        .iter()
        .filter(|(s, _)| *s == StatusCode::CREATED)
        .count();
    assert_eq!(created, 1, "{results:?}");
}
