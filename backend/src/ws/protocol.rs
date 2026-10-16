//! Messages échangés sur `/ws` (§6) : du JSON typé par le champ `type`.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use lodestone_core::document_status::DocumentStatus;

/// Client -> serveur.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Poser une question dans une conversation (traité à partir de RAG-01).
    Ask {
        conversation_id: Uuid,
        question: String,
    },
    /// Test de vie applicatif : le serveur répond `pong`.
    Ping,
}

/// Serveur -> client.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Sources retenues, envoyées AVANT le premier token de la réponse.
    Sources {
        citations: Vec<Citation>,
    },
    /// Morceau de réponse en streaming.
    Token {
        delta: String,
    },
    /// Fin de la réponse.
    Done {
        message_id: Uuid,
    },
    /// Progression de l'ingestion d'un document.
    DocStatus {
        document_id: Uuid,
        status: DocumentStatus,
    },
    Error {
        message: String,
    },
    Pong,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Citation {
    pub document_id: Uuid,
    pub title: String,
    pub score: f32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn client_msg_depuis_json() {
        let id = Uuid::new_v4();
        let msg: ClientMsg = serde_json::from_value(json!({
            "type": "ask",
            "conversation_id": id,
            "question": "Quelle est la procédure de congés ?"
        }))
        .expect("ask");
        assert_eq!(
            msg,
            ClientMsg::Ask {
                conversation_id: id,
                question: "Quelle est la procédure de congés ?".into()
            }
        );
        assert_eq!(
            serde_json::from_value::<ClientMsg>(json!({ "type": "ping" })).ok(),
            Some(ClientMsg::Ping)
        );
        assert!(serde_json::from_value::<ClientMsg>(json!({ "type": "inconnu" })).is_err());
        assert!(serde_json::from_value::<ClientMsg>(json!({ "type": "ask" })).is_err());
    }

    #[test]
    fn server_msg_vers_json() {
        let id = Uuid::nil();
        let cases = [
            (
                ServerMsg::Sources {
                    citations: vec![Citation {
                        document_id: id,
                        title: "RH.pdf".into(),
                        score: 0.5,
                    }],
                },
                json!({ "type": "sources", "citations": [ { "document_id": id, "title": "RH.pdf", "score": 0.5 } ] }),
            ),
            (
                ServerMsg::Token {
                    delta: "La procédure".into(),
                },
                json!({ "type": "token", "delta": "La procédure" }),
            ),
            (
                ServerMsg::Done { message_id: id },
                json!({ "type": "done", "message_id": id }),
            ),
            (
                ServerMsg::DocStatus {
                    document_id: id,
                    status: DocumentStatus::Ready,
                },
                json!({ "type": "doc_status", "document_id": id, "status": "ready" }),
            ),
            (
                ServerMsg::Error {
                    message: "oups".into(),
                },
                json!({ "type": "error", "message": "oups" }),
            ),
            (ServerMsg::Pong, json!({ "type": "pong" })),
        ];
        for (msg, expected) in cases {
            assert_eq!(serde_json::to_value(&msg).expect("json"), expected);
        }
    }
}
