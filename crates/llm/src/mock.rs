use std::{sync::Mutex, time::Duration};

use async_trait::async_trait;
use futures::{StreamExt, stream};

use crate::{
    client::{ChatMessage, ChatStream, LlmClient},
    error::LlmError,
};

/// Réponse générée par défaut par le mock.
pub const DEFAULT_MOCK_REPLY: &str = "Voici la réponse simulée, fondée sur les documents.";

/// Panne simulée de la génération.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatFailure {
    /// `chat_stream` échoue avant tout morceau (serveur injoignable…).
    AtStart,
    /// Le flux s'interrompt après les premiers morceaux.
    MidStream,
}

/// Client de test : pas de réseau, résultats déterministes.
///
/// Le vecteur d'un texte est la somme normalisée de vecteurs pseudo-aléatoires
/// associés à chacun de ses mots (« sac de mots »). Conséquences utiles en test :
/// - le même texte donne toujours le même vecteur (similarité cosinus = 1) ;
/// - deux textes qui partagent des mots sont proches, deux textes sans mot
///   commun sont presque orthogonaux. Une question sur les « congés » remonte
///   donc bien le document qui parle de congés.
///
/// Génération : renvoie `reply` découpée mot par mot (espaces conservés), avec
/// un délai optionnel entre deux morceaux, et garde les prompts reçus.
pub struct MockLlmClient {
    dimensions: usize,
    fail: bool,
    reply: String,
    token_delay: Duration,
    chat_failure: Option<ChatFailure>,
    chat_requests: Mutex<Vec<Vec<ChatMessage>>>,
}

impl MockLlmClient {
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            fail: false,
            reply: DEFAULT_MOCK_REPLY.to_string(),
            token_delay: Duration::ZERO,
            chat_failure: None,
            chat_requests: Mutex::default(),
        }
    }

    /// Variante dont les embeddings échouent à chaque appel (statut `failed`).
    pub fn failing(dimensions: usize) -> Self {
        Self {
            fail: true,
            ..Self::new(dimensions)
        }
    }

    pub fn with_reply(mut self, reply: impl Into<String>) -> Self {
        self.reply = reply.into();
        self
    }

    /// Délai entre deux morceaux : simule un modèle lent.
    pub fn with_token_delay(mut self, delay: Duration) -> Self {
        self.token_delay = delay;
        self
    }

    pub fn with_chat_failure(mut self, failure: ChatFailure) -> Self {
        self.chat_failure = Some(failure);
        self
    }

    /// Les prompts reçus par `chat_stream`, dans l'ordre.
    pub fn chat_requests(&self) -> Vec<Vec<ChatMessage>> {
        self.chat_requests
            .lock()
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    /// Les morceaux que `chat_stream` produit : un par mot, espace compris.
    pub fn reply_tokens(&self) -> Vec<String> {
        self.reply
            .split_inclusive(' ')
            .map(str::to_string)
            .collect()
    }

    /// Le vecteur que `embed` produit pour `text`.
    pub fn vector_for(&self, text: &str) -> Vec<f32> {
        let lower = text.to_lowercase();
        let mut words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        if words.is_empty() {
            words.push(&lower); // ponctuation seule : le texte entier compte pour un mot
        }

        let mut sum = vec![0f32; self.dimensions];
        for word in words {
            for (acc, x) in sum.iter_mut().zip(word_vector(word, self.dimensions)) {
                *acc += x;
            }
        }
        let norm = sum
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            .sqrt()
            .max(f32::EPSILON);
        sum.into_iter().map(|x| x / norm).collect()
    }
}

/// Vecteur pseudo-aléatoire dans [-1, 1]^n, fixé par le mot :
/// graine FNV-1a, puis générateur xorshift.
fn word_vector(word: &str, dimensions: usize) -> impl Iterator<Item = f32> {
    let mut state = word
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
        .max(1);
    (0..dimensions).map(move |_| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    })
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn chat_stream(&self, messages: &[ChatMessage]) -> Result<ChatStream, LlmError> {
        if let Ok(mut requests) = self.chat_requests.lock() {
            requests.push(messages.to_vec());
        }
        if self.chat_failure == Some(ChatFailure::AtStart) {
            return Err(LlmError::Transport(
                "MockLlmClient : panne au démarrage".into(),
            ));
        }

        let mut items: Vec<Result<String, LlmError>> =
            self.reply_tokens().into_iter().map(Ok).collect();
        if self.chat_failure == Some(ChatFailure::MidStream) {
            items.truncate(2);
            items.push(Err(LlmError::Transport(
                "MockLlmClient : flux interrompu".into(),
            )));
        }

        let delay = self.token_delay;
        Ok(stream::iter(items)
            .then(move |item| async move {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                item
            })
            .boxed())
    }

    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        if self.fail {
            return Err(LlmError::Transport("MockLlmClient::failing".into()));
        }
        Ok(texts.iter().map(|t| self.vector_for(t)).collect())
    }

    fn embedding_dimensions(&self) -> usize {
        self.dimensions
    }

    fn embedding_model(&self) -> &str {
        "mock-embedding"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn deterministe_et_normalise() {
        let mock = MockLlmClient::new(768);
        let a = mock.vector_for("procédure de congés");
        assert_eq!(a.len(), 768);
        assert_eq!(a, mock.vector_for("procédure de congés"));
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-4, "vecteur unitaire");
        assert_eq!(
            a,
            mock.vector_for("Procédure, de CONGÉS !"),
            "casse et ponctuation ignorées"
        );
    }

    #[test]
    fn mots_communs_rapprochent() {
        let mock = MockLlmClient::new(768);
        let question = mock.vector_for("combien de jours de congés par an");
        let conges = mock.vector_for("les salariés ont vingt-cinq jours de congés par an");
        let frais = mock.vector_for("les notes de frais sont remboursées sous trente jours");
        let sans_rapport = mock.vector_for("serveur kubernetes cluster");
        assert!(cosine(&question, &conges) > cosine(&question, &frais));
        assert!(cosine(&question, &sans_rapport).abs() < 0.2);
    }

    #[tokio::test]
    async fn generation_mot_par_mot_et_prompts_enregistres() {
        let mock = MockLlmClient::new(768).with_reply("Vingt-cinq jours par an.");
        let prompt = vec![ChatMessage::system("règles"), ChatMessage::user("congés ?")];
        let tokens: Vec<String> = mock
            .chat_stream(&prompt)
            .await
            .expect("flux")
            .map(|t| t.expect("morceau"))
            .collect()
            .await;
        assert_eq!(tokens, vec!["Vingt-cinq ", "jours ", "par ", "an."]);
        assert_eq!(mock.chat_requests(), vec![prompt]);
    }

    #[tokio::test]
    async fn pannes_de_generation() {
        let prompt = [ChatMessage::user("x")];
        let at_start = MockLlmClient::new(768).with_chat_failure(ChatFailure::AtStart);
        assert!(at_start.chat_stream(&prompt).await.is_err());

        let mid = MockLlmClient::new(768)
            .with_reply("un deux trois quatre")
            .with_chat_failure(ChatFailure::MidStream);
        let items: Vec<_> = mid
            .chat_stream(&prompt)
            .await
            .expect("flux")
            .collect()
            .await;
        assert_eq!(items.len(), 3);
        assert!(items[..2].iter().all(Result::is_ok));
        assert!(items[2].is_err());
    }

    #[test]
    fn echec_force() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let res = rt.block_on(MockLlmClient::failing(768).embed(&["x".to_string()]));
        assert!(res.is_err());
    }
}
