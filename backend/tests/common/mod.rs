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
    config::{Config, SovereignPrices, StripeConfig},
    events::Events,
    router::app_router,
    services::ingestion::EMBEDDING_DIMENSIONS,
    state::AppState,
};
use lodestone_billing::{MockPaymentProvider, PaymentProvider};
use lodestone_llm::MockLlmClient;

/// Secret des webhooks Stripe dans les tests.
pub const WEBHOOK_SECRET: &str = "whsec_test";
/// Prix Stripe Pro (mensuel, annuel) configurés dans les tests.
pub const MONTHLY_PRICE: &str = "price_pro_test";
pub const ANNUAL_PRICE: &str = "price_pro_annual_test";
/// Prix Sovereign : licence annuelle et mise en service ponctuelle.
pub const SOVEREIGN_LICENSE_PRICE: &str = "price_sovereign_license_test";
pub const SOVEREIGN_SETUP_PRICE: &str = "price_sovereign_setup_test";

/// Configuration de la facturation d'un serveur de test.
enum Billing {
    Disabled,
    /// Installation auto-hébergée : pas de Stripe, orgs créées en Sovereign.
    SelfHosted,
    /// Stripe configuré, sans prix annuel.
    MonthlyOnly,
    Enabled(MockPaymentProvider),
}
use lodestone_migration::{Migrator, MigratorTrait};

