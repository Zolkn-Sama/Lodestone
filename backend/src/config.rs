pub struct Config {
    pub database_url: String,
    /// API compatible OpenAI : Ollama (`http://localhost:11434/v1`) ou OpenAI.
    pub llm_base_url: String,
    /// Clé de l'API LLM (OpenAI) ; inutile pour Ollama.
    pub llm_api_key: Option<String>,
    pub embedding_model: String,
    /// Modèle de génération des réponses (`llama3.1`, `qwen2.5`…).
    pub chat_model: String,
    pub(crate) jwt_secret: String,
    pub app_port: u16,
}

impl Config {
    /// Config construite à la main, sans lire l'environnement (tests d'intégration).
    pub fn new(database_url: String, jwt_secret: String) -> Self {
        Self {
            database_url,
            llm_base_url: "http://localhost:11434/v1".to_string(),
            llm_api_key: None,
            embedding_model: "nomic-embed-text".to_string(),
            chat_model: "llama3.1".to_string(),
            jwt_secret,
            app_port: 0,
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        fn required(key: &str) -> anyhow::Result<String> {
            std::env::var(key)
                .map_err(|_| anyhow::anyhow!("variable d'environnement manquante: {key}"))
        }
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            llm_base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
            llm_api_key: std::env::var("LLM_API_KEY").ok().filter(|k| !k.is_empty()),
            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string()),
            chat_model: std::env::var("CHAT_MODEL").unwrap_or_else(|_| "llama3.1".to_string()),
            jwt_secret: required("JWT_SECRET")?,
            app_port: std::env::var("APP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8090),
        })
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"***") // contient le mot de passe DB
            .field("llm_base_url", &self.llm_base_url)
            .field("llm_api_key", &self.llm_api_key.as_ref().map(|_| "***"))
            .field("embedding_model", &self.embedding_model)
            .field("chat_model", &self.chat_model)
            .field("jwt_secret", &"***")
            .field("app_port", &self.app_port)
            .finish()
    }
}
