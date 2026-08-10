use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use lodestone_core::role;
use lodestone_entity::organizations;
use crate::{error::AppError, repositories};

/// Crée une org ET rend son créateur `owner`, de façon atomique.
pub async fn create_org(
    db: &DatabaseConnection,
    creator_id: Uuid,
    name: String,
) -> Result<organizations::Model, AppError> {
    // On ouvre une transaction : tout ce qui suit réussit ensemble ou est annulé.
    let txn = db.begin().await?;

    let org = repositories::organizations::create(&txn, name).await?;
    repositories::memberships::create(&txn, creator_id, org.id, role::Role::Owner).await?;

    // Rien n'est réellement écrit tant qu'on n'a pas commité.
    txn.commit().await?;
    Ok(org)
}