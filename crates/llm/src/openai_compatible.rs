use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    client::{ChatMessage, ChatStream, LlmClient},
    error::LlmError,
};

/// Modèle de génération par défaut (cf. spec §2).
pub const DEFAULT_CHAT_MODEL: &str = "llama3.1";

/// Durée maximale d'un appel d'embedding (le premier appel à Ollama charge le
/// modèle en mémoire, d'où une marge large).
const EMBEDDING_TIMEOUT: Duration = Duration::from_secs(120);

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
    chat_model: String,
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

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
}

impl OpenAiCompatibleClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        embedding_model: impl Into<String>,
        embedding_dimensions: usize,
    ) -> Result<Self, LlmError> {
        // Pas de durée totale au niveau du client : une réponse en streaming
        // peut légitimement durer plusieurs minutes. On borne la connexion et
        // l'inactivité (aucun octet reçu), et les embeddings ont leur propre limite.
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(120))
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
            chat_model: DEFAULT_CHAT_MODEL.to_string(),
        })
    }

    /// Modèle de génération (`llama3.1`, `qwen2.5`, `gpt-4o-mini`…).
    pub fn with_chat_model(mut self, model: impl Into<String>) -> Self {
        self.chat_model = model.into();
        self
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let req = self.http.post(format!("{}{path}", self.base_url));
        match &self.api_key {
            Some(key) => req.bearer_auth(key),
            None => req,
        }
    }
}

/// Erreur HTTP -> `LlmError::Api`, avec le corps (souvent explicite :
/// « model "x" not found »).
async fn check_status(res: reqwest::Response) -> Result<reqwest::Response, LlmError> {
    let status = res.status();
    if status.is_success() {
        return Ok(res);
    }
    let body = res.text().await.unwrap_or_default();
    Err(LlmError::Api {
        status: status.as_u16(),
        body,
    })
}

