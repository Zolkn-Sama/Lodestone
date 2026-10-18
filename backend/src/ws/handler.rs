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
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use tokio::{
    sync::{
        broadcast::{self, error::RecvError},
        mpsc,
    },
    task::JoinHandle,
};
use uuid::Uuid;

use lodestone_auth::jwt::decode_access_token;

use crate::{
    error::AppError,
    events::OrgEvent,
    repositories, services,
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
    // Abonnement AVANT l'upgrade : quand le client reçoit la réponse de
    // handshake, il ne peut plus rater d'événement.
    let events = state.events.subscribe();
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_socket(socket, state, user.user_id, events))
}

async fn handle_socket(
    socket: WebSocket,
    state: AppState,
    user_id: Uuid,
    events: broadcast::Receiver<OrgEvent>,
) {
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

    let notifier = tokio::spawn(forward_org_events(
        state.db.clone(),
        user_id,
        events,
        outbox.clone(),
    ));

    // La question en cours : une seule à la fois par connexion.
    let mut current: Option<JoinHandle<()>> = None;

    while let Some(frame) = stream.next().await {
        let reply = match frame {
            Ok(Message::Text(text)) => match serde_json::from_str::<ClientMsg>(&text) {
                Ok(ClientMsg::Ping) => Some(ServerMsg::Pong),
                Ok(ClientMsg::Ask {
                    conversation_id,
                    question,
                }) => {
                    if current.as_ref().is_some_and(|task| !task.is_finished()) {
                        Some(error("une question est déjà en cours de traitement"))
                    } else {
                        // Tâche à part : la connexion reste réactive (ping,
                        // fermeture) pendant la génération.
                        let state = state.clone();
                        let outbox = outbox.clone();
                        current = Some(tokio::spawn(async move {
                            services::chat::answer(
                                &state.db,
                                state.llm.as_ref(),
                                user_id,
                                conversation_id,
                                &question,
                                &outbox,
                            )
                            .await;
                        }));
                        None
                    }
                }
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

    // Client parti : on interrompt la génération en cours (et l'appel au modèle).
    if let Some(task) = current {
        task.abort();
        let _ = task.await;
    }
    notifier.abort();
    let _ = notifier.await; // libère sa copie du canal
    drop(outbox); // ferme le canal : l'écrivain termine et ferme la socket
    let _ = writer.await;
    tracing::debug!(%user_id, "connexion WebSocket fermée");
}

/// Relaie au client les événements des orgs dont il est membre. L'appartenance
/// est vérifiée à chaque événement : un membre retiré cesse aussitôt de recevoir.
async fn forward_org_events(
    db: DatabaseConnection,
    user_id: Uuid,
    mut events: broadcast::Receiver<OrgEvent>,
    outbox: mpsc::Sender<ServerMsg>,
) {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(RecvError::Lagged(skipped)) => {
                tracing::warn!(%user_id, skipped, "événements WebSocket perdus (client trop lent)");
                continue;
            }
            Err(RecvError::Closed) => return,
        };

        match repositories::memberships::find(&db, user_id, event.org_id).await {
            Ok(Some(_)) => {
                if outbox.send(event.message).await.is_err() {
                    return; // connexion fermée
                }
            }
            Ok(None) => {} // pas membre de cette org
            Err(e) => tracing::error!(error = %e, "vérification d'appartenance impossible"),
        }
    }
}

fn error(message: impl Into<String>) -> ServerMsg {
    ServerMsg::Error {
        message: message.into(),
    }
}
