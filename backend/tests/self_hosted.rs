//! BILL-04 : déploiement Sovereign auto-hébergé et bascule de modèle.

mod common;

use axum::http::StatusCode;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::json;
use uuid::Uuid;

use common::TestApp;
use lodestone_backend::services::ingestion;

#[tokio::test]
async fn installation_auto_hebergee_sans_limite_ni_stripe() {
    let app = TestApp::spawn_self_hosted().await;
    let owner = app.register("owner").await;

    // Orgs créées directement en Sovereign, sans la limite « 1 org gratuite ».
    let mut orgs = Vec::new();
    for name in ["Juridique", "RH"] {
        let (status, body) = app
            .post("/api/orgs", &owner.token, json!({ "name": name }))
            .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(body["plan"], "sovereign");
        orgs.push(
            body["id"]
                .as_str()
                .expect("id")
                .parse::<Uuid>()
                .expect("uuid"),
        );
    }

    // Pas de quotas : plus de 3 membres sans erreur 402.
    for i in 0..4 {
        let user = app.register(&format!("m{i}")).await;
        app.add_member(orgs[0], &owner, &user, "member").await;
    }
    let (_, usage) = app
        .get(&format!("/api/orgs/{}/usage", orgs[0]), &owner.token)
        .await;
    assert_eq!(usage["members"], json!({ "used": 5, "limit": null }));

    // Facturation absente : aucun appel possible vers Stripe.
    let (_, billing) = app
        .get(&format!("/api/orgs/{}/billing", orgs[0]), &owner.token)
        .await;
    assert_eq!(billing["billing_enabled"], false);
    assert_eq!(billing["sovereign"]["checkout_available"], false);
}

#[tokio::test]
async fn le_modele_d_embedding_est_enregistre_par_document() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let (_, body) = app
        .upload(
            org,
            &owner.token,
            "rh.txt",
            b"Vingt-cinq jours de conges.",
            None,
        )
        .await;
    let doc: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");

    let row = app
        .db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "SELECT embedding_model FROM documents WHERE id = $1",
            [doc.into()],
        ))
        .await
        .expect("requête")
        .expect("ligne");
    let model: Option<String> = row.try_get("", "embedding_model").expect("modèle");
    assert_eq!(model.as_deref(), Some("mock-embedding"));

    // Même modèle : rien à signaler.
    assert!(
        ingestion::documents_from_other_models(&app.db, "mock-embedding")
            .await
            .expect("requête")
            .is_empty()
    );
}

#[tokio::test]
async fn bascule_de_modele_documents_a_reuploader() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    for name in ["a.txt", "b.txt"] {
        let (_, body) = app.upload(org, &owner.token, name, b"Contenu.", None).await;
        let doc: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
        assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    }
    // Un document ingéré avant le suivi du modèle (colonne vide).
    app.db
        .execute(Statement::from_string(
            app.db.get_database_backend(),
            "UPDATE documents SET embedding_model = NULL WHERE title = 'b.txt'",
        ))
        .await
        .expect("maj");

    // Bascule vers OpenAI : les deux documents sont à ré-uploader.
    let others = ingestion::documents_from_other_models(&app.db, "text-embedding-3-small")
        .await
        .expect("requête");
    assert_eq!(
        others,
        vec![
            ("(inconnu)".to_string(), 1),
            ("mock-embedding".to_string(), 1)
        ]
    );
    // Les documents en échec ne comptent pas.
    let (_, body) = app
        .upload(org, &owner.token, "casse.pdf", b"pas un pdf", None)
        .await;
    let broken: Uuid = body["id"].as_str().expect("id").parse().expect("uuid");
    assert_eq!(app.wait_for_ingestion(broken).await, "failed");
    assert_eq!(
        ingestion::documents_from_other_models(&app.db, "text-embedding-3-small")
            .await
            .expect("requête")
            .len(),
        2
    );
}
