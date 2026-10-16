use sea_orm::prelude::DateTimeWithTimeZone;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

use lodestone_core::role::Role;

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
    pub created_at: DateTimeWithTimeZone,
}

impl From<lodestone_entity::organizations::Model> for OrgResponse {
    fn from(o: lodestone_entity::organizations::Model) -> Self {
        Self {
            id: o.id,
            name: o.name,
            created_at: o.created_at,
        }
    }
}

/// Une org vue par l'un de ses membres : on ajoute son propre rôle.
#[derive(Debug, Serialize)]
pub struct MyOrgResponse {
    pub id: Uuid,
    pub name: String,
    pub role: Role,
    pub created_at: DateTimeWithTimeZone,
}

impl MyOrgResponse {
    pub fn new(o: lodestone_entity::organizations::Model, role: Role) -> Self {
        Self {
            id: o.id,
            name: o.name,
            role,
            created_at: o.created_at,
        }
    }
}
