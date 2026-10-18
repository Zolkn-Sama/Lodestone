//! Mot de passe oublié : lien de réinitialisation par e-mail (usage unique,
//! 1 h), puis nouveau mot de passe et déconnexion de toutes les sessions.

use std::sync::Arc;

use chrono::{Duration, Utc};
use sea_orm::{DatabaseConnection, TransactionTrait};

use crate::{
    error::AppError,
    mail::{self, Email, Mailer},
    repositories,
};
use lodestone_auth::{password, refresh};

/// Durée de validité d'un lien de réinitialisation.
const RESET_TTL_MINUTES: i64 = 60;

pub const INVALID_LINK: &str = "lien de réinitialisation invalide ou expiré";

/// Envoie un lien si un compte correspond à `email`. Ne dit JAMAIS si le
/// compte existe : l'appelant répond la même chose dans tous les cas.
pub async fn request_reset(
    db: &DatabaseConnection,
    mailer: Arc<dyn Mailer>,
    app_base_url: &str,
    email: &str,
) -> Result<(), AppError> {
    let email = email.trim().to_lowercase();
    let Some(user) = repositories::users::find_by_email_ci(db, &email).await? else {
        tracing::info!("mot de passe oublié : aucun compte pour cette adresse");
        return Ok(());
    };

    let token = refresh::generate_opaque_token();
    let txn = db.begin().await?;
    // Seul le dernier lien envoyé fonctionne.
    repositories::password_resets::invalidate_for_user(&txn, user.id).await?;
    repositories::password_resets::create(
        &txn,
        user.id,
        &refresh::hash_opaque_token(&token),
        Utc::now() + Duration::minutes(RESET_TTL_MINUTES),
    )
    .await?;
    txn.commit().await?;

    mail::send_in_background(
        mailer,
        Email {
            to: user.email,
            subject: "Réinitialisation de votre mot de passe Lodestone".into(),
            body: format!(
                "Bonjour,\n\n\
                 Une réinitialisation du mot de passe de votre compte Lodestone a été demandée.\n\
                 Pour choisir un nouveau mot de passe, ouvrez ce lien (valable {RESET_TTL_MINUTES} minutes, \
                 utilisable une seule fois) :\n\n\
                 {app_base_url}/reset-password?token={token}\n\n\
                 Si vous n'êtes pas à l'origine de cette demande, ignorez ce message : \
                 votre mot de passe reste inchangé.\n"
            ),
        },
    );
    Ok(())
}

/// Applique le nouveau mot de passe si le jeton est valable, puis révoque
/// toutes les sessions de l'utilisateur.
pub async fn reset(
    db: &DatabaseConnection,
    token: &str,
    new_password: &str,
) -> Result<(), AppError> {
    let hash = refresh::hash_opaque_token(token);
    let password_hash = password::hash_password(new_password).map_err(|_| AppError::Internal)?;

    let txn = db.begin().await?;
    let user_id = repositories::password_resets::find_valid_for_update(&txn, &hash)
        .await?
        .ok_or_else(|| AppError::InvalidInput(INVALID_LINK.into()))?;

    repositories::users::set_password_hash(&txn, user_id, &password_hash).await?;
    // Ce jeton et les éventuels autres deviennent inutilisables.
    repositories::password_resets::invalidate_for_user(&txn, user_id).await?;
    // Qui avait volé une session est déconnecté.
    repositories::refresh_tokens::revoke_all_for_user(&txn, user_id).await?;
    txn.commit().await?;

    tracing::info!(%user_id, "mot de passe réinitialisé, sessions révoquées");
    Ok(())
}
