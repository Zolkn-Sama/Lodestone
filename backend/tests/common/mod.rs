//! Harnais des tests d'intégration : un Postgres (pgvector) jetable par test,
//! migrations appliquées, et le vrai router appelé via `oneshot` (sans réseau).

// Chaque fichier de test n'utilise qu'une partie des helpers.
#![allow(dead_code)]

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sea_orm::{Database, DatabaseConnection};
use serde_json::{Value, json};
use testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use testcontainers_modules::postgres::Postgres;
use tower::ServiceExt;
use uuid::Uuid;

use lodestone_backend::{
    config::Config, router::app_router, services::ingestion::EMBEDDING_DIMENSIONS, state::AppState,
};
use lodestone_llm::MockLlmClient;
use lodestone_migration::{Migrator, MigratorTrait};

pub struct TestApp {
    router: Router,
    pub db: DatabaseConnection,
    /// Le même client que celui de l'`AppState` : embeddings déterministes, sans réseau.
    pub llm: Arc<MockLlmClient>,
    // Gardé en vie tant que le test tourne ; le conteneur est supprimé au drop.
    _pg: ContainerAsync<Postgres>,
}

/// Un utilisateur inscrit, avec son access token.
#[derive(Clone)]
pub struct TestUser {
    pub id: Uuid,
    pub email: String,
    pub token: String,
}

impl TestApp {
    pub async fn spawn() -> Self {
        // Même image que docker-compose : pgvector sera là quand le RAG arrivera.
        let pg = Postgres::default()
            .with_name("pgvector/pgvector")
            .with_tag("pg16")
            .start()
            .await
            .expect("démarrage du conteneur Postgres (Docker est-il lancé ?)");
        let port = pg.get_host_port_ipv4(5432).await.expect("port Postgres");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

        let db = Database::connect(&url).await.expect("connexion Postgres");
        Migrator::up(&db, None).await.expect("migrations");

        let llm = Arc::new(MockLlmClient::new(EMBEDDING_DIMENSIONS));
        let state = AppState {
            db: db.clone(),
            config: Arc::new(Config::new(url, "secret-de-test".into())),
            llm: llm.clone(),
        };

        Self {
            router: app_router(state),
            db,
            llm,
            _pg: pg,
        }
    }

    /// Envoie une requête JSON. Renvoie le statut et le corps (`Null` si vide
    /// ou si ce n'est pas du JSON, ex. un rejet de désérialisation d'axum).
    pub async fn request(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        match body {
            Some(b) => {
                self.request_raw(
                    method,
                    uri,
                    token,
                    Some("application/json"),
                    b.to_string().into_bytes(),
                )
                .await
            }
            None => self.request_raw(method, uri, token, None, Vec::new()).await,
        }
    }

    /// Comme `request`, avec un corps et un Content-Type quelconques.
    pub async fn request_raw(
        &self,
        method: Method,
        uri: &str,
        token: Option<&str>,
        content_type: Option<&str>,
        body: Vec<u8>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(uri);
        if let Some(t) = token {
            req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
        }
        if let Some(ct) = content_type {
            req = req.header(header::CONTENT_TYPE, ct);
        }
        let req = req.body(Body::from(body)).expect("requête");

        let res = self.router.clone().oneshot(req).await.expect("réponse");
        let status = res.status();
        let bytes = res.into_body().collect().await.expect("corps").to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, json)
    }

    /// Upload multipart vers `/api/orgs/{org}/documents` : champ `file`
    /// (`file_name`, `content`) et champ `title` optionnel.
    pub async fn upload(
        &self,
        org: Uuid,
        token: &str,
        file_name: &str,
        content: &[u8],
        title: Option<&str>,
    ) -> (StatusCode, Value) {
        let mut parts = vec![(
            format!("name=\"file\"; filename=\"{file_name}\""),
            content.to_vec(),
        )];
        if let Some(t) = title {
            parts.push(("name=\"title\"".to_string(), t.as_bytes().to_vec()));
        }
        let (content_type, body) = multipart_body(&parts);
        self.request_raw(
            Method::POST,
            &format!("/api/orgs/{org}/documents"),
            Some(token),
            Some(&content_type),
            body,
        )
        .await
    }

    /// Attend que l'ingestion (tâche de fond) sorte de `pending`/`processing`.
    pub async fn wait_for_ingestion(&self, document_id: Uuid) -> String {
        for _ in 0..200 {
            let status =
                lodestone_backend::repositories::documents::find_by_id(&self.db, document_id)
                    .await
                    .expect("requête")
                    .expect("document")
                    .status;
            if status != "pending" && status != "processing" {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("ingestion du document {document_id} non terminée après 10 s");
    }

    pub async fn get(&self, uri: &str, token: &str) -> (StatusCode, Value) {
        self.request(Method::GET, uri, Some(token), None).await
    }

    pub async fn post(&self, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
        self.request(Method::POST, uri, Some(token), Some(body))
            .await
    }

    /// Inscrit un utilisateur à l'email unique (`<prefix>-<uuid>@test.io`).
    pub async fn register(&self, prefix: &str) -> TestUser {
        let email = format!("{prefix}-{}@test.io", Uuid::new_v4());
        let (status, body) = self
            .request(
                Method::POST,
                "/api/auth/register",
                None,
                Some(json!({ "email": email, "password": "password123" })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "register : {body}");
        let token = body["access_token"]
            .as_str()
            .expect("access_token")
            .to_string();

        let (_, me) = self.get("/api/auth/me", &token).await;
        let id = me["id"].as_str().expect("id").parse().expect("uuid");
        TestUser { id, email, token }
    }

    pub async fn create_org(&self, owner: &TestUser) -> Uuid {
        let (status, body) = self
            .post("/api/orgs", &owner.token, json!({ "name": "Acme" }))
            .await;
        assert_eq!(status, StatusCode::CREATED, "create org : {body}");
        body["id"].as_str().expect("id").parse().expect("uuid")
    }

    /// Crée une invitation et renvoie son jeton (échoue si statut != 201).
    pub async fn invite(&self, org: Uuid, inviter: &TestUser, email: &str, role: &str) -> String {
        let (status, body) = self
            .post(
                &format!("/api/orgs/{org}/invitations"),
                &inviter.token,
                json!({ "email": email, "role": role }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "invite : {body}");
        body["token"].as_str().expect("token").to_string()
    }

    /// Fait entrer `user` dans l'org avec `role`, par le vrai parcours
    /// invitation + acceptation (l'owner invite).
    pub async fn add_member(&self, org: Uuid, owner: &TestUser, user: &TestUser, role: &str) {
        let token = self.invite(org, owner, &user.email, role).await;
        let (status, body) = self
            .post(
                "/api/invitations/accept",
                &user.token,
                json!({ "token": token }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "accept : {body}");
    }
}

/// Construit un corps multipart/form-data. `parts` : (Content-Disposition sans
/// le préfixe `form-data; `, contenu). Renvoie (Content-Type, corps).
pub fn multipart_body(parts: &[(String, Vec<u8>)]) -> (String, Vec<u8>) {
    let boundary = "----lodestone-test-boundary";
    let mut body = Vec::new();
    for (disposition, content) in parts {
        body.extend(
            format!("--{boundary}\r\nContent-Disposition: form-data; {disposition}\r\n\r\n")
                .as_bytes(),
        );
        body.extend(content);
        body.extend(b"\r\n");
    }
    body.extend(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}
