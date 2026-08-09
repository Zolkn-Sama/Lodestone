use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core::OsRng},
    Argon2,
};
use crate::error::AuthError;

/// Hache un mot de passe en clair avec argon2id.
/// Le résultat est une "PHC string" qui contient DÉJÀ le sel et les paramètres.
pub fn hash_password(plain: &str) -> Result<String, AuthError> {
    // Un sel aléatoire, différent à CHAQUE appel (même mot de passe -> hash différent).
    let salt = SaltString::generate(&mut OsRng);

    // Argon2::default() = argon2id avec les paramètres recommandés par l'OWASP.
    Argon2::default()
        .hash_password(plain.as_bytes(), &salt)
        .map(|hash| hash.to_string()) // sérialise en PHC string, stockable telle quelle
        .map_err(|_| AuthError::Hash)
}

/// Vérifie un mot de passe en clair contre un hash stocké.
/// Renvoie Ok(true) si ça correspond, Ok(false) sinon.
pub fn verify_password(plain: &str, stored_hash: &str) -> Result<bool, AuthError> {
    // On reparse la PHC string : elle porte son propre sel et ses paramètres.
    let parsed = PasswordHash::new(stored_hash).map_err(|_| AuthError::InvalidHash)?;

    // Ok(true) si match, Ok(false) si mauvais mot de passe.
    // Toute AUTRE erreur (hash corrompu…) remonte en Err.
    match Argon2::default().verify_password(plain.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(_) => Err(AuthError::InvalidHash),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_puis_verifie() {
        let hash = hash_password("motdepasse123").unwrap();
        assert!(verify_password("motdepasse123", &hash).unwrap());       // bon mdp -> true
        assert!(!verify_password("mauvais", &hash).unwrap());            // mauvais mdp -> false
    }

    #[test]
    fn deux_hachages_different() {
        // Grâce au sel aléatoire, le même mot de passe donne deux hash différents...
        let h1 = hash_password("meme").unwrap();
        let h2 = hash_password("meme").unwrap();
        assert_ne!(h1, h2);
        // ...mais les deux se vérifient correctement.
        assert!(verify_password("meme", &h1).unwrap());
        assert!(verify_password("meme", &h2).unwrap());
    }
}