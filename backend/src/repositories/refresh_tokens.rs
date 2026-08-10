use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

use lodestone_entity::refresh_tokens;

pub async fn create(
    db: &DatabaseConnection,
    user_id: Uuid,
    token_hash: String,
    expires_at: DateTime<Utc>,
) -> Result<refresh_tokens::Model, DbErr> {
    refresh_tokens::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        token_hash: Set(token_hash),
        expires_at: Set(expires_at.into()), // DateTime<Utc> -> DateTimeWithTimeZone
        revoked_at: Set(None),
        ..Default::default()
    }
        .insert(db)
        .await
}

pub async fn find_by_hash(
    db: &DatabaseConnection,
    token_hash: &str,
) -> Result<Option<refresh_tokens::Model>, DbErr> {
    refresh_tokens::Entity::find()
        .filter(refresh_tokens::Column::TokenHash.eq(token_hash))
        .one(db)
        .await
}

pub async fn revoke(db: &DatabaseConnection, id: Uuid) -> Result<(), DbErr> {
    if let Some(row) = refresh_tokens::Entity::find_by_id(id).one(db).await? {
        let mut active: refresh_tokens::ActiveModel = row.into();
        active.revoked_at = Set(Some(Utc::now().into()));
        active.update(db).await?;
    }
    Ok(())
}