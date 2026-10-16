use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, PaginatorTrait,
    QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use lodestone_core::role;
use lodestone_entity::{memberships, organizations, users};

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

/// L'appartenance d'un utilisateur à une org (au plus une, index unique).
pub async fn find<C: ConnectionTrait>(
    conn: &C,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<Option<memberships::Model>, DbErr> {
    memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .filter(memberships::Column::OrgId.eq(org_id))
        .one(conn)
        .await
}

/// Les orgs d'un utilisateur, avec son appartenance (donc son rôle).
pub async fn list_orgs_for_user<C: ConnectionTrait>(
    conn: &C,
    user_id: Uuid,
) -> Result<Vec<(memberships::Model, Option<organizations::Model>)>, DbErr> {
    memberships::Entity::find()
        .filter(memberships::Column::UserId.eq(user_id))
        .order_by_asc(memberships::Column::CreatedAt)
        .find_also_related(organizations::Entity)
        .all(conn)
        .await
}

/// Les membres d'une org, avec l'utilisateur associé (pour l'email).
pub async fn list_members<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<Vec<(memberships::Model, Option<users::Model>)>, DbErr> {
    memberships::Entity::find()
        .filter(memberships::Column::OrgId.eq(org_id))
        .order_by_asc(memberships::Column::CreatedAt)
        .find_also_related(users::Entity)
        .all(conn)
        .await
}

pub async fn count_owners<C: ConnectionTrait>(conn: &C, org_id: Uuid) -> Result<u64, DbErr> {
    memberships::Entity::find()
        .filter(memberships::Column::OrgId.eq(org_id))
        .filter(memberships::Column::Role.eq(role::Role::Owner.as_str()))
        .count(conn)
        .await
}

pub async fn update_role<C: ConnectionTrait>(
    conn: &C,
    membership: memberships::Model,
    role: role::Role,
) -> Result<memberships::Model, DbErr> {
    let mut active: memberships::ActiveModel = membership.into();
    active.role = Set(role.as_str().to_string());
    active.update(conn).await
}

pub async fn delete<C: ConnectionTrait>(conn: &C, id: Uuid) -> Result<(), DbErr> {
    memberships::Entity::delete_by_id(id).exec(conn).await?;
    Ok(())
}
