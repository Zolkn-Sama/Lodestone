use thiserror::Error;

#[derive(Debug, Error)]
pub enum RagError {
    #[error("format non supporté : {0} (attendu : pdf, txt ou md)")]
    UnsupportedFormat(String),

    #[error("le fichier n'est pas un texte UTF-8 valide")]
    InvalidUtf8,

    #[error("PDF illisible : {0}")]
    Pdf(String),

    /// Aucun texte exploitable (ex. PDF scanné, sans couche texte).
    #[error("aucun texte exploitable dans le document")]
    Empty,
}
