//! Plans d'abonnement et leurs limites (spec §1).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plan {
    /// Essai / TPE : gratuit, limité.
    Free,
    /// PME : par siège, documents illimités (fair use).
    Pro,
    /// Self-hosted, sans limite applicative.
    Sovereign,
}

/// Limites d'un plan. `None` = illimité.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PlanLimits {
    /// Membres de l'org (sièges), owner compris.
    pub max_members: Option<u32>,
    pub max_documents: Option<u32>,
    pub max_questions_per_month: Option<u32>,
    /// Orgs de ce plan qu'un même utilisateur peut posséder.
    pub max_owned_orgs: Option<u32>,
}

impl Plan {
    /// Représentation stockée en base (colonne TEXT).
    pub fn as_str(&self) -> &'static str {
        match self {
            Plan::Free => "free",
            Plan::Pro => "pro",
            Plan::Sovereign => "sovereign",
        }
    }

    pub fn limits(&self) -> PlanLimits {
        match self {
            Plan::Free => PlanLimits {
                max_members: Some(3),
                max_documents: Some(50),
                max_questions_per_month: Some(100),
                max_owned_orgs: Some(1),
            },
            // Pro : facturé au siège, donc pas de plafond de membres ;
            // documents et questions en « fair use ».
            Plan::Pro | Plan::Sovereign => PlanLimits {
                max_members: None,
                max_documents: None,
                max_questions_per_month: None,
                max_owned_orgs: None,
            },
        }
    }
}

impl std::str::FromStr for Plan {
    type Err = UnknownPlan;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "free" => Ok(Plan::Free),
            "pro" => Ok(Plan::Pro),
            "sovereign" => Ok(Plan::Sovereign),
            other => Err(UnknownPlan(other.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("plan inconnu : {0}")]
pub struct UnknownPlan(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limites_du_plan_free() {
        assert_eq!(
            Plan::Free.limits(),
            PlanLimits {
                max_members: Some(3),
                max_documents: Some(50),
                max_questions_per_month: Some(100),
                max_owned_orgs: Some(1),
            }
        );
    }

    #[test]
    fn pro_et_sovereign_sans_limite() {
        for plan in [Plan::Pro, Plan::Sovereign] {
            let l = plan.limits();
            assert_eq!(
                (
                    l.max_members,
                    l.max_documents,
                    l.max_questions_per_month,
                    l.max_owned_orgs
                ),
                (None, None, None, None),
                "{plan:?}"
            );
        }
    }

    #[test]
    fn aller_retour_texte() {
        for p in [Plan::Free, Plan::Pro, Plan::Sovereign] {
            assert_eq!(p.as_str().parse::<Plan>().ok(), Some(p));
        }
        assert!("enterprise".parse::<Plan>().is_err());
    }
}
