use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

impl Role {
    /// Représentation stockée en base (colonne TEXT).
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Owner => "owner",
            Role::Admin => "admin",
            Role::Member => "member",
        }
    }

    /// Hiérarchie : owner > admin > member. Utile pour le RBAC (org-02).
    pub fn rank(&self) -> u8 {
        match self {
            Role::Owner => 3,
            Role::Admin => 2,
            Role::Member => 1,
        }
    }

    /// "cet utilisateur a-t-il AU MOINS ce niveau ?"
    pub fn at_least(&self, required: Role) -> bool {
        self.rank() >= required.rank()
    }
}

impl std::str::FromStr for Role {
    type Err = UnknownRole;

    /// Inverse de `as_str` : relit la colonne TEXT stockée en base.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Role::Owner),
            "admin" => Ok(Role::Admin),
            "member" => Ok(Role::Member),
            other => Err(UnknownRole(other.to_string())),
        }
    }
}

/// Valeur de rôle inconnue (base corrompue ou entrée invalide).
#[derive(Debug, thiserror::Error)]
#[error("rôle inconnu : {0}")]
pub struct UnknownRole(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hierarchie() {
        assert!(Role::Owner.at_least(Role::Admin));
        assert!(Role::Admin.at_least(Role::Admin));
        assert!(!Role::Member.at_least(Role::Admin));
        assert!(!Role::Admin.at_least(Role::Owner));
    }

    #[test]
    fn aller_retour_texte() {
        for r in [Role::Owner, Role::Admin, Role::Member] {
            assert_eq!(r.as_str().parse::<Role>().ok(), Some(r));
        }
        assert!("superadmin".parse::<Role>().is_err());
    }
}
