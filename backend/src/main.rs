use sea_orm::{ConnectOptions, Database};
use std::{sync::Arc, time::Duration};

use lodestone_backend::config::Config;
use lodestone_backend::events::Events;
use lodestone_backend::router;
use lodestone_backend::services::ingestion::EMBEDDING_DIMENSIONS;
use lodestone_backend::state::AppState;
use lodestone_billing::{PaymentProvider, StripeClient};
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

    // Ingestions interrompues par l'arrêt précédent : documents -> failed.
    lodestone_backend::services::ingestion::recover_interrupted(&db).await?;

    // 4. Client LLM (embeddings + génération) : Ollama par défaut, OpenAI si LLM_API_KEY est fourni.
    let llm = OpenAiCompatibleClient::new(
        config.llm_base_url.clone(),
        config.llm_api_key.clone(),
        config.embedding_model.clone(),
        EMBEDDING_DIMENSIONS,
    )?
    .with_chat_model(config.chat_model.clone());

    // Facturation : seulement si Stripe est configuré.
    let billing: Option<Arc<dyn PaymentProvider>> = match &config.stripe {
        Some(stripe) => Some(Arc::new(StripeClient::new(
            stripe.secret_key.clone(),
            stripe.pro_price_id.clone(),
        )?)),
        None => {
            info!("Stripe non configuré : facturation désactivée");
            None
        }
    };
    if let Some(provider) = &billing {
        // Le prix configuré chez Stripe doit correspondre au tarif de la plateforme.
        lodestone_backend::services::billing::verify_stripe_price(
            provider.as_ref(),
            config.pro_pricing,
        )
        .await;
    }

    // 5. État partagé + router.
    let state = AppState {
        billing,
        db,
        config: Arc::new(config),
        llm: Arc::new(llm),
        events: Events::default(),
    };
    let port = state.config.app_port;
    let app = router::app_router(state);

    // 6. Écoute et sert.
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    info!(%port, "🚀 Lodestone à l'écoute sur http://0.0.0.0:{port}");
    axum::serve(listener, app).await?;

    Ok(())
}
