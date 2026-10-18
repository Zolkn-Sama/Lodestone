//! RAG-01 : `ask` -> embedding de la question -> k fragments les plus proches,
//! filtrés par org. Le mock d'embedding est un « sac de mots » : une question
//! retrouve les documents qui partagent ses mots.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser, WsSocket, ws_connect, ws_recv, ws_send};
use lodestone_backend::repositories::{
    chunks::{self, NewChunk},
    documents::{self, NewDocument},
};

const CONGES: &str = "Politique de congés. Chaque salarié dispose de vingt-cinq jours de congés payés par an. \
    La demande de congés se fait sur le portail RH au moins un mois à l'avance.";
const FRAIS: &str = "Notes de frais. Les frais de déplacement sont remboursés sur justificatif, \
    dans un délai de trente jours, après validation du manager.";
const INFRA: &str = "Infrastructure. Les serveurs de production tournent sur un cluster Kubernetes \
    supervisé par Prometheus et Grafana.";

/// Upload un .txt et attend qu'il soit interrogeable.
async fn add_doc(app: &TestApp, org: Uuid, user: &TestUser, name: &str, text: &str) -> Uuid {
    let (status, body) = app
        .upload(org, &user.token, name, text.as_bytes(), None)
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    let doc: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    doc
}

async fn new_conversation(app: &TestApp, org: Uuid, user: &TestUser) -> Uuid {
    let (status, body) = app
        .post(
            &format!("/api/orgs/{org}/conversations"),
            &user.token,
            json!({}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    body["id"].as_str().expect("id").parse().expect("uuid")
}

/// Pose la question et renvoie le premier message (`sources` ou `error`),
/// après avoir lu la suite de l'échange (`token`… jusqu'à `done` ou `error`)
/// pour ne pas polluer la question suivante.
async fn ask(socket: &mut WsSocket, conversation: Uuid, question: &str) -> Value {
    ws_send(
        socket,
        json!({ "type": "ask", "conversation_id": conversation, "question": question }),
    )
    .await;
    let first = ws_recv(socket).await;
    if first["type"] == "sources" {
        loop {
            let next = ws_recv(socket).await;
            if next["type"] == "done" || next["type"] == "error" {
                break;
            }
        }
    }
    first
}

fn cited_documents(reply: &Value) -> Vec<String> {
    assert_eq!(reply["type"], "sources", "{reply}");
    reply["citations"]
        .as_array()
        .expect("citations")
        .iter()
        .map(|c| c["document_id"].as_str().expect("document_id").to_string())
        .collect()
}

#[tokio::test]
async fn la_question_remonte_le_bon_document() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;

    let conges = add_doc(&app, org, &owner, "conges.txt", CONGES).await;
    let frais = add_doc(&app, org, &owner, "frais.txt", FRAIS).await;
    add_doc(&app, org, &owner, "infra.txt", INFRA).await;

    let conv = new_conversation(&app, org, &member).await;
    let mut socket = ws_connect(addr, &member.token).await;

    let reply = ask(
        &mut socket,
        conv,
        "Combien de jours de congés payés par an ?",
    )
    .await;
    let docs = cited_documents(&reply);
    assert_eq!(docs[0], conges.to_string(), "{reply}");
    let first = &reply["citations"][0];
    assert_eq!(first["title"], "conges.txt");
    assert!(first["score"].as_f64().is_some_and(|s| s > 0.0 && s <= 1.0));

    let reply = ask(
        &mut socket,
        conv,
        "Comment se font les remboursements de frais de déplacement ?",
    )
    .await;
    assert_eq!(cited_documents(&reply)[0], frais.to_string(), "{reply}");

    // Scores décroissants.
    let scores: Vec<f64> = reply["citations"]
        .as_array()
        .expect("citations")
        .iter()
        .filter_map(|c| c["score"].as_f64())
        .collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
}

#[tokio::test]
async fn aucun_fragment_d_une_autre_org() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let org_a = app.create_org(&alice).await;
    let org_b = app.create_org(&bob).await;

    let question = "Quel est le code du coffre-fort de la direction ?";
    add_doc(&app, org_a, &alice, "conges.txt", CONGES).await;
    // Dans l'org de Bob, un document qui contient EXACTEMENT la question :
    // c'est le meilleur résultat possible, il ne doit pourtant jamais remonter.
    let secret = add_doc(&app, org_b, &bob, "secret.txt", question).await;

    let conv = new_conversation(&app, org_a, &alice).await;
    let mut socket = ws_connect(addr, &alice.token).await;
    let docs = cited_documents(&ask(&mut socket, conv, question).await);
    assert!(
        !docs.contains(&secret.to_string()),
        "fuite entre orgs : {docs:?}"
    );
    assert_eq!(docs.len(), 1, "seul le document de l'org A est cherchable");
}

