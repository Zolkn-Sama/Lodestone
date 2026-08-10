use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set};
use uuid::Uuid;

use lodestone_core::role;
use lodestone_entity::memberships;

pub async fn create<C: ConnectionTrait>(
    conn: &C,
    user_id: Uuid,
    org_id: Uuid,
    role: role::Role,
) -> Result<memberships::Model, DbErr> {
    memberships::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        org_id: Set(org_id),
        role: Set(role.as_str().to_string()),
        ..Default::default()
    }
        .insert(conn)
        .await
}