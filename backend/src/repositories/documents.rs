use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    Set,
};
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
        uploaded_by: Set(Some(new.uploaded_by)),
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

/// Les documents d'une org, du plus récent au plus ancien.
pub async fn list_for_org<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<Vec<documents::Model>, DbErr> {
    documents::Entity::find()
        .filter(documents::Column::OrgId.eq(org_id))
        .order_by_desc(documents::Column::CreatedAt)
        .order_by_desc(documents::Column::Id) // ordre stable à created_at égal
        .all(conn)
        .await
}

/// Supprime le document ; ses fragments partent avec (FK `ON DELETE CASCADE`).
/// Renvoie `true` si une ligne a été supprimée.
pub async fn delete<C: ConnectionTrait>(conn: &C, id: Uuid) -> Result<bool, DbErr> {
    let res = documents::Entity::delete_by_id(id).exec(conn).await?;
    Ok(res.rows_affected > 0)
}

/// Passe en `failed` les documents restés `pending` ou `processing` : leur
/// ingestion a été interrompue (redémarrage du serveur) et le fichier n'est
/// pas conservé, elle ne reprendra pas. Renvoie le nombre de documents touchés.
pub async fn fail_interrupted<C: ConnectionTrait>(conn: &C) -> Result<u64, DbErr> {
    let res = documents::Entity::update_many()
        .col_expr(
            documents::Column::Status,
            sea_orm::sea_query::Expr::value(DocumentStatus::Failed.as_str()),
        )
        .filter(documents::Column::Status.is_in([
            DocumentStatus::Pending.as_str(),
            DocumentStatus::Processing.as_str(),
        ]))
        .exec(conn)
        .await?;
    Ok(res.rows_affected)
}

/// Note le modèle d'embedding qui a produit les fragments du document.
pub async fn set_embedding_model<C: ConnectionTrait>(
    conn: &C,
    id: Uuid,
    model: &str,
) -> Result<(), DbErr> {
    conn.execute(sea_orm::Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE documents SET embedding_model = $1 WHERE id = $2",
        [model.into(), id.into()],
    ))
    .await?;
    Ok(())
}

/// Documents prêts dont le modèle d'embedding diffère de `model`, par modèle
/// (`(inconnu)` pour ceux ingérés avant le suivi).
pub async fn count_ready_by_other_embedding_model<C: ConnectionTrait>(
    conn: &C,
    model: &str,
) -> Result<Vec<(String, i64)>, DbErr> {
    let rows = conn
        .query_all(sea_orm::Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT COALESCE(embedding_model, '(inconnu)') AS model, count(*) AS n
             FROM documents
             WHERE status = 'ready' AND embedding_model IS DISTINCT FROM $1
             GROUP BY 1 ORDER BY 1",
            [model.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|r| Ok((r.try_get("", "model")?, r.try_get("", "n")?)))
        .collect()
}
