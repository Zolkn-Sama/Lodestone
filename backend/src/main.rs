

use std::{sync::Arc, time::Duration};
use sea_orm::{ConnectOptions, Database};
//use Testcontainers::bollard::config;


use tokio::net::TcpListener;
use tracing::info;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use lodestone_backend::router;
use lodestone_backend::config::Config;
use lodestone_backend::state::AppState;

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    dotenvy::dotenv().ok();
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,lodestone=debug")))
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
    
    // 4. État partagé + router.
    let state = AppState { db, config: Arc::new(config) };
    let port = state.config.app_port;
    let app = router::app_router(state);

    // 5. Écoute et sert.
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