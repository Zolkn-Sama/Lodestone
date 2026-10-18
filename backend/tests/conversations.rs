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

// ----- CHAT-02 / CHAT-03 / CHAT-04 ------------------------------------------------

/// Ajoute un message horodaté à `minutes` minutes de l'instant présent (négatif = passé).
async fn add_message(
    app: &TestApp,
    conversation: Uuid,
    role: &str,
    content: &str,
    minutes: i32,
) -> Uuid {
    let id = Uuid::new_v4();
    exec(
        app,
        "INSERT INTO messages (id, conversation_id, role, content, created_at)
         VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))",
        vec![
            id.into(),
            conversation.into(),
            role.into(),
            content.into(),
            minutes.into(),
        ],
    )
    .await;
    id
}

async fn create_conversation(
    app: &TestApp,
    org: Uuid,
    user: &common::TestUser,
    title: &str,
) -> Uuid {
    let (status, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &user.token,
            json!({ "title": title }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    conv_id(&body)
}

fn titles(list: &Value) -> Vec<&str> {
    list.as_array()
        .expect("liste")
        .iter()
        .filter_map(|c| c["title"].as_str())
        .collect()
}

#[tokio::test]
async fn historique_personnel_trie_par_activite() {
    let app = TestApp::spawn().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&alice).await;
    app.add_member(org, &alice, &bob, "member").await;
    let other_org = app.create_org(&alice).await;
    let uri = format!("/api/orgs/{org}/conversations");

    let (status, list) = app.get(&uri, &alice.token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list, json!([]));

    let ancienne = create_conversation(&app, org, &alice, "Ancienne").await;
    create_conversation(&app, org, &alice, "Récente").await;
    create_conversation(&app, org, &bob, "De Bob").await; // pas dans l'historique d'Alice
    create_conversation(&app, other_org, &alice, "Autre org").await; // autre org

    let (_, list) = app.get(&uri, &alice.token).await;
    assert_eq!(titles(&list), vec!["Récente", "Ancienne"]);

    // Un nouveau message fait remonter « Ancienne ».
    add_message(&app, ancienne, "user", "Je reprends ce fil.", 5).await;
    let (_, list) = app.get(&uri, &alice.token).await;
    assert_eq!(titles(&list), vec!["Ancienne", "Récente"]);
    assert_ne!(
        list[0]["updated_at"], list[0]["created_at"],
        "updated_at = dernier message"
    );
    assert_eq!(
        list[1]["updated_at"], list[1]["created_at"],
        "vide : updated_at = création"
    );

    let (_, list) = app.get(&uri, &bob.token).await;
    assert_eq!(titles(&list), vec!["De Bob"]);

    let (status, _) = app.get(&uri, &outsider.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app.request(Method::GET, &uri, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn detail_messages_et_sources() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let conv = create_conversation(&app, org, &owner, "Congés").await;

    // Deux documents, puis un échange qui cite deux fragments du premier et un du second.
    let mut docs = Vec::new();
    for (name, text) in [
        (
            "rh.txt",
            "Vingt-cinq jours de congés.\n\nDemande sur le portail RH.",
        ),
        ("frais.txt", "Remboursement sous trente jours."),
    ] {
        let (_, body) = app
            .upload(org, &owner.token, name, text.as_bytes(), None)
            .await;
        let doc: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
        assert_eq!(app.wait_for_ingestion(doc).await, "ready");
        docs.push(doc);
    }
    let question = add_message(&app, conv, "user", "Combien de jours ?", 0).await;
    let answer = add_message(&app, conv, "assistant", "Vingt-cinq [1].", 1).await;
    for (doc, score) in [(docs[0], 0.4_f32), (docs[0], 0.9), (docs[1], 0.6)] {
        exec(
            &app,
            "INSERT INTO message_sources (id, message_id, chunk_id, score)
             SELECT gen_random_uuid(), $1, id, $2 FROM document_chunks WHERE document_id = $3 LIMIT 1",
            vec![answer.into(), score.into(), doc.into()],
        )
        .await;
    }

    let uri = format!("/api/conversations/{conv}");
    let (status, body) = app.get(&uri, &owner.token).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["id"], conv.to_string());
    assert_eq!(body["title"], "Congés");

    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["id"], question.to_string());
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["sources"], json!([]));
    assert_eq!(messages[1]["id"], answer.to_string());
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["content"], "Vingt-cinq [1].");

    // Une source par document, meilleur score, triées.
    let sources = messages[1]["sources"].as_array().expect("sources");
    let got: Vec<(&str, f64)> = sources
        .iter()
        .map(|s| {
            (
                s["title"].as_str().expect("title"),
                s["score"].as_f64().expect("score"),
            )
        })
        .collect();
    assert_eq!(got.len(), 2, "{sources:?}");
    assert_eq!(got[0].0, "rh.txt");
    assert!((got[0].1 - 0.9).abs() < 1e-6);
    assert_eq!(got[1].0, "frais.txt");
    assert!((got[1].1 - 0.6).abs() < 1e-6);

    // Document supprimé : sa source disparaît, le message reste.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/documents/{}", docs[0]),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, body) = app.get(&uri, &owner.token).await;
    let sources = body["messages"][1]["sources"].as_array().expect("sources");
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0]["title"], "frais.txt");
}

