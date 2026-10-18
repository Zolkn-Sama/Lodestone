//! Application des limites du plan (BILL-02).
//!
//! Chaque vérification se fait DANS la transaction qui écrit, après un verrou
//! (`SELECT … FOR UPDATE`) sur la ligne de l'org ou de l'utilisateur : deux
//! requêtes simultanées ne peuvent pas dépasser la limite à elles deux.

use sea_orm::{ConnectionTrait, DbErr, Statement};
use serde::Serialize;
use uuid::Uuid;

use lodestone_core::plan::Plan;

/// Ressource limitée par le plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Members,
    Documents,
    Questions,
    OwnedOrgs,
}

/// Dépassement de quota : renvoyé au client (402) avec une suggestion d'upgrade.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuotaExceeded {
    pub resource: Resource,
    pub limit: u32,
    pub plan: Plan,
    /// Plan qui lève la limite.
    pub upgrade_to: Plan,
}

impl QuotaExceeded {
    fn new(resource: Resource, limit: u32, plan: Plan) -> Self {
        Self {
            resource,
            limit,
            plan,
            upgrade_to: Plan::Pro,
        }
    }

    pub fn message(&self) -> String {
        let what = match self.resource {
            Resource::Members => format!(
                "{} membres maximum (invitations en attente comprises)",
                self.limit
            ),
            Resource::Documents => format!("{} documents maximum", self.limit),
            Resource::Questions => format!("{} questions par mois maximum", self.limit),
            Resource::OwnedOrgs => format!(
                "{} organisation gratuite maximum par utilisateur",
                self.limit
            ),
        };
        format!(
            "Limite du plan {} atteinte : {what}. Passez au plan {} pour aller au-delà.",
            display(self.plan),
            display(self.upgrade_to)
        )
    }
}

