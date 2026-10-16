use sea_orm::prelude::DateTimeWithTimeZone;
use serde::Serialize;
use uuid::Uuid;

use lodestone_core::document_status::DocumentStatus;

#[derive(Debug, Serialize)]
pub struct DocumentResponse {
    pub id: Uuid,
    pub title: String,
    pub source_type: String,
    pub status: DocumentStatus,
    pub created_at: DateTimeWithTimeZone,
}

impl TryFrom<lodestone_entity::documents::Model> for DocumentResponse {
    type Error = lodestone_core::document_status::UnknownDocumentStatus;

    fn try_from(d: lodestone_entity::documents::Model) -> Result<Self, Self::Error> {
        Ok(Self {
            id: d.id,
            status: d.status.parse()?,
            title: d.title,
            source_type: d.source_type,
            created_at: d.created_at,
        })
    }
}
