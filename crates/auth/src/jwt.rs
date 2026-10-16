use crate::error::AuthError;
use chrono::{Duration, Utc};
use jsonwebtoken::{DecodingKey, EncodingKey, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Le contenu (payload) du JWT. Rappel : c'est SIGNÉ, pas chiffré —
/// donc lisible par tous. On n'y met JAMAIS de secret (pas de password_hash).
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// "subject" : à qui appartient le token. Ici l'id de l'utilisateur.
    pub sub: Uuid,
    /// "issued at" : quand il a été émis (timestamp Unix).
    pub iat: i64,
    /// "expiration" : après ce timestamp, le token est invalide.
    pub exp: i64,
}

/// Crée un access token signé pour un utilisateur donné.
/// Le `secret` vient de la config (state.config.jwt_secret), pas du code.
pub fn create_access_token(user_id: Uuid, secret: &str) -> Result<String, AuthError> {
    let now = Utc::now();
    let claims = Claims {
        sub: user_id,
        iat: now.timestamp(),
        exp: (now + Duration::minutes(15)).timestamp(), // access court : 15 min
    };

    let token = encode(
        &Header::default(), // HS256 par défaut = ton design
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;
    Ok(token)
}

/// Décode et VALIDE un token (signature + expiration). Renvoie les claims.
pub fn decode_access_token(token: &str, secret: &str) -> Result<Claims, AuthError> {
    let data = decode::<Claims>(
        // <-- Claims, pas User
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(), // vérifie la signature ET l'expiration
    )?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aller_retour() {
        let id = Uuid::new_v4();
        let token = create_access_token(id, "secret").expect("encode");
        let claims = decode_access_token(&token, "secret").expect("decode");
        assert_eq!(claims.sub, id);
    }

    #[test]
    fn mauvais_secret_rejete() {
        let token = create_access_token(Uuid::new_v4(), "secret").expect("encode");
        assert!(decode_access_token(&token, "autre").is_err());
    }
}