#[async_trait]
impl LlmClient for OpenAiCompatibleClient {
    async fn chat_stream(&self, messages: &[ChatMessage]) -> Result<ChatStream, LlmError> {
        let res = self
            .post("/chat/completions")
            .json(&ChatRequest {
                model: &self.chat_model,
                messages,
                stream: true,
            })
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let res = check_status(res).await?;
        Ok(sse_deltas(res.bytes_stream()).boxed())
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let res = self
            .post("/embeddings")
            .timeout(EMBEDDING_TIMEOUT)
            .json(&EmbeddingRequest {
                model: &self.embedding_model,
                input: texts,
            })
            .send()
            .await
            .map_err(|e| LlmError::Transport(e.to_string()))?;
        let res = check_status(res).await?;

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

/// Ce que contient une ligne SSE (Server-Sent Events) d'OpenAI / Ollama.
#[derive(Debug)]
enum SseLine {
    /// Ligne vide, commentaire, `event:`, morceau sans texte…
    Skip,
    Delta(String),
    /// `data: [DONE]` : fin normale du flux.
    Done,
    Error(LlmError),
}

fn parse_sse_line(line: &str) -> SseLine {
    let Some(data) = line.strip_prefix("data:") else {
        return SseLine::Skip;
    };
    let data = data.trim();
    if data == "[DONE]" {
        return SseLine::Done;
    }
    let json: Value = match serde_json::from_str(data) {
        Ok(json) => json,
        Err(e) => return SseLine::Error(LlmError::InvalidResponse(format!("SSE illisible : {e}"))),
    };
    if let Some(error) = json.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .map_or_else(|| error.to_string(), str::to_string);
        return SseLine::Error(LlmError::InvalidResponse(message));
    }
    match json["choices"][0]["delta"]["content"].as_str() {
        Some(text) if !text.is_empty() => SseLine::Delta(text.to_string()),
        _ => SseLine::Skip,
    }
}

/// Transforme le flux d'octets SSE en flux de morceaux de texte. Les lignes
/// peuvent être coupées n'importe où entre deux paquets réseau : on accumule
/// jusqu'au `\n` (une ligne complète est donc toujours de l'UTF-8 entier).
fn sse_deltas<B, C, E>(bytes: B) -> impl Stream<Item = Result<String, LlmError>> + Send + 'static
where
    B: Stream<Item = Result<C, E>> + Send + 'static,
    C: AsRef<[u8]>,
    E: std::fmt::Display,
{
    struct State<B> {
        bytes: std::pin::Pin<Box<B>>,
        buffer: Vec<u8>,
        finished: bool,
    }

    futures::stream::unfold(
        State {
            bytes: Box::pin(bytes),
            buffer: Vec::new(),
            finished: false,
        },
        |mut st| async move {
            loop {
                if st.finished {
                    return None;
                }
                if let Some(pos) = st.buffer.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = st.buffer.drain(..=pos).collect();
                    match parse_sse_line(String::from_utf8_lossy(&line).trim()) {
                        SseLine::Skip => continue,
                        SseLine::Delta(text) => return Some((Ok(text), st)),
                        SseLine::Done => return None,
                        SseLine::Error(e) => {
                            st.finished = true;
                            return Some((Err(e), st));
                        }
                    }
                }
                match st.bytes.next().await {
                    Some(Ok(chunk)) => st.buffer.extend_from_slice(chunk.as_ref()),
                    Some(Err(e)) => {
                        st.finished = true;
                        return Some((Err(LlmError::Transport(e.to_string())), st));
                    }
                    // Fin du corps : on traite une éventuelle dernière ligne sans `\n`.
                    None if st.buffer.is_empty() => return None,
                    None => st.buffer.push(b'\n'),
                }
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, extract::State, http::HeaderMap, routing::post};
    use serde_json::json;

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

    // ----- Streaming (SSE) ---------------------------------------------------------

    fn sse(deltas: &[&str]) -> String {
        let mut body = String::new();
        for d in deltas {
            body.push_str(&format!(
                "data: {}\n\n",
                json!({ "choices": [ { "index": 0, "delta": { "content": d } } ] })
            ));
        }
        body.push_str("data: [DONE]\n\n");
        body
    }

    /// Décode `body` découpé en paquets de `size` octets.
    async fn decode(body: &[u8], size: usize) -> Vec<Result<String, String>> {
        let chunks: Vec<Result<Vec<u8>, String>> =
            body.chunks(size).map(|c| Ok(c.to_vec())).collect();
        sse_deltas(futures::stream::iter(chunks))
            .map(|r| r.map_err(|e| e.to_string()))
            .collect()
            .await
    }

    #[tokio::test]
    async fn sse_decoupe_n_importe_ou() {
        let body = sse(&["La procé", "dure de ", "congés…"]);
        // Paquets de 1 à 40 octets : coupe aussi au milieu des caractères UTF-8.
        for size in 1..40 {
            let got = decode(body.as_bytes(), size).await;
            assert_eq!(
                got,
                vec![
                    Ok("La procé".into()),
                    Ok("dure de ".into()),
                    Ok("congés…".into())
                ],
                "paquets de {size} octets"
            );
        }
    }

    #[tokio::test]
    async fn sse_lignes_ignorees_et_fin() {
        let body = format!(
            ": commentaire\nevent: message\n\ndata: {}\n\ndata: {}\n\n{}data: {}\n\n",
            json!({ "choices": [ { "delta": { "role": "assistant" } } ] }), // pas de texte
            json!({ "choices": [ { "delta": { "content": "" } } ] }),       // texte vide
            sse(&["ok"]),
            json!({ "choices": [ { "delta": { "content": "après DONE" } } ] }),
        );
        assert_eq!(decode(body.as_bytes(), 7).await, vec![Ok("ok".into())]);
    }

    #[tokio::test]
    async fn sse_derniere_ligne_sans_retour() {
        let body = format!(
            "data: {}",
            json!({ "choices": [ { "delta": { "content": "fin" } } ] })
        );
        assert_eq!(decode(body.as_bytes(), 5).await, vec![Ok("fin".into())]);
    }

    #[tokio::test]
    async fn sse_erreurs() {
        let error_line = format!(
            "{}data: {}\n\n{}",
            sse(&["début"]).replace("data: [DONE]\n\n", ""),
            json!({ "error": { "message": "model overloaded" } }),
            sse(&["jamais lu"])
        );
        let got = decode(error_line.as_bytes(), 9).await;
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[0], Ok("début".into()));
        assert!(
            got[1]
                .as_ref()
                .is_err_and(|e| e.contains("model overloaded"))
        );

        let got = decode(b"data: {pas du json\n\n", 4).await;
        assert!(matches!(got.as_slice(), [Err(_)]));

        let broken: Vec<Result<Vec<u8>, String>> = vec![
            Ok(sse(&["a"]).replace("data: [DONE]\n\n", "").into_bytes()),
            Err("connexion coupée".into()),
        ];
        let got: Vec<_> = sse_deltas(futures::stream::iter(broken)).collect().await;
        assert!(matches!(got.as_slice(), [Ok(a), Err(LlmError::Transport(_))] if a == "a"));
    }

    /// Faux `/v1/chat/completions` : répond `status` avec `body` en SSE.
    async fn fake_chat_server(status: u16, body: String) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let app = Router::new()
            .route(
                "/v1/chat/completions",
                post(
                    move |State(seen): State<Seen>, Json(req): Json<Value>| async move {
                        if let Ok(mut s) = seen.lock() {
                            s.push((None, req));
                        }
                        (
                            axum::http::StatusCode::from_u16(status)
                                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
                            [("content-type", "text/event-stream")],
                            body,
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

    #[tokio::test]
    async fn chat_stream_envoie_le_bon_format() {
        let (url, seen) = fake_chat_server(200, sse(&["Vingt-cinq ", "jours."])).await;
        let client = OpenAiCompatibleClient::new(url, None, "nomic-embed-text", 768)
            .expect("client")
            .with_chat_model("qwen2.5");
        let prompt = [
            ChatMessage::system("Réponds avec les documents."),
            ChatMessage::user("Combien de jours de congés ?"),
        ];
        let tokens: Vec<String> = client
            .chat_stream(&prompt)
            .await
            .expect("flux")
            .map(|t| t.expect("morceau"))
            .collect()
            .await;
        assert_eq!(tokens, vec!["Vingt-cinq ", "jours."]);

        let req = &seen.lock().expect("lock")[0].1;
        assert_eq!(
            req,
            &json!({
                "model": "qwen2.5",
                "stream": true,
                "messages": [
                    { "role": "system", "content": "Réponds avec les documents." },
                    { "role": "user", "content": "Combien de jours de congés ?" }
                ]
            })
        );
    }

    #[tokio::test]
    async fn chat_stream_modele_inconnu() {
        let (url, _) = fake_chat_server(404, r#"{"error":"model \"x\" not found"}"#.into()).await;
        let client = OpenAiCompatibleClient::new(url, None, "m", 3)
            .expect("client")
            .with_chat_model("x");
        match client.chat_stream(&[ChatMessage::user("?")]).await {
            Err(LlmError::Api { status: 404, body }) => assert!(body.contains("not found")),
            Err(e) => panic!("attendu LlmError::Api 404, obtenu {e:?}"),
            Ok(_) => panic!("attendu une erreur"),
        }
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

        // Génération : CHAT_MODEL (par défaut llama3.1) doit être téléchargé.
        let model = std::env::var("CHAT_MODEL").unwrap_or_else(|_| DEFAULT_CHAT_MODEL.to_string());
        let tokens: Vec<String> = client
            .with_chat_model(model)
            .chat_stream(&[ChatMessage::user("Réponds juste : bonjour")])
            .await
            .expect("chat_stream (modèle téléchargé ?)")
            .map(|t| t.expect("morceau"))
            .collect()
            .await;
        assert!(!tokens.concat().trim().is_empty());
    }
}
