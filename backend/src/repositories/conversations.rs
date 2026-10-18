use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set};
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
