use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, EntityTrait, Set};
use uuid::Uuid;

use lodestone_core::document_status::DocumentStatus;
use lodestone_entity::documents;

pub struct NewDocument {
    pub org_id: Uuid,
    pub title: String,
    pub source_type: String,
    pub uploaded_by: Uuid,
}

/// Crée le document en statut `pending` (l'ingestion le fera avancer).
pub async fn create<C: ConnectionTrait>(
    conn: &C,
    new: NewDocument,
) -> Result<documents::Model, DbErr> {
    documents::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(new.org_id),
        title: Set(new.title),
        source_type: Set(new.source_type),
        status: Set(DocumentStatus::Pending.as_str().to_string()),
        uploaded_by: Set(new.uploaded_by),
        ..Default::default()
    }
    .insert(conn)
    .await
}

pub async fn find_by_id<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
) -> Result<Option<documents::Model>, DbErr> {
    documents::Entity::find_by_id(id).one(conn).await
}

pub async fn set_status<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
    status: DocumentStatus,
) -> Result<(), DbErr> {
    documents::ActiveModel {
        id: Set(id),
        status: Set(status.as_str().to_string()),
        ..Default::default()
    }
    .update(conn)
    .await?;
    Ok(())
}
