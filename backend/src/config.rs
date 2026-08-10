
#[allow(dead_code)] // jwt_secret / ollama_base_url seront lus quand les services arriveront
pub struct Config {
    pub database_url: String,
    ollama_base_url: String,
    pub(crate) jwt_secret: String,
    pub app_port: u16,
}

impl Config {
     pub fn from_env() -> anyhow::Result<Self> {
         fn required(key: &str) -> anyhow::Result<String> {
            std::env::var(key)
                .map_err(|_| anyhow::anyhow!("variable d'environnement manquante: {key}"))
        }
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            ollama_base_url: std::env::var("OLLAMA_BASE_URL")
                .unwrap_or_else(|_| "http://localhost:11434/v1".to_string()),
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
            .field("database_url", &"***")   // contient le mot de passe DB
            .field("ollama_base_url", &self.ollama_base_url)
            .field("jwt_secret", &"***")
            .field("app_port", &self.app_port)
            .finish()
    }
}