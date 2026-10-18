use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set, Statement,
};
use uuid::Uuid;

use lodestone_entity::messages;

/// Auteur d'un message (colonne `messages.role`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageRole {
    User,
    Assistant,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
        }
    }
}

pub struct NewMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub role: MessageRole,
    pub content: String,
    /// Horodatage explicite : dans une même transaction, `now()` donnerait la
    /// même heure à la question et à la réponse.
    pub created_at: DateTime<Utc>,
}

pub async fn create<C: ConnectionTrait>(
    conn: &C,
    new: NewMessage,
) -> Result<messages::Model, DbErr> {
    messages::ActiveModel {
        id: Set(new.id),
        conversation_id: Set(new.conversation_id),
        role: Set(new.role.as_str().to_string()),
        content: Set(new.content),
        created_at: Set(new.created_at.into()),
    }
    .insert(conn)
    .await
}

/// Lie les fragments cités (avec leur score) à un message de l'assistant.
/// Un fragment supprimé entre-temps (document effacé pendant la génération)
/// est ignoré au lieu de faire échouer tout l'enregistrement.
/// Renvoie le nombre de sources enregistrées.
pub async fn add_sources<C: ConnectionTrait>(
    conn: &C,
    message_id: Uuid,
    sources: &[(Uuid, f32)],
) -> Result<u64, DbErr> {
    let mut inserted = 0;
    for &(chunk_id, score) in sources {
        let res = conn
            .execute(Statement::from_sql_and_values(
                conn.get_database_backend(),
                "INSERT INTO message_sources (id, message_id, chunk_id, score)
                 SELECT $1, $2, id, $3 FROM document_chunks WHERE id = $4",
                [
                    Uuid::new_v4().into(),
                    message_id.into(),
                    score.into(),
                    chunk_id.into(),
                ],
            ))
            .await?;
        inserted += res.rows_affected();
    }
    Ok(inserted)
}

/// Les messages d'une conversation, dans l'ordre chronologique.
pub async fn list_for_conversation<C: ConnectionTrait>(
    conn: &C,
    conversation_id: Uuid,
) -> Result<Vec<messages::Model>, DbErr> {
    messages::Entity::find()
        .filter(messages::Column::ConversationId.eq(conversation_id))
        .order_by_asc(messages::Column::CreatedAt)
        .order_by_asc(messages::Column::Id)
        .all(conn)
        .await
}

/// Une source d'un message : un document, avec le meilleur score de ses fragments cités.
pub struct MessageSource {
    pub message_id: Uuid,
    pub document_id: Uuid,
    pub title: String,
    pub score: f32,
}

/// Les sources de tous les messages d'une conversation (une par document et
/// par message), meilleur score d'abord.
pub async fn sources_for_conversation<C: ConnectionTrait>(
    conn: &C,
    conversation_id: Uuid,
) -> Result<Vec<MessageSource>, DbErr> {
    let rows = conn
        .query_all(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT ms.message_id, c.document_id, d.title, max(ms.score) AS score
             FROM message_sources ms
             JOIN messages m ON m.id = ms.message_id
             JOIN document_chunks c ON c.id = ms.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE m.conversation_id = $1
             GROUP BY ms.message_id, c.document_id, d.title
             ORDER BY ms.message_id, score DESC",
            [conversation_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(MessageSource {
                message_id: row.try_get("", "message_id")?,
                document_id: row.try_get("", "document_id")?,
                title: row.try_get("", "title")?,
                score: row.try_get("", "score")?,
            })
        })
        .collect()
}
