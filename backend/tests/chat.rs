//! RAG-03 : réponse générée en streaming (`sources`, `token`…, `done`).

mod common;

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser, WsSocket, ws_connect, ws_recv, ws_send, ws_try_recv};
use lodestone_backend::services::{chat::NO_SOURCE_REPLY, ingestion::EMBEDDING_DIMENSIONS};
use lodestone_llm::{ChatFailure, ChatRole, MockLlmClient};
use lodestone_rag::prompt::SYSTEM_PROMPT;

const CONGES: &str =
    "Politique de congés. Chaque salarié dispose de vingt-cinq jours de congés payés par an.";
const QUESTION: &str = "Combien de jours de congés par an ?";

struct Setup {
    app: TestApp,
    socket: WsSocket,
    conversation: Uuid,
}

/// Une org avec un document sur les congés, une conversation, une WebSocket.
async fn setup(llm: MockLlmClient, with_document: bool) -> Setup {
    let app = TestApp::spawn_with_llm(llm).await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    if with_document {
        upload_ready(&app, org, &owner, "conges.txt", CONGES).await;
    }
    let (status, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &owner.token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    let conversation = body["id"].as_str().expect("id").parse().expect("uuid");
    let socket = ws_connect(addr, &owner.token).await;
    Setup {
        app,
        socket,
        conversation,
    }
}

async fn upload_ready(app: &TestApp, org: Uuid, user: &TestUser, name: &str, text: &str) {
    let (status, body) = app
        .upload(org, &user.token, name, text.as_bytes(), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let doc = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
}

async fn send_ask(socket: &mut WsSocket, conversation: Uuid) {
    ws_send(
        socket,
        json!({ "type": "ask", "conversation_id": conversation, "question": QUESTION }),
    )
    .await;
}

/// Tous les messages jusqu'à `done` ou `error` inclus.
async fn exchange(socket: &mut WsSocket) -> Vec<Value> {
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

fn types(messages: &[Value]) -> Vec<&str> {
    messages.iter().filter_map(|m| m["type"].as_str()).collect()
}

fn tokens(messages: &[Value]) -> Vec<String> {
    messages
        .iter()
        .filter(|m| m["type"] == "token")
        .filter_map(|m| m["delta"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test]
async fn sources_puis_tokens_puis_done() {
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS).with_reply("Vingt-cinq jours par an [1].");
    let mut s = setup(llm, true).await;

    send_ask(&mut s.socket, s.conversation).await;
    let messages = exchange(&mut s.socket).await;

    assert_eq!(
        types(&messages),
        vec![
            "sources", "token", "token", "token", "token", "token", "done"
        ],
        "{messages:?}"
    );
    assert_eq!(messages[0]["citations"][0]["title"], "conges.txt");
    assert_eq!(tokens(&messages).concat(), "Vingt-cinq jours par an [1].");
    let message_id = messages
        .last()
        .and_then(|m| m["message_id"].as_str())
        .expect("message_id");
    assert!(message_id.parse::<Uuid>().is_ok());
}

#[tokio::test]
async fn le_prompt_contient_consignes_extraits_et_question() {
    let mut s = setup(MockLlmClient::new(EMBEDDING_DIMENSIONS), true).await;
    send_ask(&mut s.socket, s.conversation).await;
    exchange(&mut s.socket).await;

    let requests = s.app.llm.chat_requests();
    assert_eq!(requests.len(), 1);
    let prompt = &requests[0];
    assert_eq!(prompt.len(), 2);
    assert_eq!(prompt[0].role, ChatRole::System);
    assert_eq!(prompt[0].content, SYSTEM_PROMPT);
    assert_eq!(prompt[1].role, ChatRole::User);
    let user = &prompt[1].content;
    assert!(user.contains("[1] conges.txt"), "{user}");
    assert!(user.contains("vingt-cinq jours de congés payés"), "{user}");
    assert!(user.ends_with(&format!("Question : {QUESTION}")), "{user}");
}

#[tokio::test]
async fn sans_document_pas_d_appel_au_modele() {
    let mut s = setup(MockLlmClient::new(EMBEDDING_DIMENSIONS), false).await;
    send_ask(&mut s.socket, s.conversation).await;
    let messages = exchange(&mut s.socket).await;

    assert_eq!(types(&messages), vec!["sources", "token", "done"]);
    assert_eq!(messages[0]["citations"], json!([]));
    assert_eq!(tokens(&messages), vec![NO_SOURCE_REPLY]);
    assert!(
        s.app.llm.chat_requests().is_empty(),
        "le modèle n'est pas sollicité"
    );
}

#[tokio::test]
async fn modele_indisponible_erreur_propre() {
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS).with_chat_failure(ChatFailure::AtStart);
    let mut s = setup(llm, true).await;

    send_ask(&mut s.socket, s.conversation).await;
    let messages = exchange(&mut s.socket).await;
    assert_eq!(types(&messages), vec!["sources", "error"]);
    let message = messages[1]["message"].as_str().expect("message");
    assert!(message.contains("indisponible"), "{message}");
    assert!(
        !message.contains("MockLlmClient"),
        "pas de détail technique : {message}"
    );

    // La connexion survit.
    ws_send(&mut s.socket, json!({ "type": "ping" })).await;
    assert_eq!(ws_recv(&mut s.socket).await, json!({ "type": "pong" }));
}

#[tokio::test]
async fn flux_interrompu_pas_de_done() {
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS)
        .with_reply("un deux trois quatre cinq")
        .with_chat_failure(ChatFailure::MidStream);
    let mut s = setup(llm, true).await;

    send_ask(&mut s.socket, s.conversation).await;
    let messages = exchange(&mut s.socket).await;
    assert_eq!(types(&messages), vec!["sources", "token", "token", "error"]);
    assert!(
        messages[3]["message"]
            .as_str()
            .is_some_and(|m| m.contains("interrompue"))
    );
    assert_eq!(
        ws_try_recv(&mut s.socket, Duration::from_millis(200)).await,
        None,
        "pas de done"
    );
}

#[tokio::test]
async fn une_seule_question_a_la_fois_et_ping_pendant_la_generation() {
    // 20 mots à 30 ms : ~600 ms de génération.
    let reply = (0..20)
        .map(|i| format!("mot{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS)
        .with_reply(reply.clone())
        .with_token_delay(Duration::from_millis(30));
    let mut s = setup(llm, true).await;

    send_ask(&mut s.socket, s.conversation).await;
    assert_eq!(ws_recv(&mut s.socket).await["type"], "sources");

    // Pendant la génération : une 2e question est refusée, un ping répond.
    send_ask(&mut s.socket, s.conversation).await;
    ws_send(&mut s.socket, json!({ "type": "ping" })).await;

    let rest = exchange_including(&mut s.socket, "done").await;
    let refused = rest
        .iter()
        .position(|m| m["type"] == "error")
        .expect("2e question refusée");
    assert!(
        rest[refused]["message"]
            .as_str()
            .is_some_and(|m| m.contains("déjà en cours")),
        "{rest:?}"
    );
    let pong = rest.iter().position(|m| m["type"] == "pong").expect("pong");
    let done = rest.iter().position(|m| m["type"] == "done").expect("done");
    assert!(pong < done, "le ping est servi pendant la génération");
    assert_eq!(
        tokens(&rest).concat(),
        reply,
        "la réponse n'est pas mélangée"
    );

    // Une fois la réponse terminée, une nouvelle question est acceptée.
    send_ask(&mut s.socket, s.conversation).await;
    let next = exchange(&mut s.socket).await;
    assert_eq!(
        next.first().map(|m| m["type"].clone()),
        Some(json!("sources"))
    );
    assert_eq!(next.last().map(|m| m["type"].clone()), Some(json!("done")));
}

/// Lit jusqu'au premier message de type `until` inclus (en gardant les autres).
async fn exchange_including(socket: &mut WsSocket, until: &str) -> Vec<Value> {
    let mut messages = Vec::new();
    loop {
        let msg = ws_recv(socket).await;
        let end = msg["type"] == until;
        messages.push(msg);
        if end {
            return messages;
        }
    }
}

#[tokio::test]
async fn deconnexion_pendant_la_generation() {
    let reply = (0..50)
        .map(|i| format!("mot{i}"))
        .collect::<Vec<_>>()
        .join(" ");
    let llm = MockLlmClient::new(EMBEDDING_DIMENSIONS)
        .with_reply(reply)
        .with_token_delay(Duration::from_millis(20));
    let mut s = setup(llm, true).await;

    send_ask(&mut s.socket, s.conversation).await;
    assert_eq!(ws_recv(&mut s.socket).await["type"], "sources");
    assert_eq!(ws_recv(&mut s.socket).await["type"], "token");
    drop(s.socket); // l'utilisateur ferme l'onglet

    // Le serveur continue de servir les autres.
    let addr = s.app.serve().await;
    let other = s.app.register("other").await;
    let org = s.app.create_org(&other).await;
    upload_ready(&s.app, org, &other, "conges.txt", CONGES).await;
    let (_, body) = s
        .app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &other.token,
            json!({}),
        )
        .await;
    let conv: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
    let mut socket = ws_connect(addr, &other.token).await;
    send_ask(&mut socket, conv).await;
    let messages = exchange(&mut socket).await;
    assert_eq!(
        messages.last().map(|m| m["type"].clone()),
        Some(json!("done"))
    );
}
