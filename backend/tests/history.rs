//! RAG-02 / RAG-04 : l'échange (question, réponse, sources) est enregistré à
//! la fin de la génération, avant l'envoi de `done`.

mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, Statement, Value as DbValue};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser, WsSocket, ws_connect, ws_recv, ws_send};
use lodestone_backend::services::{chat::NO_SOURCE_REPLY, ingestion::EMBEDDING_DIMENSIONS};
use lodestone_llm::{ChatFailure, MockLlmClient};

const CONGES: &str =
    "Politique de congés. Chaque salarié dispose de vingt-cinq jours de congés payés par an.";
const FRAIS: &str = "Notes de frais. Les frais de déplacement sont remboursés sous trente jours, \
    y compris les congés de formation.";
const QUESTION: &str = "Combien de jours de congés par an ?";
const REPLY: &str = "Vingt-cinq jours par an [1].";

struct Setup {
    app: TestApp,
    owner: TestUser,
    org: Uuid,
    socket: WsSocket,
    conversation: Uuid,
}

async fn setup(llm: MockLlmClient, docs: &[(&str, &str)]) -> Setup {
    let app = TestApp::spawn_with_llm(llm).await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    for (name, text) in docs {
        upload_ready(&app, org, &owner, name, text).await;
    }
    let conversation = new_conversation(&app, org, &owner).await;
    let socket = ws_connect(addr, &owner.token).await;
    Setup {
        app,
        owner,
        org,
        socket,
        conversation,
    }
}

async fn upload_ready(app: &TestApp, org: Uuid, user: &TestUser, name: &str, text: &str) -> Uuid {
    let (status, body) = app
        .upload(org, &user.token, name, text.as_bytes(), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let doc = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    doc
}

async fn new_conversation(app: &TestApp, org: Uuid, user: &TestUser) -> Uuid {
    let (_, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &user.token,
            json!({}),
        )
        .await;
    body["id"].as_str().expect("id").parse().expect("uuid")
}

/// Pose la question et renvoie tous les messages jusqu'à `done`/`error` inclus.
async fn ask(socket: &mut WsSocket, conversation: Uuid, question: &str) -> Vec<Value> {
    ws_send(
        socket,
        json!({ "type": "ask", "conversation_id": conversation, "question": question }),
    )
    .await;
    let mut messages = Vec::new();
    loop {
        let msg = ws_recv(socket).await;
        let end = msg["type"] == "done" || msg["type"] == "error";
        messages.push(msg);
        if end {
            return messages;
        }
    }
}

async fn rows(app: &TestApp, sql: &str, values: Vec<DbValue>) -> Vec<sea_orm::QueryResult> {
    app.db
        .query_all(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            sql,
            values,
        ))
        .await
        .expect("requête")
}

/// (rôle, contenu) des messages de la conversation, dans l'ordre.
async fn messages(app: &TestApp, conversation: Uuid) -> Vec<(String, String)> {
    rows(
        app,
        "SELECT role, content FROM messages WHERE conversation_id = $1 ORDER BY created_at",
        vec![conversation.into()],
    )
    .await
    .into_iter()
    .map(|r| {
        (
            r.try_get("", "role").expect("role"),
            r.try_get("", "content").expect("content"),
        )
    })
    .collect()
}

async fn count(app: &TestApp, sql: &str) -> i64 {
    rows(app, sql, vec![]).await[0].try_get("", "n").expect("n")
}

#[tokio::test]
async fn question_reponse_et_sources_enregistrees() {
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS).with_reply(REPLY);
    let mut s = setup(llm, &[("conges.txt", CONGES), ("frais.txt", FRAIS)]).await;

    let exchange = ask(&mut s.socket, s.conversation, &format!("  {QUESTION}  ")).await;
    let done = exchange.last().expect("done");
    assert_eq!(done["type"], "done", "{exchange:?}");
    let message_id: Uuid = done["message_id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");

    // RAG-02 : `sources` est bien le premier message, avant tout token.
    assert_eq!(exchange[0]["type"], "sources");

    // RAG-04 : question (sans espaces superflus) puis réponse complète.
    assert_eq!(
        messages(&s.app, s.conversation).await,
        vec![
            ("user".to_string(), QUESTION.to_string()),
            ("assistant".to_string(), REPLY.to_string()),
        ]
    );
    let assistant = rows(
        &s.app,
        "SELECT id FROM messages WHERE role = 'assistant'",
        vec![],
    )
    .await;
    let stored_id: Uuid = assistant[0].try_get("", "id").expect("id");
    assert_eq!(stored_id, message_id, "même id que dans `done`");

    // Sources : liées au message de l'assistant, fragments de l'org, et pour
    // chaque document le meilleur score = celui annoncé dans `sources`.
    let sources = rows(
        &s.app,
        "SELECT c.document_id, max(ms.score) AS score, count(*) AS n
         FROM message_sources ms
         JOIN document_chunks c ON c.id = ms.chunk_id
         WHERE ms.message_id = $1 AND c.org_id = $2
         GROUP BY c.document_id",
        vec![message_id.into(), s.org.into()],
    )
    .await;
    let citations = exchange[0]["citations"].as_array().expect("citations");
    assert_eq!(sources.len(), citations.len());
    for citation in citations {
        let doc: Uuid = citation["document_id"]
            .as_str()
            .expect("doc")
            .parse()
            .expect("uuid");
        let row = sources
            .iter()
            .find(|r| r.try_get::<Uuid>("", "document_id").ok() == Some(doc))
            .expect("source enregistrée pour chaque citation");
        let stored: f32 = row.try_get("", "score").expect("score");
        let announced = citation["score"].as_f64().expect("score") as f32;
        assert!((stored - announced).abs() < 1e-6, "{stored} != {announced}");
    }
    assert_eq!(
        count(
            &s.app,
            "SELECT count(*) AS n FROM message_sources WHERE message_id IS NOT NULL"
        )
        .await,
        sources
            .iter()
            .map(|r| r.try_get::<i64>("", "n").expect("n"))
            .sum::<i64>()
    );
}

