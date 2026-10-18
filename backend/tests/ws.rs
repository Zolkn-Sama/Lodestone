//! WS-01 : connexion WebSocket authentifiée, messages typés par `type`.

mod common;

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::json;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{Error as WsError, Message},
};

use common::{TestApp, ws_connect as connect, ws_recv as recv, ws_send as send};

/// Statut HTTP d'une connexion refusée (avant l'upgrade).
async fn rejected_status(url: String) -> u16 {
    match connect_async(url).await {
        Err(WsError::Http(res)) => res.status().as_u16(),
        Err(e) => panic!("erreur inattendue : {e}"),
        Ok(_) => panic!("la connexion aurait dû être refusée"),
    }
}

#[tokio::test]
async fn connexion_refusee_sans_token_valide() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let user = app.register("user").await;

    assert_eq!(
        rejected_status(format!("ws://{addr}/ws")).await,
        401,
        "sans token"
    );
    assert_eq!(
        rejected_status(format!("ws://{addr}/ws?token=pas.un.jwt")).await,
        401,
        "token invalide"
    );

    // Token valide, mais compte supprimé entre-temps.
    app.db
        .execute(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "DELETE FROM users WHERE id = $1",
            [user.id.into()],
        ))
        .await
        .expect("suppression");
    assert_eq!(
        rejected_status(format!("ws://{addr}/ws?token={}", user.token)).await,
        401,
        "compte supprimé"
    );
}

#[tokio::test]
async fn ping_pong() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let user = app.register("user").await;
    let mut socket = connect(addr, &user.token).await;

    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(recv(&mut socket).await, json!({ "type": "pong" }));
}

#[tokio::test]
async fn messages_invalides_sans_couper_la_connexion() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let user = app.register("user").await;
    let mut socket = connect(addr, &user.token).await;

    for bad in [
        Message::text("pas du json"),
        Message::text(json!({ "type": "inconnu" }).to_string()),
        Message::text(json!({ "type": "ask" }).to_string()), // champs manquants
        Message::binary(vec![1, 2, 3]),
    ] {
        socket.send(bad).await.expect("envoi");
        let reply = recv(&mut socket).await;
        assert_eq!(reply["type"], "error", "{reply}");
        assert!(reply["message"].as_str().is_some_and(|m| !m.is_empty()));
    }

    // La connexion est toujours utilisable.
    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(recv(&mut socket).await, json!({ "type": "pong" }));
}

#[tokio::test]
async fn ask_sur_conversation_inconnue() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let user = app.register("user").await;
    let mut socket = connect(addr, &user.token).await;

    send(
        &mut socket,
        json!({
            "type": "ask",
            "conversation_id": uuid::Uuid::new_v4(),
            "question": "Quelle est la procédure de congés ?"
        }),
    )
    .await;
    let reply = recv(&mut socket).await;
    assert_eq!(reply["type"], "error");
    assert_eq!(reply["message"], "conversation introuvable");
}

#[tokio::test]
async fn fermeture_propre() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let user = app.register("user").await;
    let mut socket = connect(addr, &user.token).await;

    socket.close(None).await.expect("fermeture");
    // Le serveur répond au Close puis la socket se termine.
    let end = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(Ok(_)) = socket.next().await {}
    })
    .await;
    assert!(end.is_ok(), "la socket aurait dû se fermer");
}

#[tokio::test]
async fn message_trop_gros_coupe_la_connexion() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let user = app.register("user").await;
    let mut socket = connect(addr, &user.token).await;

    let huge = json!({ "type": "ask", "conversation_id": uuid::Uuid::new_v4(), "question": "x".repeat(100_000) });
    // L'envoi peut réussir ou échouer selon le moment où le serveur coupe.
    let _ = socket.send(Message::text(huge.to_string())).await;

    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await {
                None | Some(Err(_)) | Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "le serveur aurait dû fermer la connexion");
}

#[tokio::test]
async fn plusieurs_connexions_independantes() {
    let app = TestApp::spawn().await;
    let addr = app.serve().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let mut a = connect(addr, &alice.token).await;
    let mut b = connect(addr, &bob.token).await;

    send(&mut a, json!({ "type": "ping" })).await;
    assert_eq!(recv(&mut a).await, json!({ "type": "pong" }));

    // Bob n'a rien reçu du ping d'Alice.
    let nothing = tokio::time::timeout(Duration::from_millis(200), b.next()).await;
    assert!(nothing.is_err(), "Bob ne doit rien recevoir");

    send(&mut b, json!({ "type": "ping" })).await;
    assert_eq!(recv(&mut b).await, json!({ "type": "pong" }));
}