#[tokio::test]
async fn conversation_privee_a_son_auteur() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let member = app.register("member").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;
    app.add_member(org, &owner, &member, "member").await;

    let conv = create_conversation(&app, org, &member, "Privée").await;
    let uri = format!("/api/conversations/{conv}");

    // Même l'owner et l'admin de l'org n'y ont pas accès.
    for user in [&owner, &admin, &outsider] {
        let (status, _) = app.get(&uri, &user.token).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "GET en {}", user.email);
        let (status, _) = app
            .request(Method::DELETE, &uri, Some(&user.token), None)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "DELETE en {}", user.email);
    }
    let (status, _) = app.request(Method::GET, &uri, None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = app
        .get(
            &format!("/api/conversations/{}", Uuid::new_v4()),
            &member.token,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = app.get(&uri, &member.token).await;
    assert_eq!(status, StatusCode::OK);

    // Retiré de l'org, l'auteur perd l'accès à ses conversations de cette org.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/orgs/{org}/members/{}", member.id),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = app.get(&uri, &member.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn supprimer_une_conversation() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let keep = create_conversation(&app, org, &owner, "À garder").await;
    let conv = create_conversation(&app, org, &owner, "À supprimer").await;
    add_message(&app, conv, "user", "Question ?", 0).await;
    add_message(&app, keep, "user", "Autre question ?", 0).await;

    let uri = format!("/api/conversations/{conv}");
    let (status, _) = app
        .request(Method::DELETE, &uri, Some(&owner.token), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app.get(&uri, &owner.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .request(Method::DELETE, &uri, Some(&owner.token), None)
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "deuxième suppression");

    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM messages WHERE conversation_id = $1",
            vec![conv.into()]
        )
        .await,
        0,
        "messages supprimés en cascade"
    );
    assert_eq!(
        count(
            &app,
            "SELECT count(*) AS n FROM messages WHERE conversation_id = $1",
            vec![keep.into()]
        )
        .await,
        1,
        "l'autre conversation est intacte"
    );
    let (_, list) = app
        .get(&format!("/api/orgs/{org}/conversations"), &owner.token)
        .await;
    assert_eq!(titles(&list), vec!["À garder"]);
}

#[tokio::test]
async fn relire_ce_qu_on_a_vu_en_direct() {
    use common::{ws_connect, ws_recv, ws_send};

    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let (_, body) = app
        .upload(
            org,
            &owner.token,
            "conges.txt",
            "Chaque salarié dispose de vingt-cinq jours de congés par an.".as_bytes(),
            None,
        )
        .await;
    let doc: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    let conv = create_conversation(&app, org, &owner, "Congés").await;

    let mut socket = ws_connect(addr, &owner.token).await;
    ws_send(
        &mut socket,
        json!({ "type": "ask", "conversation_id": conv, "question": "Combien de jours de congés ?" }),
    )
    .await;
    let mut live = Vec::new();
    loop {
        let msg = ws_recv(&mut socket).await;
        let end = msg["type"] == "done" || msg["type"] == "error";
        live.push(msg);
        if end {
            break;
        }
    }
    let citations = live[0]["citations"].clone();
    let answer: String = live
        .iter()
        .filter(|m| m["type"] == "token")
        .filter_map(|m| m["delta"].as_str())
        .collect();
    let message_id = live.last().map(|m| m["message_id"].clone()).expect("done");

    let (_, body) = app
        .get(&format!("/api/conversations/{conv}"), &owner.token)
        .await;
    let messages = body["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0]["content"], "Combien de jours de congés ?");
    assert_eq!(messages[1]["id"], message_id);
    assert_eq!(messages[1]["content"], answer.as_str());
    assert_eq!(
        messages[1]["sources"], citations,
        "mêmes sources qu'en direct"
    );
}
