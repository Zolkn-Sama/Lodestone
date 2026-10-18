use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{client::LlmClient, error::LlmError};

/// Client pour toute API compatible OpenAI : Ollama (`http://localhost:11434/v1`)
/// ou OpenAI lui-même. Seules l'URL de base et la clé changent : c'est ce qui
/// rend le mode « souverain » (LLM local) trivial à activer.
pub struct OpenAiCompatibleClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    embedding_model: String,
    embedding_dimensions: usize,
    document_prefix: &'static str,
    query_prefix: &'static str,
}

/// Préfixes attendus par le modèle d'embedding : nomic-embed-text a été
/// entraîné avec `search_document:` / `search_query:` et cherche nettement
/// mieux avec. Les autres modèles (OpenAI…) n'en veulent pas.
fn prefixes_for(model: &str) -> (&'static str, &'static str) {
    if model.contains("nomic-embed") {
        ("search_document: ", "search_query: ")
    } else {
        ("", "")
    }
}

#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
}

#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Deserialize)]
struct EmbeddingData {
    index: usize,
    embedding: Vec<f32>,
}

impl OpenAiCompatibleClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        embedding_model: impl Into<String>,
        embedding_dimensions: usize,
    ) -> Result<Self, LlmError> {
        let http = reqwest::Client::builder()
            // Large : un premier appel à Ollama charge le modèle en mémoire.
            .timeout(Duration::from_secs(120))
            .build()
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let embedding_model = embedding_model.into();
        let (document_prefix, query_prefix) = prefixes_for(&embedding_model);
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            embedding_model,
            embedding_dimensions,
            document_prefix,
            query_prefix,
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatibleClient {
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut req = self
            .http
            .post(format!("{}/embeddings", self.base_url))
            .json(&EmbeddingRequest {
                model: &self.embedding_model,
                input: texts,
            });
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key);
        }

        let res = req
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let status = res.status();
        if !status.is_success() {
            let body = res.text().await.unwrap_or_default();
            return Err(LlmError::Api {
                status: status.as_u16(),
                body,
            });
        }

        let mut body: EmbeddingResponse = res
            .json()
            .await
            .map_err(|e| LlmError::InvalidResponse(e.to_string()))?;

        // L'API ne garantit pas l'ordre : on se fie au champ `index`.
        body.data.sort_by_key(|d| d.index);
        let vectors: Vec<Vec<f32>> = body.data.into_iter().map(|d| d.embedding).collect();

        if vectors.len() != texts.len() {
            return Err(LlmError::InvalidResponse(format!(
                "{} vecteurs reçus pour {} textes",
                vectors.len(),
                texts.len()
            )));
        }
        if let Some(v) = vectors
            .iter()
            .find(|v| v.len() != self.embedding_dimensions)
        {
            return Err(LlmError::InvalidResponse(format!(
                "vecteur de taille {} au lieu de {} (modèle {})",
                v.len(),
                self.embedding_dimensions,
                self.embedding_model
            )));
        }
        Ok(vectors)
    }

    fn embedding_dimensions(&self) -> usize {
        self.embedding_dimensions
    }

    fn document_prefix(&self) -> &str {
        self.document_prefix
    }

    fn query_prefix(&self) -> &str {
        self.query_prefix
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
    use serde_json::{Value, json};

    use super::*;

    /// Requêtes reçues par le faux serveur : (en-tête Authorization, corps).
    type Seen = Arc<Mutex<Vec<(Option<String>, Value)>>>;

    /// Démarre un faux serveur `/v1/embeddings` qui répond avec `reply(corps)`.
    async fn fake_server(reply: fn(&Value) -> (u16, Value)) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let app =
            Router::new()
                .route(
                    "/v1/embeddings",
                    post(
                        move |State(seen): State<Seen>,
                              headers: HeaderMap,
                              Json(body): Json<Value>| async move {
                            let auth = headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(String::from);
                            let (status, json) = reply(&body);
                            if let Ok(mut s) = seen.lock() {
                                s.push((auth, body));
                            }
                            (
                                axum::http::StatusCode::from_u16(status)
                                    .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
                                Json(json),
                            )
                        },
                    ),
                )
                .with_state(seen.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await });
        (format!("http://{addr}/v1"), seen)
    }

    /// Répond un vecteur [i, i, i] par texte, dans le désordre.
    fn reply_ok(body: &Value) -> (u16, Value) {
        let n = body["input"].as_array().map_or(0, Vec::len);
        let data: Vec<Value> = (0..n)
            .rev()
            .map(|i| json!({ "index": i, "embedding": vec![i as f32; 3] }))
            .collect();
        (200, json!({ "object": "list", "data": data }))
    }

    fn texts(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("texte {i}")).collect()
    }

    #[tokio::test]
    async fn envoie_le_bon_format_et_remet_dans_l_ordre() {
        let (url, seen) = fake_server(reply_ok).await;
        let client = OpenAiCompatibleClient::new(url, None, "nomic-embed-text", 3).expect("client");

        let vectors = client.embed(&texts(3)).await.expect("embed");
        assert_eq!(vectors, vec![vec![0.0; 3], vec![1.0; 3], vec![2.0; 3]]);

        let seen = seen.lock().expect("lock");
        let (auth, body) = &seen[0];
        assert_eq!(auth, &None, "pas de clé -> pas d'en-tête Authorization");
        assert_eq!(body["model"], "nomic-embed-text");
        assert_eq!(body["input"], json!(["texte 0", "texte 1", "texte 2"]));
    }

    #[tokio::test]
    async fn envoie_la_cle_api_si_fournie() {
        let (url, seen) = fake_server(reply_ok).await;
        let client =
            OpenAiCompatibleClient::new(url, Some("sk-test".into()), "m", 3).expect("client");
        client.embed(&texts(1)).await.expect("embed");
        assert_eq!(
            seen.lock().expect("lock")[0].0.as_deref(),
            Some("Bearer sk-test")
        );
    }

    #[tokio::test]
    async fn liste_vide_sans_appel_reseau() {
        let (url, seen) = fake_server(reply_ok).await;
        let client = OpenAiCompatibleClient::new(url, None, "m", 3).expect("client");
        assert!(client.embed(&[]).await.expect("embed").is_empty());
        assert!(seen.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn statut_d_erreur_remonte() {
        let (url, _) = fake_server(|_| (404, json!({ "error": "model \"x\" not found" }))).await;
        let client = OpenAiCompatibleClient::new(url, None, "x", 3).expect("client");
        match client.embed(&texts(1)).await {
            Err(LlmError::Api { status, body }) => {
                assert_eq!(status, 404);
                assert!(body.contains("not found"));
            }
            other => panic!("attendu LlmError::Api, obtenu {other:?}"),
        }
    }

    #[tokio::test]
    async fn mauvaise_dimension_rejetee() {
        let (url, _) = fake_server(reply_ok).await;
        // Le serveur renvoie des vecteurs de 3, on en attend 768.
        let client = OpenAiCompatibleClient::new(url, None, "m", 768).expect("client");
        assert!(matches!(
            client.embed(&texts(2)).await,
            Err(LlmError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn nombre_de_vecteurs_incoherent_rejete() {
        let (url, _) = fake_server(|_| {
            (
                200,
                json!({ "data": [ { "index": 0, "embedding": [0.0, 0.0, 0.0] } ] }),
            )
        })
        .await;
        let client = OpenAiCompatibleClient::new(url, None, "m", 3).expect("client");
        assert!(matches!(
            client.embed(&texts(2)).await,
            Err(LlmError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn serveur_injoignable() {
        // Port 9 (discard) : rien n'écoute.
        let client =
            OpenAiCompatibleClient::new("http://127.0.0.1:9/v1", None, "m", 3).expect("client");
        assert!(matches!(
            client.embed(&texts(1)).await,
            Err(LlmError::Transport(_))
        ));
    }

    #[tokio::test]
    async fn prefixes_nomic_pour_documents_et_questions() {
        let (url, seen) = fake_server(reply_ok).await;
        let nomic = OpenAiCompatibleClient::new(url.clone(), None, "nomic-embed-text:latest", 3)
            .expect("client");
        nomic.embed_documents(&texts(1)).await.expect("documents");
        nomic.embed_query("congés ?").await.expect("question");

        let other =
            OpenAiCompatibleClient::new(url, None, "text-embedding-3-small", 3).expect("client");
        other.embed_documents(&texts(1)).await.expect("documents");
        other.embed_query("congés ?").await.expect("question");

        let inputs: Vec<Value> = seen
            .lock()
            .expect("lock")
            .iter()
            .map(|(_, body)| body["input"].clone())
            .collect();
        assert_eq!(
            inputs,
            vec![
                json!(["search_document: texte 0"]),
                json!(["search_query: congés ?"]),
                json!(["texte 0"]),
                json!(["congés ?"]),
            ]
        );
    }

    /// Contre un vrai Ollama (`ollama serve` + `ollama pull nomic-embed-text`) :
    /// `cargo test -p lodestone-llm -- --ignored`
    #[tokio::test]
    #[ignore = "nécessite un Ollama local avec nomic-embed-text"]
    async fn ollama_reel() {
        let base = std::env::var("OLLAMA_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:11434/v1".to_string());
        let client =
            OpenAiCompatibleClient::new(base, None, "nomic-embed-text", 768).expect("client");
        let vectors = client
            .embed(&[
                "procédure de congés".to_string(),
                "note de frais".to_string(),
            ])
            .await
            .expect("embed (Ollama lancé ? modèle téléchargé ?)");
        assert_eq!(vectors.len(), 2);
        assert!(vectors.iter().all(|v| v.len() == 768));
        assert_ne!(vectors[0], vectors[1]);
    }
}
