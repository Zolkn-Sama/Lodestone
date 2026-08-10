use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set};
use uuid::Uuid;

use lodestone_entity::organizations;

/// Générique sur `C: ConnectionTrait` -> accepte une connexion NORMALE
/// ou une TRANSACTION. C'est la clé pour l'étape 4.
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