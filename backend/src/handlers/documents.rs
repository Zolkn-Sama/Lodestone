use axum::{
    Json,
    extract::{Multipart, State, multipart::MultipartError},
    http::StatusCode,
    response::IntoResponse,
};

use crate::{
    dto::documents::DocumentResponse,
    error::AppError,
    extractors::{
        document_access::DocumentAccess,
        org_member::{Admin, OrgMember},
    },
    services::{self, documents::Upload},
    state::AppState,
};

/// Taille maximale d'un upload (corps multipart complet).
pub const MAX_UPLOAD_BYTES: usize = 20 * 1024 * 1024;

/// POST /api/orgs/{id}/documents — admin. Multipart : `file` (obligatoire,
/// .pdf / .txt / .md) et `title` (optionnel, sinon le nom du fichier).
/// Répond 202 : le document est `pending`, l'ingestion tourne en arrière-plan.
pub async fn upload(
    State(state): State<AppState>,
    member: OrgMember<Admin>,
    mut multipart: Multipart, // en DERNIER : il consomme le corps
) -> Result<impl IntoResponse, AppError> {
    let mut file: Option<(String, Vec<u8>)> = None;
    let mut title = None;

    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name() {
            Some("file") => {
                let name = field.file_name().map(str::to_string).ok_or_else(|| {
                    AppError::InvalidInput("le champ file doit être un fichier".into())
                })?;
                let bytes = field.bytes().await.map_err(multipart_error)?;
                file = Some((name, bytes.to_vec()));
            }
            Some("title") => title = Some(field.text().await.map_err(multipart_error)?),
            _ => {} // champs inconnus ignorés
        }
    }

    let (file_name, bytes) =
        file.ok_or_else(|| AppError::InvalidInput("champ file manquant".into()))?;

    let document = services::documents::upload(
        &state.db,
        state.llm.clone(),
        Upload {
            org_id: member.org_id,
            uploaded_by: member.user_id,
            file_name,
            title,
            bytes,
        },
    )
    .await?;

    Ok((StatusCode::ACCEPTED, Json(document)))
}

/// GET /api/orgs/{id}/documents — membre : titre + statut de chaque document.
pub async fn list(
    State(state): State<AppState>,
    member: OrgMember,
) -> Result<impl IntoResponse, AppError> {
    let documents = services::documents::list(&state.db, member.org_id).await?;
    Ok(Json(documents))
}

/// GET /api/documents/{id} — membre de l'org du document : détail + statut.
pub async fn get(access: DocumentAccess) -> Result<impl IntoResponse, AppError> {
    let document = DocumentResponse::try_from(access.document).map_err(|_| AppError::Internal)?;
    Ok(Json(document))
}

/// DELETE /api/documents/{id} — admin de l'org du document.
pub async fn delete(
    State(state): State<AppState>,
    access: DocumentAccess<Admin>,
) -> Result<impl IntoResponse, AppError> {
    services::documents::delete(&state.db, access.document.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Garde le 413 d'axum quand la limite de taille est dépassée.
fn multipart_error(e: MultipartError) -> AppError {
    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
        AppError::PayloadTooLarge(format!(
            "fichier trop volumineux ({} Mo maximum)",
            MAX_UPLOAD_BYTES / (1024 * 1024)
        ))
    } else {
        AppError::InvalidInput(e.body_text())
    }
}
