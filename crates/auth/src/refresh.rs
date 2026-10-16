use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

/// Jeton opaque = 32 octets aléatoires -> 64 caractères hex. PAS un JWT.
/// Sert aux refresh tokens comme aux liens d'invitation.
pub fn generate_opaque_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    to_hex(&bytes)
}

/// Hash SHA-256 pour le stockage. Rapide (pas argon2) car le token est déjà
/// à haute entropie : inutile de ralentir, il n'y a rien à "deviner".
pub fn hash_opaque_token(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

pub fn generate_refresh_token() -> String {
    generate_opaque_token()
}

pub fn hash_refresh_token(token: &str) -> String {
    hash_opaque_token(token)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jetons_uniques_et_hash_stable() {
        let a = generate_opaque_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, generate_opaque_token());
        assert_eq!(hash_opaque_token(&a), hash_opaque_token(&a));
        assert_ne!(hash_opaque_token(&a), a);
    }
}
