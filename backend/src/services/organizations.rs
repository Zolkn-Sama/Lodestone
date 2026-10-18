use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use crate::{
    dto::organizations::{MyOrgResponse, Quota, UsageResponse, plan_of},
    error::AppError,
    repositories,
    services::quotas,
};
use lodestone_core::{
    plan::Plan,
    role::{self, Role},
};
use lodestone_entity::organizations;

/// Crée une org ET rend son créateur `owner`, de façon atomique.
/// Crée une org au plan `plan` (config `NEW_ORG_PLAN` : Free en SaaS, Sovereign
/// pour une installation auto-hébergée) ET rend son créateur `owner`.
pub async fn create_org(
    db: &DatabaseConnection,
    creator_id: Uuid,
    name: String,
    plan: Plan,
) -> Result<organizations::Model, AppError> {
    // On ouvre une transaction : tout ce qui suit réussit ensemble ou est annulé.
    let txn = db.begin().await?;

    // En Free : une seule org par utilisateur.
    if plan == Plan::Free {
        quotas::ensure_can_create_org(&txn, creator_id).await?;
    }
    let org = repositories::organizations::create(&txn, name, plan).await?;
    repositories::memberships::create(&txn, creator_id, org.id, role::Role::Owner).await?;

    // Rien n'est réellement écrit tant qu'on n'a pas commité.
    txn.commit().await?;
    Ok(org)
}

/// Les orgs dont l'utilisateur est membre, avec son rôle dans chacune.
pub async fn list_for_user(
    db: &DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<MyOrgResponse>, AppError> {
    let rows = repositories::memberships::list_orgs_for_user(db, user_id).await?;

    rows.into_iter()
        .filter_map(|(m, org)| org.map(|o| (m, o)))
        .map(|(m, o)| {
            let role: Role = m.role.parse().map_err(|_| AppError::Internal)?;
            Ok(MyOrgResponse::new(o, role))
        })
        .collect()
}

pub async fn get(db: &DatabaseConnection, org_id: Uuid) -> Result<organizations::Model, AppError> {
    repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)
}

pub async fn rename(
    db: &DatabaseConnection,
    org_id: Uuid,
    name: String,
) -> Result<organizations::Model, AppError> {
    repositories::organizations::rename(db, org_id, name)
        .await?
        .ok_or(AppError::NotFound)
}

/// Supprime l'org ; memberships (et plus tard documents…) suivent en cascade.
pub async fn delete(db: &DatabaseConnection, org_id: Uuid) -> Result<(), AppError> {
    if repositories::organizations::delete(db, org_id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}

/// Consommation de l'org face aux limites de son plan.
pub async fn usage(db: &DatabaseConnection, org_id: Uuid) -> Result<UsageResponse, AppError> {
    let org = get(db, org_id).await?;
    let plan = plan_of(&org);
    let limits = plan.limits();
    let usage = repositories::usage::current(db, org_id).await?;

    Ok(UsageResponse {
        plan,
        limits,
        period: usage.period,
        members: Quota {
            used: usage.members,
            limit: limits.max_members,
        },
        documents: Quota {
            used: usage.documents,
            limit: limits.max_documents,
        },
        questions: Quota {
            used: usage.questions_this_month,
            limit: limits.max_questions_per_month,
        },
    })
}
