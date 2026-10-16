//! DOC-01 : upload d'un document (multipart) + ingestion en tâche de fond.

mod common;

use axum::http::{Method, StatusCode};
use sea_orm::{ConnectionTrait, Statement};
use serde_json::Value;
use uuid::Uuid;

use common::{TestApp, multipart_body};

fn doc_id(body: &Value) -> Uuid {
    body["id"].as_str().expect("id").parse().expect("uuid")
}

/// Un texte assez long pour donner plusieurs fragments.
fn long_text() -> String {
    (0..20)
        .map(|i| {
            let words: Vec<String> = (0..60).map(|w| format!("mot{i}x{w}")).collect();
            format!("Paragraphe {i}. {}", words.join(" "))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

async fn chunks_in_org(app: &TestApp, doc: Uuid, org: Uuid) -> i64 {
    app.db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "SELECT count(*) AS n FROM document_chunks WHERE document_id = $1 AND org_id = $2",
            [doc.into(), org.into()],
        ))
        .await
        .expect("requête")
        .expect("ligne")
        .try_get("", "n")
        .expect("n")
}

#[tokio::test]
async fn upload_txt_puis_ingestion() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let (status, body) = app
        .upload(
            org,
            &owner.token,
            "procedures.txt",
            long_text().as_bytes(),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(body["status"], "pending");
    assert_eq!(
        body["title"], "procedures.txt",
        "titre par défaut = nom du fichier"
    );
    assert_eq!(body["source_type"], "txt");

    let doc = doc_id(&body);
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    assert!(chunks_in_org(&app, doc, org).await > 1);

    // Le document est rattaché à la bonne org et au bon auteur.
    let row = lodestone_backend::repositories::documents::find_by_id(&app.db, doc)
        .await
        .expect("requête")
        .expect("document");
    assert_eq!(row.org_id, org);
    assert_eq!(row.uploaded_by, owner.id);
}

#[tokio::test]
async fn upload_md_et_pdf_avec_titre() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let (status, body) = app
        .upload(
            org,
            &owner.token,
            "conges.MD",
            b"# Conges\n\n25 jours par an.",
            Some("  Politique de congés  "),
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(
        body["title"], "Politique de congés",
        "titre fourni, espaces retirés"
    );
    assert_eq!(body["source_type"], "md");
    assert_eq!(app.wait_for_ingestion(doc_id(&body)).await, "ready");

    let (status, body) = app
        .upload(
            org,
            &owner.token,
            "C:\\Users\\rh\\reglement.pdf",
            &minimal_pdf("Reglement interieur"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{body}");
    assert_eq!(
        body["title"], "reglement.pdf",
        "chemin du navigateur retiré"
    );
    let doc = doc_id(&body);
    assert_eq!(app.wait_for_ingestion(doc).await, "ready");
    assert_eq!(chunks_in_org(&app, doc, org).await, 1);
}

#[tokio::test]
async fn pdf_corrompu_accepte_puis_failed() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let (status, body) = app
        .upload(
            org,
            &owner.token,
            "casse.pdf",
            b"%PDF-1.4 pas vraiment",
            None,
        )
        .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let doc = doc_id(&body);
    assert_eq!(app.wait_for_ingestion(doc).await, "failed");
    assert_eq!(chunks_in_org(&app, doc, org).await, 0);
}

#[tokio::test]
async fn reserve_aux_admins() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let member = app.register("member").await;
    let outsider = app.register("outsider").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;
    app.add_member(org, &owner, &member, "member").await;

    for (user, expected) in [
        (&owner, StatusCode::ACCEPTED),
        (&admin, StatusCode::ACCEPTED),
        (&member, StatusCode::FORBIDDEN),
        (&outsider, StatusCode::NOT_FOUND),
    ] {
        let (status, _) = app
            .upload(org, &user.token, "a.txt", b"Bonjour.", None)
            .await;
        assert_eq!(status, expected, "{}", user.email);
    }

    let (content_type, body) =
        multipart_body(&[("name=\"file\"; filename=\"a.txt\"".into(), b"x".to_vec())]);
    let (status, _) = app
        .request_raw(
            Method::POST,
            &format!("/api/orgs/{org}/documents"),
            None,
            Some(&content_type),
            body,
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Rien n'a été créé pour les refus.
    let n: i64 = app
        .db
        .query_one(Statement::from_string(
            app.db.get_database_backend(),
            "SELECT count(*) AS n FROM documents",
        ))
        .await
        .expect("requête")
        .expect("ligne")
        .try_get("", "n")
        .expect("n");
    assert_eq!(n, 2);
}

#[tokio::test]
async fn requetes_invalides() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let uri = format!("/api/orgs/{org}/documents");

    // Format non supporté, sans extension, fichier vide, titre trop long.
    for (name, content, title) in [
        ("contrat.docx", &b"x"[..], None),
        ("sans_extension", b"x", None),
        ("vide.txt", b"", None),
        ("a.txt", b"x", Some("t".repeat(256))),
    ] {
        let (status, body) = app
            .upload(org, &owner.token, name, content, title.as_deref())
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name} : {body}");
    }

    // Pas de champ `file`.
    let (ct, body) = multipart_body(&[("name=\"title\"".into(), b"Seul".to_vec())]);
    let (status, _) = app
        .request_raw(Method::POST, &uri, Some(&owner.token), Some(&ct), body)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Champ `file` qui n'est pas un fichier (pas de filename).
    let (ct, body) = multipart_body(&[("name=\"file\"".into(), b"texte".to_vec())]);
    let (status, _) = app
        .request_raw(Method::POST, &uri, Some(&owner.token), Some(&ct), body)
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Pas du multipart du tout.
    let (status, _) = app
        .request_raw(
            Method::POST,
            &uri,
            Some(&owner.token),
            Some("application/json"),
            b"{}".to_vec(),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn fichier_trop_volumineux() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let big = vec![b'a'; 21 * 1024 * 1024];
    let (status, body) = app.upload(org, &owner.token, "gros.txt", &big, None).await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "{body}");
}

/// PDF d'une page avec `text` (police Helvetica), xref correcte.
fn minimal_pdf(text: &str) -> Vec<u8> {
    let content = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>"
            .to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_string(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend(format!("{} 0 obj\n{obj}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
    for off in offsets {
        pdf.extend(format!("{off:010} 00000 n \n").as_bytes());
    }
    pdf.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}
