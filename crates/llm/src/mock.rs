use async_trait::async_trait;

use crate::{client::LlmClient, error::LlmError};

/// Client de test : pas de réseau, et le même texte donne toujours le même
/// vecteur (normalisé). Deux textes identiques ont donc une similarité
/// cosinus de 1, ce qui permet de tester la recherche de bout en bout.
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
        // Graine FNV-1a du texte, puis générateur pseudo-aléatoire xorshift.
        let mut state = text
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
            })
            .max(1);
        let raw: Vec<f32> = (0..self.dimensions)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                // Valeur dans [-1, 1].
                (state >> 40) as f32 / (1u64 << 23) as f32 - 1.0
            })
            .collect();
        let norm = raw
            .iter()
            .map(|x| x * x)
            .sum::<f32>()
            .sqrt()
            .max(f32::EPSILON);
        raw.into_iter().map(|x| x / norm).collect()
    }
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
    fn deterministe_normalise_et_discriminant() {
        let mock = MockLlmClient::new(768);
        let a = mock.vector_for("procédure de congés");
        assert_eq!(a.len(), 768);
        assert_eq!(a, mock.vector_for("procédure de congés"));
        assert!((cosine(&a, &a) - 1.0).abs() < 1e-4, "vecteur unitaire");
        let b = mock.vector_for("note de frais");
        assert!(
            cosine(&a, &b) < 0.5,
            "textes différents -> vecteurs éloignés"
        );
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
