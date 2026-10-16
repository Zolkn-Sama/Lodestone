use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, Set};
use uuid::Uuid;

use lodestone_entity::organizations;

/// Générique sur `C: ConnectionTrait` -> accepte une connexion NORMALE
/// ou une TRANSACTION.
pub async fn create<C: ConnectionTrait>(
    conn: &C,
    name: String,
) -> Result<organizations::Model, DbErr> {
    organizations::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(name),
        ..Default::default()
    }
    .insert(conn)
    .await
}

pub async fn find_by_id<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<Option<organizations::Model>, DbErr> {
    organizations::Entity::find_by_id(id).one(conn).await
}

/// Renomme l'org. `Ok(None)` si elle n'existe pas.
pub async fn rename<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
    name: String,
) -> Result<Option<organizations::Model>, DbErr> {
    let Some(org) = find_by_id(conn, id).await? else {
        return Ok(None);
    };
    let mut active: organizations::ActiveModel = org.into();
    active.name = Set(name);
    active.update(conn).await.map(Some)
}

/// Supprime l'org. Les memberships partent avec (FK `ON DELETE CASCADE`).
/// Renvoie `true` si une ligne a été supprimée.
pub async fn delete<C: ConnectionTrait>(conn: &C, id: Uuid) -> Result<bool, DbErr> {
    let res = organizations::Entity::delete_by_id(id).exec(conn).await?;
    Ok(res.rows_affected > 0)
}
