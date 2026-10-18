use sea_orm::prelude::DateTimeWithTimeZone;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

use lodestone_core::{
    plan::{Plan, PlanLimits},
    role::Role,
};

#[derive(Debug, Deserialize, Validate)]
pub struct CreateOrgRequest {
    #[validate(length(min = 1, max = 100, message = "nom : 1 à 100 caractères"))]
    pub name: String,
}

#[derive(Debug, Deserialize, Validate)]
pub struct RenameOrgRequest {
    #[validate(length(min = 1, max = 100, message = "nom : 1 à 100 caractères"))]
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct OrgResponse {
    pub id: Uuid,
    pub name: String,
    pub plan: Plan,
    pub created_at: DateTimeWithTimeZone,
}

impl From<lodestone_entity::organizations::Model> for OrgResponse {
    fn from(o: lodestone_entity::organizations::Model) -> Self {
        Self {
            id: o.id,
            plan: plan_of(&o),
            name: o.name,
            created_at: o.created_at,
        }
    }
}

/// Le plan stocké. La contrainte CHECK en base garantit une valeur connue ;
/// par prudence, une valeur inattendue est journalisée et traitée comme `free`
/// (le plan le plus restrictif).
pub fn plan_of(o: &lodestone_entity::organizations::Model) -> Plan {
    o.plan.parse().unwrap_or_else(|e| {
        tracing::error!(org_id = %o.id, error = %e, "plan inconnu en base, traité comme free");
        Plan::Free
    })
}

/// Une org vue par l'un de ses membres : on ajoute son propre rôle.
#[derive(Debug, Serialize)]
pub struct MyOrgResponse {
    pub id: Uuid,
    pub name: String,
    pub plan: Plan,
    pub role: Role,
    pub created_at: DateTimeWithTimeZone,
}

impl MyOrgResponse {
    pub fn new(o: lodestone_entity::organizations::Model, role: Role) -> Self {
        Self {
            id: o.id,
            plan: plan_of(&o),
            name: o.name,
            role,
            created_at: o.created_at,
        }
    }
}

/// Consommation d'une ressource face à la limite du plan.
#[derive(Debug, Serialize)]
pub struct Quota {
    pub used: i64,
    /// `null` = illimité.
    pub limit: Option<u32>,
}

/// Réponse de `GET /api/orgs/{id}/usage`.
#[derive(Debug, Serialize)]
pub struct UsageResponse {
    pub plan: Plan,
    pub limits: PlanLimits,
    /// Mois en cours (`AAAA-MM`, UTC) pour le compteur de questions.
    pub period: String,
    pub members: Quota,
    pub documents: Quota,
    pub questions: Quota,
}
