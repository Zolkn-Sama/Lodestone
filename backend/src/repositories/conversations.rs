use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, Set};
use uuid::Uuid;

use lodestone_entity::conversations;

pub async fn create<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
    user_id: Uuid,
    title: String,
) -> Result<conversations::Model, DbErr> {
    conversations::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(org_id),
        user_id: Set(user_id),
        title: Set(title),
        ..Default::default()
    }
    .insert(conn)
    .await
}

pub async fn find_by_id<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<Option<conversations::Model>, DbErr> {
    conversations::Entity::find_by_id(id).one(conn).await
}

/// Une conversation de l'historique, avec sa dernière activité.
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: String,
    pub created_at: sea_orm::prelude::DateTimeWithTimeZone,
    /// Dernier message, ou création si la conversation est vide.
    pub updated_at: sea_orm::prelude::DateTimeWithTimeZone,
}

/// Les conversations de `user_id` dans `org_id`, la plus récemment active d'abord.
pub async fn list_for_user<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<ConversationSummary>, DbErr> {
    let rows = conn
        .query_all(sea_orm::Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT c.id, c.title, c.created_at,
                    COALESCE(max(m.created_at), c.created_at) AS updated_at
             FROM conversations c
             LEFT JOIN messages m ON m.conversation_id = c.id
             WHERE c.org_id = $1 AND c.user_id = $2
             GROUP BY c.id
             ORDER BY updated_at DESC, c.id DESC",
            [org_id.into(), user_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ConversationSummary {
                id: row.try_get("", "id")?,
                title: row.try_get("", "title")?,
                created_at: row.try_get("", "created_at")?,
                updated_at: row.try_get("", "updated_at")?,
            })
        })
        .collect()
}

/// Supprime la conversation ; messages et sources suivent en cascade.
pub async fn delete<C: ConnectionTrait>(conn: &C, id: Uuid) -> Result<bool, DbErr> {
    let res = conversations::Entity::delete_by_id(id).exec(conn).await?;
    Ok(res.rows_affected > 0)
}