pub struct TestApp {
    router: Router,
    pub db: DatabaseConnection,
    /// Le même client que celui de l'`AppState` : embeddings déterministes, sans réseau.
    pub llm: Arc<MockLlmClient>,
    /// Le même bus que celui de l'`AppState`.
    pub events: Events,
    /// Le fournisseur de paiement de l'`AppState` (facturation active par défaut).
    pub billing: Arc<MockPaymentProvider>,
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
        Self::spawn_with_llm(MockLlmClient::new(EMBEDDING_DIMENSIONS)).await
    }

    /// Comme `spawn`, avec un mock configuré (réponse, lenteur, pannes…).
    pub async fn spawn_with_llm(llm: MockLlmClient) -> Self {
        Self::spawn_full(llm, Billing::Enabled(MockPaymentProvider::new())).await
    }

    /// Serveur sans Stripe configuré : la facturation répond 503.
    pub async fn spawn_without_billing() -> Self {
        Self::spawn_full(MockLlmClient::new(EMBEDDING_DIMENSIONS), Billing::Disabled).await
    }

    /// Installation Sovereign auto-hébergée (NEW_ORG_PLAN=sovereign, sans Stripe).
    pub async fn spawn_self_hosted() -> Self {
        Self::spawn_full(
            MockLlmClient::new(EMBEDDING_DIMENSIONS),
            Billing::SelfHosted,
        )
        .await
    }

    /// Stripe configuré sans prix annuel : seul le mensuel est proposé.
    pub async fn spawn_without_annual() -> Self {
        Self::spawn_full(
            MockLlmClient::new(EMBEDDING_DIMENSIONS),
            Billing::MonthlyOnly,
        )
        .await
    }

    /// Facturation active avec un fournisseur configuré (ex. en panne).
    pub async fn spawn_with_billing(provider: MockPaymentProvider) -> Self {
        Self::spawn_full(
            MockLlmClient::new(EMBEDDING_DIMENSIONS),
            Billing::Enabled(provider),
        )
        .await
    }

    async fn spawn_full(llm: MockLlmClient, billing_mode: Billing) -> Self {
        // Même image que docker-compose : pgvector sera là quand le RAG arrivera.
        let pg = Postgres::default()
            .with_name("pgvector/pgvector")
            .with_tag("pg16")
            .start()
            .await
            .expect("démarrage du conteneur Postgres (Docker est-il lancé ?)");
        let port = pg.get_host_port_ipv4(5432).await.expect("port Postgres");
        let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

        let db = connect_with_retry(&url).await;
        Migrator::up(&db, None).await.expect("migrations");

        let llm = Arc::new(llm);
        let events = Events::default();
        let mut config = Config::new(url, "secret-de-test".into());
        config.app_base_url = "https://app.test".into();
        let annual_offered = !matches!(billing_mode, Billing::MonthlyOnly);
        if matches!(billing_mode, Billing::SelfHosted) {
            config.new_org_plan = lodestone_core::plan::Plan::Sovereign;
        }
        let (billing, billing_enabled) = match billing_mode {
            Billing::Disabled | Billing::SelfHosted => (MockPaymentProvider::new(), false),
            Billing::MonthlyOnly => (MockPaymentProvider::new(), true),
            Billing::Enabled(provider) => (provider, true),
        };
        let billing = Arc::new(billing);
        if billing_enabled {
            config.stripe = Some(StripeConfig {
                secret_key: "sk_test".into(),
                webhook_secret: WEBHOOK_SECRET.into(),
                pro_price_id: MONTHLY_PRICE.into(),
                pro_annual_price_id: annual_offered.then(|| ANNUAL_PRICE.into()),
                sovereign_prices: Some(SovereignPrices {
                    license_price_id: SOVEREIGN_LICENSE_PRICE.into(),
                    setup_price_id: SOVEREIGN_SETUP_PRICE.into(),
                }),
            });
        }
        let state = AppState {
            db: db.clone(),
            config: Arc::new(config),
            llm: llm.clone(),
            events: events.clone(),
            billing: billing_enabled.then(|| billing.clone() as Arc<dyn PaymentProvider>),
        };

        Self {
            router: app_router(state),
            db,
            llm,
            events,
            billing,
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

    /// Sert le router sur un vrai port local (nécessaire pour le WebSocket,
    /// que `oneshot` ne sait pas « upgrader »). Renvoie `127.0.0.1:<port>`.
    pub async fn serve(&self) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let router = self.router.clone();
        tokio::spawn(async move { axum::serve(listener, router).await });
        addr
    }

    /// Envoie une requête construite à la main (en-têtes libres, ex. CORS).
    pub async fn send(&self, req: Request<Body>) -> axum::response::Response {
        self.router.clone().oneshot(req).await.expect("réponse")
    }

    /// Envoie un événement Stripe signé (comme Stripe le ferait) au webhook.
    pub async fn send_webhook(&self, event: &Value) -> (StatusCode, Value) {
        let body = event.to_string().into_bytes();
        let signature =
            lodestone_billing::sign_payload(&body, WEBHOOK_SECRET, chrono::Utc::now().timestamp());
        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/billing/webhook")
            .header("stripe-signature", signature)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .expect("requête");
        let res = self.send(req).await;
        let status = res.status();
        let bytes = res.into_body().collect().await.expect("corps").to_bytes();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
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

/// Sous forte charge (beaucoup de conteneurs démarrés en parallèle), le port
/// publié par Docker peut refuser les toutes premières connexions : on réessaie
/// quelques secondes avant d'abandonner.
async fn connect_with_retry(url: &str) -> DatabaseConnection {
    let mut last_error = None;
    for _ in 0..20 {
        match Database::connect(url).await {
            Ok(db) => return db,
            Err(e) => {
                last_error = Some(e);
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
        }
    }
    panic!("connexion Postgres impossible après 5 s : {last_error:?}");
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

// ----- WebSocket ------------------------------------------------------------------

pub type WsSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

pub async fn ws_connect(addr: std::net::SocketAddr, token: &str) -> WsSocket {
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws?token={token}"))
        .await
        .expect("connexion WebSocket");
    socket
}

pub async fn ws_send(socket: &mut WsSocket, value: Value) {
    use futures::SinkExt;
    socket
        .send(tokio_tungstenite::tungstenite::Message::text(
            value.to_string(),
        ))
        .await
        .expect("envoi");
}

/// Prochain message texte du serveur, en JSON, ou `None` si rien n'arrive
/// avant `timeout`.
pub async fn ws_try_recv(socket: &mut WsSocket, timeout: std::time::Duration) -> Option<Value> {
    use futures::StreamExt;
    tokio::time::timeout(timeout, async {
        loop {
            match socket.next().await {
                Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                    return serde_json::from_str(&text).expect("JSON");
                }
                Some(Ok(_)) => {}
                other => panic!("socket fermée : {other:?}"),
            }
        }
    })
    .await
    .ok()
}

/// Prochain message texte du serveur, en JSON (échoue après 2 s).
pub async fn ws_recv(socket: &mut WsSocket) -> Value {
    ws_try_recv(socket, std::time::Duration::from_secs(2))
        .await
        .expect("réponse du serveur sous 2 s")
}
