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