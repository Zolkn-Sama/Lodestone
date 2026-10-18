use axum::http::HeaderValue;

/// Origine autorisée par défaut : le front Next.js en développement.
pub const DEFAULT_CORS_ORIGIN: &str = "http://localhost:3000";

pub struct Config {
    pub database_url: String,
    /// Origines (navigateur) autorisées à appeler l'API.
    pub cors_allowed_origins: Vec<HeaderValue>,
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
            cors_allowed_origins: vec![HeaderValue::from_static(DEFAULT_CORS_ORIGIN)],
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
            cors_allowed_origins: parse_origins(
                &std::env::var("CORS_ALLOWED_ORIGINS")
                    .unwrap_or_else(|_| DEFAULT_CORS_ORIGIN.to_string()),
            )?,
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
            .field("cors_allowed_origins", &self.cors_allowed_origins)
            .field("llm_base_url", &self.llm_base_url)
            .field("llm_api_key", &self.llm_api_key.as_ref().map(|_| "***"))
            .field("embedding_model", &self.embedding_model)
            .field("chat_model", &self.chat_model)
            .field("jwt_secret", &"***")
            .field("app_port", &self.app_port)
            .finish()
    }
}

/// `"https://app.exemple.fr, http://localhost:3000"` -> liste d'origines.
/// Une origine invalide fait échouer le démarrage plutôt que d'être ignorée.
pub fn parse_origins(raw: &str) -> anyhow::Result<Vec<HeaderValue>> {
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(|o| {
            let o = o.trim_end_matches('/');
            if !(o.starts_with("http://") || o.starts_with("https://"))
                || o.contains(char::is_whitespace)
            {
                anyhow::bail!("CORS_ALLOWED_ORIGINS : origine invalide « {o} » (attendu http(s)://hôte[:port])");
            }
            HeaderValue::from_str(o)
                .map_err(|_| anyhow::anyhow!("CORS_ALLOWED_ORIGINS : origine invalide « {o} »"))
        })
        .collect::<anyhow::Result<_>>()?;
    if origins.is_empty() {
        anyhow::bail!("CORS_ALLOWED_ORIGINS est vide");
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origines_valides() {
        let origins =
            parse_origins(" https://app.lodestone.fr/ ,http://localhost:3000").expect("valide");
        assert_eq!(
            origins,
            vec![
                HeaderValue::from_static("https://app.lodestone.fr"),
                HeaderValue::from_static("http://localhost:3000")
            ]
        );
    }

    #[test]
    fn origines_invalides() {
        for raw in ["", " , ", "*", "app.lodestone.fr", "ftp://x", "http://a b"] {
            assert!(parse_origins(raw).is_err(), "{raw:?}");
        }
    }
}
