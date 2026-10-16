use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set, prelude::PgVector,
};
use uuid::Uuid;

use lodestone_entity::document_chunks;

pub struct NewChunk {
    pub chunk_index: i32,
    pub content: String,
    pub embedding: Vec<f32>,
}

/// Lignes par INSERT : reste loin de la limite de 65 535 paramètres de Postgres.
const INSERT_BATCH: usize = 500;

/// Insère les fragments d'un document. `org_id` est recopié sur chaque
/// fragment : la recherche vectorielle filtre dessus sans jointure.
pub async fn insert_many<C: ConnectionTrait>(
    conn: &C,
    document_id: Uuid,
    org_id: Uuid,
    chunks: Vec<NewChunk>,
) -> Result<(), DbErr> {
    let rows: Vec<document_chunks::ActiveModel> = chunks
        .into_iter()
        .map(|c| document_chunks::ActiveModel {
            id: Set(Uuid::new_v4()),
            document_id: Set(document_id),
            org_id: Set(org_id),
            chunk_index: Set(c.chunk_index),
            content: Set(c.content),
            embedding: Set(PgVector::from(c.embedding)),
        })
        .collect();

    let mut rows = rows.into_iter().peekable();
    while rows.peek().is_some() {
        let batch: Vec<_> = rows.by_ref().take(INSERT_BATCH).collect();
        document_chunks::Entity::insert_many(batch)
            .exec_without_returning(conn)
            .await?;
    }
    Ok(())
}

pub async fn delete_for_document<C: ConnectionTrait>(
    conn: &C,
    document_id: Uuid,
) -> Result<(), DbErr> {
    document_chunks::Entity::delete_many()
        .filter(document_chunks::Column::DocumentId.eq(document_id))
        .exec(conn)
        .await?;
    Ok(())
}
