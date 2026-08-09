use sea_orm::DatabaseConnection;

use chrono::{Duration, Utc};

const REFRESH_TTL_DAYS: i64 = 7;

use lodestone_auth::{jwt, password, refresh};
use crate::{dto::auth::TokenResponse, error::AppError, repositories::users, repositories::refresh_tokens};

/// Inscription : hache le mot de passe, refuse un email déjà pris, crée le user.
pub async fn register(
    db: &DatabaseConnection,
    secret: &str,
    email: String,
    plain_password: String,
) -> Result<TokenResponse, AppError> {
    // 1. Email déjà utilisé ? (une requête, pas une contrainte qu'on laisse exploser)
    if users::find_by_email(db, &email).await?.is_some() {
        return Err(AppError::InvalidInput("cet email est déjà utilisé".into()));
    }

    // 2. On hache AVANT de stocker. Le clair ne touche jamais la base.
    let password_hash = password::hash_password(&plain_password)
        .map_err(|_| AppError::Internal)?;

    // 3. Création.
    let user = users::create(db, email, password_hash).await?;

    // 4. On connecte directement le nouvel utilisateur (on lui rend un token).
    issue_tokens(db, user.id, secret).await
}

/// Connexion : trouve l'user, vérifie le mot de passe, émet un token.
pub async fn login(
    db: &DatabaseConnection,
    secret: &str,
    email: String,
    plain_password: String,
) -> Result<TokenResponse, AppError> {
    // 1. On cherche l'utilisateur.
    let user = users::find_by_email(db, &email)
        .await?
        .ok_or(AppError::Unauthorized)?; // introuvable -> 401 (message générique)

    // 2. On vérifie le mot de passe contre le hash stocké.
    let ok = password::verify_password(&plain_password, &user.password_hash)
        .map_err(|_| AppError::Internal)?;
    if !ok {
        return Err(AppError::Unauthorized); // mauvais mdp -> 401, même message
    }

    // 3. Tout est bon : on émet un token.
    issue_tokens(db, user.id, secret).await
}

/// Émet une paire access + refresh. Le refresh en CLAIR part au client ;
/// seul son hash est stocké.
async fn issue_tokens(
    db: &DatabaseConnection,
    user_id: uuid::Uuid,
    secret: &str,
) -> Result<TokenResponse, AppError> {
    let access_token = jwt::create_access_token(user_id, secret)?;

    let refresh_plain = refresh::generate_refresh_token();
    let refresh_hash = refresh::hash_refresh_token(&refresh_plain);
    let expires_at = Utc::now() + Duration::days(REFRESH_TTL_DAYS);
    refresh_tokens::create(db, user_id, refresh_hash, expires_at).await?;

    Ok(TokenResponse {
        access_token,
        refresh_token: refresh_plain,
        token_type: "Bearer".into(),
    })
}

/// Renouvelle la paire à partir d'un refresh valide, en le faisant TOURNER.
pub async fn refresh(
    db: &DatabaseConnection,
    secret: &str,
    refresh_plain: String,
) -> Result<TokenResponse, AppError> {
    let hash = refresh::hash_refresh_token(&refresh_plain);

    let row = refresh_tokens::find_by_hash(db, &hash)
        .await?
        .ok_or(AppError::Unauthorized)?;

    // Rejeté si révoqué ou expiré.
    if row.revoked_at.is_some() || row.expires_at.with_timezone(&Utc) < Utc::now() {
        return Err(AppError::Unauthorized);
    }

    // ROTATION : on révoque l'ancien AVANT d'émettre le nouveau.
    refresh_tokens::revoke(db, row.id).await?;
    issue_tokens(db, row.user_id, secret).await
}

/// Déconnexion : révoque le refresh fourni (silencieux s'il n'existe pas).
pub async fn logout(db: &DatabaseConnection, refresh_plain: String) -> Result<(), AppError> {
    let hash = refresh::hash_refresh_token(&refresh_plain);
    if let Some(row) = refresh_tokens::find_by_hash(db, &hash).await? {
        refresh_tokens::revoke(db, row.id).await?;
    }
    Ok(())
}