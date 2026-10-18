use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DbErr, Statement};
use uuid::Uuid;

/// Enregistre un jeton (son hash seulement), valable jusqu'à `expires_at`.
pub async fn create<C: ConnectionTrait>(
    conn: &C,
    user_id: Uuid,
    token_hash: &str,
    expires_at: DateTime<Utc>,
) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO password_resets (id, user_id, token_hash, expires_at) VALUES ($1, $2, $3, $4)",
        [
            Uuid::new_v4().into(),
            user_id.into(),
            token_hash.into(),
            expires_at.into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Invalide les jetons encore utilisables de l'utilisateur : seul le dernier
/// lien envoyé fonctionne, et un mot de passe changé rend les autres caducs.
pub async fn invalidate_for_user<C: ConnectionTrait>(conn: &C, user_id: Uuid) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE password_resets SET used_at = now() WHERE user_id = $1 AND used_at IS NULL",
        [user_id.into()],
    ))
    .await?;
    Ok(())
}

/// Le propriétaire d'un jeton encore valable (non utilisé, non expiré),
/// verrouillé jusqu'à la fin de la transaction : un jeton ne sert qu'une fois,
/// même avec deux requêtes simultanées.
pub async fn find_valid_for_update<C: ConnectionTrait>(
    conn: &C,
    token_hash: &str,
) -> Result<Option<Uuid>, DbErr> {
    match conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT user_id FROM password_resets
             WHERE token_hash = $1 AND used_at IS NULL AND expires_at > now()
             FOR UPDATE",
            [token_hash.into()],
        ))
        .await?
    {
        Some(row) => Ok(Some(row.try_get("", "user_id")?)),
        None => Ok(None),
    }
}
