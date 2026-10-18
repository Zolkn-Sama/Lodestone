use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, Set};
use uuid::Uuid;

use lodestone_core::plan::Plan;
use lodestone_entity::organizations;

/// Générique sur `C: ConnectionTrait` -> accepte une connexion NORMALE
/// ou une TRANSACTION.
pub async fn create<C: ConnectionTrait>(
    conn: &C,
    name: String,
    plan: Plan,
) -> Result<organizations::Model, DbErr> {
    organizations::ActiveModel {
        id: Set(Uuid::new_v4()),
        name: Set(name),
        plan: Set(plan.as_str().to_string()),
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

/// Change le plan de l'org (Stripe le fera avec BILL-03).
pub async fn set_plan<C: ConnectionTrait>(conn: &C, id: Uuid, plan: Plan) -> Result<(), DbErr> {
    organizations::ActiveModel {
        id: Set(id),
        plan: Set(plan.as_str().to_string()),
        ..Default::default()
    }
    .update(conn)
    .await?;
    Ok(())
}

/// Date de paiement de la mise en service Sovereign, si payée.
pub async fn sovereign_setup_paid_at<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<Option<sea_orm::prelude::DateTimeWithTimeZone>, DbErr> {
    let row = conn
        .query_one(sea_orm::Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT sovereign_setup_paid_at FROM organizations WHERE id = $1",
            [id.into()],
        ))
        .await?;
    match row {
        Some(row) => row.try_get("", "sovereign_setup_paid_at"),
        None => Ok(None),
    }
}

/// Enregistre le paiement de la mise en service (la première fois seulement).
pub async fn mark_sovereign_setup_paid<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<(), DbErr> {
    conn.execute(sea_orm::Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE organizations SET sovereign_setup_paid_at = now()
         WHERE id = $1 AND sovereign_setup_paid_at IS NULL",
        [id.into()],
    ))
    .await?;
    Ok(())
}