#[tokio::test]
async fn petite_org_parmi_une_grosse_org() {
    // 3 fragments noyés parmi 600 d'une autre org, tous plus proches de la
    // question : la petite org doit quand même retrouver ses 3 documents.
    // (Ici Postgres passe par l'index org_id puis trie exactement ;
    // `hnsw.iterative_scan` protège le cas des grosses orgs, où il préfère
    // l'index HNSW.)
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let small = app.register("small").await;
    let big = app.register("big").await;
    let org_small = app.create_org(&small).await;
    let org_big = app.create_org(&big).await;

    for (i, text) in [CONGES, FRAIS, INFRA].into_iter().enumerate() {
        add_doc(&app, org_small, &small, &format!("doc{i}.txt"), text).await;
    }

    let doc = documents::create(
        &app.db,
        NewDocument {
            org_id: org_big,
            title: "gros.txt".into(),
            source_type: "txt".into(),
            uploaded_by: big.id,
        },
    )
    .await
    .expect("document");
    let rows = (0..600)
        .map(|i| NewChunk {
            chunk_index: i,
            content: format!("fragment {i}"),
            // Proches de la question : ils occupent les premières places de l'index.
            embedding: app
                .llm
                .vector_for(&format!("question sans rapport numéro {i}")),
        })
        .collect();
    chunks::insert_many(&app.db, doc.id, org_big, rows)
        .await
        .expect("fragments");

    let conv = new_conversation(&app, org_small, &small).await;
    let mut socket = ws_connect(addr, &small.token).await;
    let docs = cited_documents(&ask(&mut socket, conv, "question sans rapport").await);
    assert_eq!(docs.len(), 3, "les 3 documents de la petite org : {docs:?}");
}

#[tokio::test]
async fn au_plus_k_fragments_et_une_citation_par_document() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    // 8 documents qui parlent tous de congés.
    for i in 0..8 {
        add_doc(
            &app,
            org,
            &owner,
            &format!("conges{i}.txt"),
            &format!("{CONGES} Version {i}."),
        )
        .await;
    }
    let conv = new_conversation(&app, org, &owner).await;
    let mut socket = ws_connect(addr, &owner.token).await;
    let docs = cited_documents(&ask(&mut socket, conv, "jours de congés").await);
    assert_eq!(docs.len(), 5, "TOP_K = 5 fragments");
    let mut unique = docs.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        docs.len(),
        "un document n'est cité qu'une fois"
    );
}

#[tokio::test]
async fn org_sans_document() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let conv = new_conversation(&app, org, &owner).await;
    let mut socket = ws_connect(addr, &owner.token).await;

    let reply = ask(&mut socket, conv, "Combien de jours de congés ?").await;
    assert_eq!(reply, json!({ "type": "sources", "citations": [] }));
}

#[tokio::test]
async fn acces_a_la_conversation() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let member = app.register("member").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &member, "member").await;
    add_doc(&app, org, &owner, "conges.txt", CONGES).await;

    let owner_conv = new_conversation(&app, org, &owner).await;
    let member_conv = new_conversation(&app, org, &member).await;
    let mut socket = ws_connect(addr, &member.token).await;

    // La conversation d'un autre (même dans la même org) ou inconnue.
    for conv in [owner_conv, Uuid::new_v4()] {
        let reply = ask(&mut socket, conv, "congés ?").await;
        assert_eq!(
            reply,
            json!({ "type": "error", "message": "conversation introuvable" })
        );
    }

    // Sa propre conversation fonctionne…
    assert_eq!(
        ask(&mut socket, member_conv, "congés ?").await["type"],
        "sources"
    );

    // … jusqu'à ce qu'il soit retiré de l'org.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/orgs/{org}/members/{}", member.id),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let reply = ask(&mut socket, member_conv, "congés ?").await;
    assert_eq!(
        reply,
        json!({ "type": "error", "message": "conversation introuvable" })
    );
}

#[tokio::test]
async fn questions_invalides() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let conv = new_conversation(&app, org, &owner).await;
    let mut socket = ws_connect(addr, &owner.token).await;

    for question in ["", "   \n "] {
        let reply = ask(&mut socket, conv, question).await;
        assert_eq!(
            reply,
            json!({ "type": "error", "message": "la question est vide" })
        );
    }
    let reply = ask(&mut socket, conv, &"mot ".repeat(1001)).await;
    assert_eq!(reply["type"], "error");
    assert!(
        reply["message"]
            .as_str()
            .is_some_and(|m| m.contains("trop longue"))
    );

    // La connexion reste utilisable.
    assert_eq!(ask(&mut socket, conv, "congés ?").await["type"], "sources");
}
