use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
    Statement, TransactionTrait, prelude::PgVector,
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

/// Un fragment retrouvé par la recherche, avec le titre de son document.
#[derive(Debug, Clone)]
pub struct RetrievedChunk {
    pub chunk_id: Uuid,
    pub document_id: Uuid,
    pub title: String,
    pub chunk_index: i32,
    pub content: String,
    /// Similarité cosinus, de -1 à 1 (1 = identique).
    pub score: f32,
}

/// Les `limit` fragments de l'org les plus proches de `embedding` (cosinus).
///
/// Isolation tenant : le filtre `org_id` est dans la requête elle-même.
///
/// Pour une org de taille modeste, Postgres passe par l'index `org_id` puis
/// trie exactement : le résultat est exact. Pour une grosse org, il peut
/// préférer l'index HNSW, qui ne renvoie qu'une quarantaine de candidats
/// (toutes orgs confondues) AVANT le filtre : on risquerait alors moins de
/// `limit` résultats. `hnsw.iterative_scan` (pgvector >= 0.8) fait continuer
/// le parcours de l'index jusqu'à en avoir assez.
pub async fn search(
    db: &DatabaseConnection,
    org_id: Uuid,
    embedding: Vec<f32>,
    limit: u64,
) -> Result<Vec<RetrievedChunk>, DbErr> {
    let txn = db.begin().await?;
    // SET LOCAL : limité à cette transaction, sans effet sur les autres requêtes.
    txn.execute_unprepared("SET LOCAL hnsw.iterative_scan = strict_order")
        .await?;
    let rows = txn
        .query_all(Statement::from_sql_and_values(
            txn.get_database_backend(),
            "SELECT c.id, c.document_id, d.title, c.chunk_index, c.content,
                    (1 - (c.embedding <=> $1))::real AS score
             FROM document_chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.org_id = $2
             ORDER BY c.embedding <=> $1
             LIMIT $3",
            [
                PgVector::from(embedding).into(),
                org_id.into(),
                i64::try_from(limit).unwrap_or(i64::MAX).into(),
            ],
        ))
        .await?;
    txn.commit().await?;

    rows.into_iter()
        .map(|row| {
            Ok(RetrievedChunk {
                chunk_id: row.try_get("", "id")?,
                document_id: row.try_get("", "document_id")?,
                title: row.try_get("", "title")?,
                chunk_index: row.try_get("", "chunk_index")?,
                content: row.try_get("", "content")?,
                score: row.try_get("", "score")?,
            })
        })
        .collect()
}