fn display(plan: Plan) -> &'static str {
    match plan {
        Plan::Free => "Free",
        Plan::Pro => "Pro",
        Plan::Sovereign => "Sovereign",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum QuotaError {
    #[error("{}", .0.message())]
    Exceeded(QuotaExceeded),
    #[error(transparent)]
    Database(#[from] DbErr),
}

/// Verrouille l'org jusqu'à la fin de la transaction et renvoie son plan.
async fn lock_org<C: ConnectionTrait>(conn: &C, org_id: Uuid) -> Result<Plan, QuotaError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT plan FROM organizations WHERE id = $1 FOR UPDATE",
            [org_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("organisation".into()))?;
    let plan: String = row.try_get("", "plan")?;
    // La contrainte CHECK garantit une valeur connue ; sinon, le plus restrictif.
    Ok(plan.parse().unwrap_or(Plan::Free))
}

async fn count<C: ConnectionTrait>(conn: &C, sql: &str, id: Uuid) -> Result<i64, DbErr> {
    conn.query_one(Statement::from_sql_and_values(
        conn.get_database_backend(),
        sql,
        [id.into()],
    ))
    .await?
    .ok_or_else(|| DbErr::RecordNotFound("comptage".into()))?
    .try_get("", "n")
}

fn check(resource: Resource, used: i64, limit: Option<u32>, plan: Plan) -> Result<(), QuotaError> {
    match limit {
        Some(limit) if used >= i64::from(limit) => Err(QuotaError::Exceeded(QuotaExceeded::new(
            resource, limit, plan,
        ))),
        _ => Ok(()),
    }
}

/// Avant une invitation : membres + invitations encore valides < limite.
pub async fn ensure_can_invite<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<(), QuotaError> {
    let plan = lock_org(conn, org_id).await?;
    let used = count(
        conn,
        "SELECT (SELECT count(*) FROM memberships WHERE org_id = $1)
              + (SELECT count(*) FROM invitations
                 WHERE org_id = $1 AND accepted_at IS NULL AND expires_at > now()) AS n",
        org_id,
    )
    .await?;
    check(Resource::Members, used, plan.limits().max_members, plan)
}

/// Avant d'accepter une invitation : membres < limite (l'invitation, déjà
/// comptée à sa création, n'est plus en attente au moment où elle devient membre).
pub async fn ensure_can_join<C: ConnectionTrait>(conn: &C, org_id: Uuid) -> Result<(), QuotaError> {
    let plan = lock_org(conn, org_id).await?;
    let used = count(
        conn,
        "SELECT count(*) AS n FROM memberships WHERE org_id = $1",
        org_id,
    )
    .await?;
    check(Resource::Members, used, plan.limits().max_members, plan)
}

pub async fn ensure_can_add_document<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<(), QuotaError> {
    let plan = lock_org(conn, org_id).await?;
    let used = count(
        conn,
        "SELECT count(*) AS n FROM documents WHERE org_id = $1",
        org_id,
    )
    .await?;
    check(Resource::Documents, used, plan.limits().max_documents, plan)
}

/// Avant de traiter une question. Pas de verrou ici : la génération dure
/// plusieurs secondes et le compteur n'est incrémenté qu'à la fin. Plusieurs
/// questions lancées en même temps au moment exact de la limite peuvent donc
/// la dépasser de quelques unités (limite « souple »), sans bloquer l'org.
pub async fn ensure_can_ask<C: ConnectionTrait>(conn: &C, org_id: Uuid) -> Result<(), QuotaError> {
    let row = conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT o.plan,
                    COALESCE((SELECT questions::bigint FROM org_usage u
                              WHERE u.org_id = o.id
                                AND u.period = date_trunc('month', now() AT TIME ZONE 'UTC')::date), 0) AS n
             FROM organizations o WHERE o.id = $1",
            [org_id.into()],
        ))
        .await?
        .ok_or_else(|| DbErr::RecordNotFound("organisation".into()))?;
    let plan: String = row.try_get("", "plan")?;
    let plan: Plan = plan.parse().unwrap_or(Plan::Free);
    let used: i64 = row.try_get("", "n")?;
    check(
        Resource::Questions,
        used,
        plan.limits().max_questions_per_month,
        plan,
    )
}

/// Avant de créer une org (toujours en Free) : l'utilisateur ne doit pas déjà
/// posséder le nombre maximal d'orgs Free. Verrouille la ligne de l'utilisateur.
pub async fn ensure_can_create_org<C: ConnectionTrait>(
    conn: &C,
    user_id: Uuid,
) -> Result<(), QuotaError> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "SELECT id FROM users WHERE id = $1 FOR UPDATE",
        [user_id.into()],
    ))
    .await?;
    let used = count(
        conn,
        "SELECT count(*) AS n FROM memberships m
         JOIN organizations o ON o.id = m.org_id
         WHERE m.user_id = $1 AND m.role = 'owner' AND o.plan = 'free'",
        user_id,
    )
    .await?;
    check(
        Resource::OwnedOrgs,
        used,
        Plan::Free.limits().max_owned_orgs,
        Plan::Free,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_explicites() {
        let e = QuotaExceeded::new(Resource::Members, 3, Plan::Free);
        assert_eq!(
            e.message(),
            "Limite du plan Free atteinte : 3 membres maximum (invitations en attente comprises). \
             Passez au plan Pro pour aller au-delà."
        );
        assert!(
            QuotaExceeded::new(Resource::Questions, 100, Plan::Free)
                .message()
                .contains("100 questions par mois")
        );
    }

    #[test]
    fn verification_des_limites() {
        assert!(check(Resource::Documents, 49, Some(50), Plan::Free).is_ok());
        assert!(matches!(
            check(Resource::Documents, 50, Some(50), Plan::Free),
            Err(QuotaError::Exceeded(QuotaExceeded { limit: 50, .. }))
        ));
        assert!(check(Resource::Documents, 10_000, None, Plan::Pro).is_ok());
    }
}
