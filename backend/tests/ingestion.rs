//! ING-02 : extraction -> découpage -> embeddings (mock) -> document_chunks.

mod common;

use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value as DbValue};
use uuid::Uuid;

use common::TestApp;
use lodestone_backend::{
    repositories::documents::{self, NewDocument},
    services::ingestion::{self, EMBEDDING_DIMENSIONS, IngestionError},
};
use lodestone_llm::{LlmClient, MockLlmClient};

/// Un utilisateur, une org et un document `pending` au format `source_type`.
async fn seed_document(app: &TestApp, source_type: &str) -> (Uuid, Uuid) {
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let doc = documents::create(
        &app.db,
        NewDocument {
            org_id: org,
            title: format!("doc.{source_type}"),
            source_type: source_type.to_string(),
            uploaded_by: owner.id,
        },
    )
    .await
    .expect("création du document");
    (org, doc.id)
}

/// Texte de ~`paragraphs` x 100 tokens, découpé en plusieurs fragments.
fn long_text(paragraphs: usize) -> String {
    (0..paragraphs)
        .map(|i| {
            let words: Vec<String> = (0..60).map(|w| format!("mot{i}x{w}")).collect();
            format!("Paragraphe {i}. {}", words.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn count(db: &DatabaseConnection, sql: &str, values: Vec<DbValue>) -> i64 {
    db.query_one(Statement::from_sql_and_values(
        db.get_database_backend(),
        sql,
        values,
    ))
    .await
    .expect("requête")
    .expect("ligne")
    .try_get("", "n")
    .expect("n")
}

async fn status(app: &TestApp, doc: Uuid) -> String {
    documents::find_by_id(&app.db, doc)
        .await
        .expect("requête")
        .expect("document")
        .status
}

async fn chunk_count(app: &TestApp, doc: Uuid) -> i64 {
    count(
        &app.db,
        "SELECT count(*) AS n FROM document_chunks WHERE document_id = $1",
        vec![doc.into()],
    )
    .await
}

#[tokio::test]
async fn ingestion_complete() {
    let app = TestApp::spawn().await;
    let (org, doc) = seed_document(&app, "txt").await;
    assert_eq!(status(&app, doc).await, "pending");

    let stored = ingestion::ingest(&app.db, app.llm.as_ref(), doc, long_text(20).into_bytes())
        .await
        .expect("ingestion");

    assert!(stored > 1, "texte long -> plusieurs fragments ({stored})");
    assert_eq!(status(&app, doc).await, "ready");
    assert_eq!(chunk_count(&app, doc).await, stored as i64);

    // Chaque fragment porte l'org (isolation tenant) et un vecteur de 768.
    let n = count(
        &app.db,
        "SELECT count(*) AS n FROM document_chunks
         WHERE document_id = $1 AND org_id = $2 AND vector_dims(embedding) = 768",
        vec![doc.into(), org.into()],
    )
    .await;
    assert_eq!(n, stored as i64);

    // Indices 0..n-1 sans trou.
    let n = count(
        &app.db,
        "SELECT count(DISTINCT chunk_index) AS n FROM document_chunks
         WHERE document_id = $1 AND chunk_index BETWEEN 0 AND $2",
        vec![doc.into(), (stored as i32 - 1).into()],
    )
    .await;
    assert_eq!(n, stored as i64);
}

#[tokio::test]
async fn le_vecteur_stocke_est_celui_du_fragment() {
    let app = TestApp::spawn().await;
    let (org, doc) = seed_document(&app, "md").await;
    ingestion::ingest(&app.db, app.llm.as_ref(), doc, long_text(20).into_bytes())
        .await
        .expect("ingestion");

    // On prend un fragment, on recalcule son vecteur avec le mock, et la
    // recherche cosinus pgvector (filtrée par org) doit le classer premier.
    let row = app
        .db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "SELECT id, content FROM document_chunks WHERE document_id = $1 AND chunk_index = 1",
            [doc.into()],
        ))
        .await
        .expect("requête")
        .expect("fragment 1");
    let id: Uuid = row.try_get("", "id").expect("id");
    let content: String = row.try_get("", "content").expect("content");

    let query = pgvector::Vector::from(app.llm.vector_for(&content));
    let best = app
        .db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "SELECT id, 1 - (embedding <=> $1) AS score FROM document_chunks
             WHERE org_id = $2 ORDER BY embedding <=> $1 LIMIT 1",
            [query.into(), org.into()],
        ))
        .await
        .expect("recherche")
        .expect("résultat");
    assert_eq!(best.try_get::<Uuid>("", "id").expect("id"), id);
    let score: f64 = best.try_get("", "score").expect("score");
    assert!(score > 0.999, "similarité cosinus {score}");
}

