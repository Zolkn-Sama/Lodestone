use serde::{Deserialize, Serialize};

/// Cycle de vie d'un document : `pending` -> `processing` -> `ready` | `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentStatus {
    Pending,
    Processing,
    Ready,
    Failed,
}

impl DocumentStatus {
    /// Représentation stockée en base (colonne TEXT).
    pub fn as_str(&self) -> &'static str {
        match self {
            DocumentStatus::Pending => "pending",
            DocumentStatus::Processing => "processing",
            DocumentStatus::Ready => "ready",
            DocumentStatus::Failed => "failed",
        }
    }
}

impl std::str::FromStr for DocumentStatus {
    type Err = UnknownDocumentStatus;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pending" => Ok(DocumentStatus::Pending),
            "processing" => Ok(DocumentStatus::Processing),
            "ready" => Ok(DocumentStatus::Ready),
            "failed" => Ok(DocumentStatus::Failed),
            other => Err(UnknownDocumentStatus(other.to_string())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("statut de document inconnu : {0}")]
pub struct UnknownDocumentStatus(pub String);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour_texte() {
        use DocumentStatus::*;
        for s in [Pending, Processing, Ready, Failed] {
            assert_eq!(s.as_str().parse::<DocumentStatus>().ok(), Some(s));
        }
        assert!("done".parse::<DocumentStatus>().is_err());
    }
}
