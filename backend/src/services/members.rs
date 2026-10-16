use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use crate::{dto::members::MemberResponse, error::AppError, repositories};
use lodestone_core::role::Role;

fn parse_role(raw: &str) -> Result<Role, AppError> {
    raw.parse().map_err(|_| AppError::Internal)
}

pub async fn list(db: &DatabaseConnection, org_id: Uuid) -> Result<Vec<MemberResponse>, AppError> {
    let rows = repositories::memberships::list_members(db, org_id).await?;

    rows.into_iter()
        .filter_map(|(m, user)| user.map(|u| (m, u)))
        .map(|(m, u)| {
            Ok(MemberResponse {
                user_id: u.id,
                email: u.email,
                role: parse_role(&m.role)?,
                joined_at: m.created_at,
            })
        })
        .collect()
}

/// Retire un membre. Règles :
/// - seul un owner peut retirer un owner ;
/// - l'org ne peut jamais se retrouver sans owner.
pub async fn remove(
    db: &DatabaseConnection,
    org_id: Uuid,
    actor_role: Role,
    target_user_id: Uuid,
) -> Result<(), AppError> {
    let txn = db.begin().await?;

    let target = repositories::memberships::find(&txn, target_user_id, org_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if parse_role(&target.role)? == Role::Owner {
        if actor_role != Role::Owner {
            return Err(AppError::Forbidden);
        }
        if repositories::memberships::count_owners(&txn, org_id).await? <= 1 {
            return Err(AppError::Conflict(
                "impossible de retirer le dernier owner de l'organisation".into(),
            ));
        }
    }

    repositories::memberships::delete(&txn, target.id).await?;
    txn.commit().await?;
    Ok(())
}

/// Change le rôle d'un membre (route réservée aux owners).
/// Refuse de rétrograder le dernier owner.
pub async fn change_role(
    db: &DatabaseConnection,
    org_id: Uuid,
    target_user_id: Uuid,
    new_role: Role,
) -> Result<MemberResponse, AppError> {
    let txn = db.begin().await?;

    let target = repositories::memberships::find(&txn, target_user_id, org_id)
        .await?
        .ok_or(AppError::NotFound)?;

    if parse_role(&target.role)? == Role::Owner
        && new_role != Role::Owner
        && repositories::memberships::count_owners(&txn, org_id).await? <= 1
    {
        return Err(AppError::Conflict(
            "impossible de rétrograder le dernier owner de l'organisation".into(),
        ));
    }

    let updated = repositories::memberships::update_role(&txn, target, new_role).await?;
    let user = repositories::users::find_by_id(&txn, target_user_id)
        .await?
        .ok_or(AppError::NotFound)?;
    txn.commit().await?;

    Ok(MemberResponse {
        user_id: user.id,
        email: user.email,
        role: new_role,
        joined_at: updated.created_at,
    })
}
