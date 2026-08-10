
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("échec d'encodage/décodage du JWT")]
    Jwt(#[from] jsonwebtoken::errors::Error),

    #[error("échec de hachage du mot de passe")]
    Hash,

    #[error("format de hash invalide")]
    InvalidHash,
}