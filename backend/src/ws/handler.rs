//! `GET /ws?token=<access_jwt>` : la connexion temps réel (chat, statuts).

use axum::{
    extract::{
        FromRequestParts, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::request::Parts,
    response::Response,
};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc;
use uuid::Uuid;

use lodestone_auth::jwt::decode_access_token;

use crate::{
    error::AppError,
    repositories,
    state::AppState,
    ws::protocol::{ClientMsg, ServerMsg},
};

/// Taille maximale d'un message client (une question n'a pas besoin de plus).
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// Messages serveur en attente d'envoi, par connexion.
const OUTBOX_CAPACITY: usize = 64;

#[derive(Deserialize)]
struct WsQuery {
    token: Option<String>,
}

/// Utilisateur authentifié par `?token=` (un navigateur ne peut pas poser
/// d'en-tête Authorization sur une connexion WebSocket).
/// Placé AVANT `WebSocketUpgrade` : un token invalide reçoit un 401 HTTP,
/// sans upgrade.
pub struct WsUser {
    pub user_id: Uuid,
}

impl FromRequestParts<AppState> for WsUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let Query(query) = Query::<WsQuery>::from_request_parts(parts, state)
            .await
            .map_err(|_| AppError::Unauthorized)?;
        let token = query.token.ok_or(AppError::Unauthorized)?;
        let claims = decode_access_token(&token, &state.config.jwt_secret)
            .map_err(|_| AppError::Unauthorized)?;

        // Token valide mais compte supprimé : refusé aussi.
        repositories::users::find_by_id(&state.db, claims.sub)
            .await?
            .ok_or(AppError::Unauthorized)?;

        Ok(WsUser {
            user_id: claims.sub,
        })
    }
}

pub async fn ws_handler(
    State(state): State<AppState>,
    user: WsUser,
    ws: WebSocketUpgrade,
) -> Response {
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, state, user.user_id))
}

async fn handle_socket(socket: WebSocket, _state: AppState, user_id: Uuid) {
    tracing::debug!(%user_id, "connexion WebSocket ouverte");
    let (mut sink, mut stream) = socket.split();

    // Tout envoi passe par ce canal : la boucle de lecture comme, plus tard,
    // les notifications (doc_status) ou le streaming d'une réponse.
    let (outbox, mut outbox_rx) = mpsc::channel::<ServerMsg>(OUTBOX_CAPACITY);
    let writer = tokio::spawn(async move {
        while let Some(msg) = outbox_rx.recv().await {
            let Ok(json) = serde_json::to_string(&msg) else {
                continue;
            };
            if sink.send(Message::Text(json.into())).await.is_err() {
                break; // client parti
            }
        }
        let _ = sink.close().await;
    });

    while let Some(frame) = stream.next().await {
        let reply = match frame {
            Ok(Message::Text(text)) => match serde_json::from_str::<ClientMsg>(&text) {
                Ok(msg) => handle_message(msg).await,
                Err(e) => Some(error(format!("message invalide : {e}"))),
            },
            Ok(Message::Binary(_)) => Some(error("seuls les messages texte (JSON) sont acceptés")),
            // Ping/pong de protocole : gérés par axum.
            Ok(Message::Ping(_) | Message::Pong(_)) => None,
            Ok(Message::Close(_)) | Err(_) => break,
        };
        if let Some(reply) = reply
            && outbox.send(reply).await.is_err()
        {
            break;
        }
    }

    drop(outbox); // ferme le canal : l'écrivain termine et ferme la socket
    let _ = writer.await;
    tracing::debug!(%user_id, "connexion WebSocket fermée");
}

async fn handle_message(msg: ClientMsg) -> Option<ServerMsg> {
    match msg {
        ClientMsg::Ping => Some(ServerMsg::Pong),
        // Branché sur le pipeline RAG avec RAG-01.
        ClientMsg::Ask { .. } => Some(error("les questions ne sont pas encore disponibles")),
    }
}

fn error(message: impl Into<String>) -> ServerMsg {
    ServerMsg::Error {
        message: message.into(),
    }
}