#[tokio::test]
async fn reingestion_remplace_les_fragments() {
    let app = TestApp::spawn().await;
    let (_, doc) = seed_document(&app, "txt").await;

    let first = ingestion::ingest(&app.db, app.llm.as_ref(), doc, long_text(20).into_bytes())
        .await
        .expect("1re ingestion");
    let second = ingestion::ingest(&app.db, app.llm.as_ref(), doc, b"Texte court.".to_vec())
        .await
        .expect("2e ingestion");

    assert!(first > 1);
    assert_eq!(second, 1);
    assert_eq!(chunk_count(&app, doc).await, 1);
}

#[tokio::test]
async fn fichier_illisible_passe_en_failed() {
    let app = TestApp::spawn().await;
    let (_, txt) = seed_document(&app, "txt").await;
    let (_, pdf) = seed_document(&app, "pdf").await;

    let err = ingestion::ingest(&app.db, app.llm.as_ref(), txt, vec![0xff, 0xfe, 0x00]).await;
    assert!(matches!(err, Err(IngestionError::Rag(_))));
    assert_eq!(status(&app, txt).await, "failed");

    let err = ingestion::ingest(&app.db, app.llm.as_ref(), pdf, b"pas un pdf".to_vec()).await;
    assert!(matches!(err, Err(IngestionError::Rag(_))));
    assert_eq!(status(&app, pdf).await, "failed");
    assert_eq!(chunk_count(&app, pdf).await, 0);
}

#[tokio::test]
async fn panne_du_llm_passe_en_failed_sans_rien_ecrire() {
    let app = TestApp::spawn().await;
    let (_, doc) = seed_document(&app, "txt").await;

    // Une première ingestion réussie…
    ingestion::ingest(&app.db, app.llm.as_ref(), doc, long_text(5).into_bytes())
        .await
        .expect("ingestion");
    let before = chunk_count(&app, doc).await;

    // … puis le LLM tombe : statut failed, anciens fragments conservés.
    let failing = MockLlmClient::failing(EMBEDDING_DIMENSIONS);
    let err = ingestion::ingest(&app.db, &failing, doc, long_text(20).into_bytes()).await;
    assert!(matches!(err, Err(IngestionError::Llm(_))));
    assert_eq!(status(&app, doc).await, "failed");
    assert_eq!(chunk_count(&app, doc).await, before);
}

#[tokio::test]
async fn mauvaise_dimension_refusee_avant_tout_appel() {
    let app = TestApp::spawn().await;
    let (_, doc) = seed_document(&app, "txt").await;

    let wrong = MockLlmClient::new(384);
    assert_eq!(wrong.embedding_dimensions(), 384);
    let err = ingestion::ingest(&app.db, &wrong, doc, b"Bonjour.".to_vec()).await;
    assert!(matches!(
        err,
        Err(IngestionError::Dimensions {
            expected: 768,
            got: 384
        })
    ));
    assert_eq!(status(&app, doc).await, "failed");
}

#[tokio::test]
async fn format_inconnu_et_document_absent() {
    let app = TestApp::spawn().await;
    let (_, doc) = seed_document(&app, "docx").await;

    let err = ingestion::ingest(&app.db, app.llm.as_ref(), doc, b"x".to_vec()).await;
    assert!(matches!(err, Err(IngestionError::Rag(_))));
    assert_eq!(status(&app, doc).await, "failed");

    let err = ingestion::ingest(&app.db, app.llm.as_ref(), Uuid::new_v4(), b"x".to_vec()).await;
    assert!(matches!(err, Err(IngestionError::DocumentNotFound)));
}

#[tokio::test]
async fn suppressions_en_cascade() {
    let app = TestApp::spawn().await;
    let (org, doc) = seed_document(&app, "txt").await;
    ingestion::ingest(&app.db, app.llm.as_ref(), doc, long_text(10).into_bytes())
        .await
        .expect("ingestion");
    assert!(chunk_count(&app, doc).await > 0);

    // Supprimer l'org supprime ses documents, donc leurs fragments.
    app.db
        .execute(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "DELETE FROM organizations WHERE id = $1",
            [org.into()],
        ))
        .await
        .expect("suppression");
    assert_eq!(chunk_count(&app, doc).await, 0);
    assert!(
        documents::find_by_id(&app.db, doc)
            .await
            .expect("requête")
            .is_none()
    );
}
