use sea_orm::{ConnectOptions, Database};
use std::{sync::Arc, time::Duration};
//use Testcontainers::bollard::config;

use lodestone_backend::config::Config;
use lodestone_backend::events::Events;
use lodestone_backend::router;
use lodestone_backend::services::ingestion::EMBEDDING_DIMENSIONS;
use lodestone_backend::state::AppState;
use lodestone_llm::OpenAiCompatibleClient;
use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,lodestone=debug")),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 2. Configuration depuis l'environnement.
    let config = Config::from_env()?;
    info!(port = config.app_port, "Config load with success !");

    // 3. Connexion Postgres (fail-fast : lance `docker compose up -d db` d'abord).
    let mut opt = ConnectOptions::new(config.database_url.clone());
    opt.max_connections(10)
        .connect_timeout(Duration::from_secs(5))
        .sqlx_logging(false);
    let db = Database::connect(opt).await?;

    // 4. Client LLM (embeddings) : Ollama par défaut, OpenAI si LLM_API_KEY est fourni.
    let llm = OpenAiCompatibleClient::new(
        config.llm_base_url.clone(),
        config.llm_api_key.clone(),
        config.embedding_model.clone(),
        EMBEDDING_DIMENSIONS,
    )?;

    // 5. État partagé + router.
    let state = AppState {
        db,
        config: Arc::new(config),
        llm: Arc::new(llm),
        events: Events::default(),
    };
    let port = state.config.app_port;
    let app = router::app_router(state);

    // 6. Écoute et sert.
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!("🚀 Lodestone v1: Fondation");
    info!("   Server starting...");
    info!("   Loading config...");
    info!("   Connexion to database...");
    info!("   Connection established !");
    info!("   Server open on http://0.0.0.0:{port}");
    println!();
    info!("📝 Current endpoints:");
    info!("   GET  /        - Hello World");
    info!("   GET  /health  - Health check");
    println!();
    axum::serve(listener, app).await?;

    Ok(())
}
