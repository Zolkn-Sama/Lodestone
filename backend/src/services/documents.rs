use std::sync::Arc;

use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use crate::{
    dto::documents::DocumentResponse,
    error::AppError,
    events::Events,
    repositories::documents::{self, NewDocument},
    services::{ingestion, quotas},
    ws::protocol::ServerMsg,
};
use lodestone_core::document_status::DocumentStatus;
use lodestone_llm::LlmClient;
use lodestone_rag::SourceType;

/// Longueur maximale d'un titre (caractères).
const MAX_TITLE_CHARS: usize = 255;

pub struct Upload {
    pub org_id: Uuid,
    pub uploaded_by: Uuid,
    pub file_name: String,
    /// Titre choisi ; à défaut, le nom du fichier.
    pub title: Option<String>,
    pub bytes: Vec<u8>,
}

/// Crée le document en `pending` et lance son ingestion en tâche de fond.
/// La réponse part tout de suite : le client suit ensuite le statut.
pub async fn upload(
    db: &DatabaseConnection,
    events: &Events,
    llm: Arc<dyn LlmClient>,
    upload: Upload,
) -> Result<DocumentResponse, AppError> {
    // Certains navigateurs envoient un chemin complet : on ne garde que le nom.
    let file_name = upload
        .file_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();

    let source =
        SourceType::from_filename(&file_name).map_err(|e| AppError::InvalidInput(e.to_string()))?;
    if upload.bytes.is_empty() {
        return Err(AppError::InvalidInput("le fichier est vide".into()));
    }

    let title = upload
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or(file_name);
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(AppError::InvalidInput(format!(
            "titre : {MAX_TITLE_CHARS} caractères maximum"
        )));
    }

    let txn = db.begin().await?;
    quotas::ensure_can_add_document(&txn, upload.org_id).await?;
    let document = documents::create(
        &txn,
        NewDocument {
            org_id: upload.org_id,
            title,
            source_type: source.as_str().to_string(),
            uploaded_by: upload.uploaded_by,
        },
    )
    .await?;
    txn.commit().await?;

    events.publish(
        document.org_id,
        ServerMsg::DocStatus {
            document_id: document.id,
            status: DocumentStatus::Pending,
        },
    );

    // Pas d'attente : l'ingestion journalise et notifie elle-même la suite
    // (`processing`, puis `ready` ou `failed`).
    let db = db.clone();
    let events = events.clone();
    let document_id = document.id;
    let bytes = upload.bytes;
    tokio::spawn(async move {
        let _ = ingestion::ingest(&db, &events, llm.as_ref(), document_id, bytes).await;
    });

    DocumentResponse::try_from(document).map_err(|_| AppError::Internal)
}

pub async fn list(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<DocumentResponse>, AppError> {
    documents::list_for_org(db, org_id)
        .await?
        .into_iter()
        .map(|d| DocumentResponse::try_from(d).map_err(|_| AppError::Internal))
        .collect()
}

/// Supprime un document et ses fragments. Si son ingestion tourne encore,
/// elle échouera à l'écriture (le document n'existe plus) sans rien recréer.
pub async fn delete(db: &DatabaseConnection, document_id: Uuid) -> Result<(), AppError> {
    if documents::delete(db, document_id).await? {
        Ok(())
    } else {
        Err(AppError::NotFound)
    }
}
