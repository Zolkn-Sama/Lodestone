use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

use lodestone_core::role::Role;
use lodestone_entity::invitations;

pub struct NewInvitation {
    pub org_id: Uuid,
    pub email: String,
    pub role: Role,
    pub token_hash: String,
    pub invited_by: Uuid,
    pub expires_at: DateTime<Utc>,
}

pub async fn create<C: ConnectionTrait>(
    conn: &C,
    new: NewInvitation,
) -> Result<invitations::Model, DbErr> {
    invitations::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(new.org_id),
        email: Set(new.email),
        role: Set(new.role.as_str().to_string()),
        token_hash: Set(new.token_hash),
        invited_by: Set(Some(new.invited_by)),
        expires_at: Set(new.expires_at.into()),
        accepted_at: Set(None),
        ..Default::default()
    }
    .insert(conn)
    .await
}

pub async fn find_by_hash<C: ConnectionTrait>(
    conn: &C,
    token_hash: &str,
) -> Result<Option<invitations::Model>, DbErr> {
    invitations::Entity::find()
        .filter(invitations::Column::TokenHash.eq(token_hash))
        .one(conn)
        .await
}

/// Supprime les invitations encore en attente pour cet email dans l'org :
/// ré-inviter quelqu'un invalide l'ancien lien.
pub async fn delete_pending<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
    email: &str,
) -> Result<(), DbErr> {
    invitations::Entity::delete_many()
        .filter(invitations::Column::OrgId.eq(org_id))
        .filter(invitations::Column::Email.eq(email))
        .filter(invitations::Column::AcceptedAt.is_null())
        .exec(conn)
        .await?;
    Ok(())
}

pub async fn mark_accepted<C: ConnectionTrait>(
    conn: &C,
    invitation: invitations::Model,
) -> Result<invitations::Model, DbErr> {
    let mut active: invitations::ActiveModel = invitation.into();
    active.accepted_at = Set(Some(Utc::now().into()));
    active.update(conn).await
}
