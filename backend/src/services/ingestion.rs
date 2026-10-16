//! Ingestion d'un document : extraction -> découpage -> embeddings -> stockage.
//! Fait passer le document de `pending` à `processing`, puis `ready` ou `failed`.

use sea_orm::{DatabaseConnection, DbErr, TransactionTrait};
use thiserror::Error;
use uuid::Uuid;

use crate::repositories::{self, chunks::NewChunk};
use lodestone_core::document_status::DocumentStatus;
use lodestone_entity::documents;
use lodestone_llm::{LlmClient, LlmError};
use lodestone_rag::{ChunkConfig, RagError, SourceType, chunk_text, extract_text};

/// Dimension de la colonne `document_chunks.embedding` (nomic-embed-text).
pub const EMBEDDING_DIMENSIONS: usize = 768;

/// Fragments envoyés par appel d'embedding.
const EMBED_BATCH: usize = 32;

#[derive(Debug, Error)]
pub enum IngestionError {
    #[error("document introuvable")]
    DocumentNotFound,

    #[error(transparent)]
    Rag(#[from] RagError),

    #[error(transparent)]
    Llm(#[from] LlmError),

    #[error(transparent)]
    Database(#[from] DbErr),

    #[error(
        "le modèle d'embedding produit des vecteurs de {got} dimensions, la base en attend {expected}"
    )]
    Dimensions { expected: usize, got: usize },

    #[error("la tâche d'extraction s'est interrompue")]
    ExtractionAborted,
}

/// Ingère `bytes` comme contenu du document `document_id`. Renvoie le nombre
/// de fragments stockés. En cas d'échec, le document passe en `failed` et
/// l'erreur est renvoyée. Ré-ingérer un document remplace ses fragments.
pub async fn ingest(
    db: &DatabaseConnection,
    llm: &dyn LlmClient,
    document_id: Uuid,
    bytes: Vec<u8>,
) -> Result<usize, IngestionError> {
    let document = repositories::documents::find_by_id(db, document_id)
        .await?
        .ok_or(IngestionError::DocumentNotFound)?;

    repositories::documents::set_status(db, document.id, DocumentStatus::Processing).await?;

    match run(db, llm, &document, bytes).await {
        Ok(count) => {
            repositories::documents::set_status(db, document.id, DocumentStatus::Ready).await?;
            tracing::info!(document_id = %document.id, chunks = count, "document ingéré");
            Ok(count)
        }
        Err(e) => {
            tracing::warn!(document_id = %document.id, error = %e, "échec de l'ingestion");
            if let Err(status_err) =
                repositories::documents::set_status(db, document.id, DocumentStatus::Failed).await
            {
                tracing::error!(document_id = %document.id, error = %status_err, "statut failed non enregistré");
            }
            Err(e)
        }
    }
}

async fn run(
    db: &DatabaseConnection,
    llm: &dyn LlmClient,
    document: &documents::Model,
    bytes: Vec<u8>,
) -> Result<usize, IngestionError> {
    // Vérifié avant tout appel : un modèle mal configuré ne doit rien écrire.
    check_dimensions(llm.embedding_dimensions())?;

    let source: SourceType = document.source_type.parse()?;

    // L'extraction PDF est du calcul pur et peut être longue : hors du runtime async.
    let chunks = tokio::task::spawn_blocking(move || -> Result<_, RagError> {
        let text = extract_text(&bytes, source)?;
        Ok(chunk_text(&text, &ChunkConfig::default()))
    })
    .await
    .map_err(|_| IngestionError::ExtractionAborted)??;

    let mut rows = Vec::with_capacity(chunks.len());
    for batch in chunks.chunks(EMBED_BATCH) {
        let texts: Vec<String> = batch.iter().map(|c| c.content.clone()).collect();
        let vectors = llm.embed(&texts).await?;
        for (chunk, embedding) in batch.iter().zip(vectors) {
            check_dimensions(embedding.len())?;
            rows.push(NewChunk {
                // Un document de plus de 2^31 fragments n'existe pas en pratique.
                chunk_index: i32::try_from(chunk.index).unwrap_or(i32::MAX),
                content: chunk.content.clone(),
                embedding,
            });
        }
    }
    let count = rows.len();

    // Tout ou rien : les anciens fragments ne disparaissent que si les
    // nouveaux sont bien écrits.
    let txn = db.begin().await?;
    repositories::chunks::delete_for_document(&txn, document.id).await?;
    repositories::chunks::insert_many(&txn, document.id, document.org_id, rows).await?;
    txn.commit().await?;

    Ok(count)
}

fn check_dimensions(got: usize) -> Result<(), IngestionError> {
    if got == EMBEDDING_DIMENSIONS {
        Ok(())
    } else {
        Err(IngestionError::Dimensions {
            expected: EMBEDDING_DIMENSIONS,
            got,
        })
    }
}