#[tokio::test]
async fn plusieurs_echanges_dans_l_ordre() {
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS).with_reply(REPLY);
    let mut s = setup(llm, &[("conges.txt", CONGES)]).await;

    for question in ["Première question ?", "Deuxième question ?"] {
        let exchange = ask(&mut s.socket, s.conversation, question).await;
        assert_eq!(
            exchange.last().map(|m| m["type"].clone()),
            Some(json!("done"))
        );
    }
    let roles: Vec<(String, String)> = messages(&s.app, s.conversation).await;
    assert_eq!(
        roles,
        vec![
            ("user".into(), "Première question ?".into()),
            ("assistant".into(), REPLY.into()),
            ("user".into(), "Deuxième question ?".into()),
            ("assistant".into(), REPLY.into()),
        ]
    );
}

#[tokio::test]
async fn sans_document_echange_enregistre_sans_source() {
    let mut s = setup(MockLlmClient::new(EMBEDDING_DIMENSIONS), &[]).await;
    let exchange = ask(&mut s.socket, s.conversation, QUESTION).await;
    assert_eq!(
        exchange.last().map(|m| m["type"].clone()),
        Some(json!("done"))
    );

    assert_eq!(
        messages(&s.app, s.conversation).await,
        vec![
            ("user".to_string(), QUESTION.to_string()),
            ("assistant".to_string(), NO_SOURCE_REPLY.to_string()),
        ]
    );
    assert_eq!(
        count(&s.app, "SELECT count(*) AS n FROM message_sources").await,
        0
    );
}

#[tokio::test]
async fn rien_n_est_enregistre_en_cas_d_echec() {
    for failure in [ChatFailure::AtStart, ChatFailure::MidStream] {
        let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS)
            .with_reply("un deux trois quatre")
            .with_chat_failure(failure);
        let mut s = setup(llm, &[("conges.txt", CONGES)]).await;
        let exchange = ask(&mut s.socket, s.conversation, QUESTION).await;
        assert_eq!(
            exchange.last().map(|m| m["type"].clone()),
            Some(json!("error"))
        );
        assert_eq!(
            count(&s.app, "SELECT count(*) AS n FROM messages").await,
            0,
            "{failure:?}"
        );
    }

    // Question refusée (vide, conversation d'un autre) : rien non plus.
    let mut s = setup(
        MockLlmClient::new(EMBEDDING_DIMENSIONS),
        &[("conges.txt", CONGES)],
    )
    .await;
    let other = s.app.register("other").await;
    s.app.add_member(s.org, &s.owner, &other, "member").await;
    let foreign = new_conversation(&s.app, s.org, &other).await;
    for (conversation, question) in [(s.conversation, "   "), (foreign, QUESTION)] {
        let exchange = ask(&mut s.socket, conversation, question).await;
        assert_eq!(exchange[0]["type"], "error");
    }
    assert_eq!(count(&s.app, "SELECT count(*) AS n FROM messages").await, 0);
}

#[tokio::test]
async fn document_supprime_pendant_la_generation() {
    // ~1 s de génération : le temps de supprimer le document cité.
    let reply = (0..20)
        .map(|i| format!("mot{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS)
        .with_reply(reply.clone())
        .with_token_delay(Duration::from_millis(50));
    let mut s = setup(llm, &[]).await;
    let doc = upload_ready(&s.app, s.org, &s.owner, "conges.txt", CONGES).await;

    ws_send(
        &mut s.socket,
        json!({ "type": "ask", "conversation_id": s.conversation, "question": QUESTION }),
    )
    .await;
    // La socket était ouverte pendant l'upload : elle a reçu les `doc_status`.
    let first = loop {
        let msg = ws_recv(&mut s.socket).await;
        if msg["type"] != "doc_status" {
            break msg;
        }
    };
    assert_eq!(first["type"], "sources");
    let (status, _) = s
        .app
        .request(
            Method::DELETE,
            &format!("/api/documents/{doc}"),
            Some(&s.owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let last = loop {
        let msg = ws_recv(&mut s.socket).await;
        if msg["type"] != "token" {
            break msg;
        }
    };
    assert_eq!(
        last["type"], "done",
        "l'échange est enregistré malgré tout : {last}"
    );
    assert_eq!(
        messages(&s.app, s.conversation).await,
        vec![
            ("user".to_string(), QUESTION.to_string()),
            ("assistant".to_string(), reply)
        ]
    );
    assert_eq!(
        count(&s.app, "SELECT count(*) AS n FROM message_sources").await,
        0,
        "la source supprimée est ignorée"
    );
}

#[tokio::test]
async fn deconnexion_pendant_la_generation_rien_d_enregistre() {
    let reply = (0..50)
        .map(|i| format!("mot{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS)
        .with_reply(reply)
        .with_token_delay(Duration::from_millis(20));
    let mut s = setup(llm, &[("conges.txt", CONGES)]).await;

    ws_send(
        &mut s.socket,
        json!({ "type": "ask", "conversation_id": s.conversation, "question": QUESTION }),
    )
    .await;
    assert_eq!(ws_recv(&mut s.socket).await["type"], "sources");
    assert_eq!(ws_recv(&mut s.socket).await["type"], "token");
    drop(s.socket);

    // Plus que le temps restant de génération (~1 s).
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert_eq!(count(&s.app, "SELECT count(*) AS n FROM messages").await, 0);
}
