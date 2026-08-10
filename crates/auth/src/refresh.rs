use argon2::password_hash::rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

/// Refresh token = 32 octets aléatoires -> 64 caractères hex. PAS un JWT.
pub fn generate_refresh_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    to_hex(&bytes)
}

/// Hash SHA-256 pour le stockage. Rapide (pas argon2) car le token est déjà
/// à haute entropie : inutile de ralentir, il n'y a rien à "deviner".
pub fn hash_refresh_token(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}