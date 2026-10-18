use async_trait::async_trait;

use crate::{client::LlmClient, error::LlmError};

/// Client de test : pas de réseau, résultats déterministes.
///
/// Le vecteur d'un texte est la somme normalisée de vecteurs pseudo-aléatoires
/// associés à chacun de ses mots (« sac de mots »). Conséquences utiles en test :
/// - le même texte donne toujours le même vecteur (similarité cosinus = 1) ;
/// - deux textes qui partagent des mots sont proches, deux textes sans mot
///   commun sont presque orthogonaux. Une question sur les « congés » remonte
///   donc bien le document qui parle de congés.
pub struct MockLlmClient {
    dimensions: usize,
    fail: bool,
}

impl MockLlmClient {
    pub fn new(dimensions: usize) -> Self {
        Self {
            dimensions,
            fail: false,
        }
    }

    /// Variante qui échoue à chaque appel (pour tester le statut `failed`).
    pub fn failing(dimensions: usize) -> Self {
        Self {
            dimensions,
            fail: true,
        }
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
    async fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, LlmError> {
        if self.fail {
            return Err(LlmError::Transport("MockLlmClient::failing".into()));
        }
        Ok(texts.iter().map(|t| self.vector_for(t)).collect())
    }

    fn embedding_dimensions(&self) -> usize {
        self.dimensions
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

    #[test]
    fn echec_force() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime");
        let res = rt.block_on(MockLlmClient::failing(768).embed(&["x".to_string()]));
        assert!(res.is_err());
    }
}
