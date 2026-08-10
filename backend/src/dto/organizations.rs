use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

#[derive(Debug, Deserialize, Validate)]
pub struct CreateOrgRequest {
    #[validate(length(min = 1, max = 100, message = "nom : 1 à 100 caractères"))]
    pub name: String,
}

#[derive(Debug, Serialize)]
pub struct OrgResponse {
    pub id: Uuid,
    pub name: String,
}
impl From<lodestone_entity::organizations::Model> for OrgResponse {
    fn from(o: lodestone_entity::organizations::Model) -> Self {
        Self { id: o.id, name: o.name }
    }
}