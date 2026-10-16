use lodestone_entity::users;

use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait,
    QueryFilter, Set,
};

use sea_orm::sea_query::{Expr, Func};
use uuid::Uuid;

/// Crée un utilisateur en base et renvoie la ligne réellement insérée.
///
/// Note : ce repositories ne HACHE PAS le mot de passe. Il reçoit un
/// `password_hash` déjà haché (ce sera le boulot du service, semaine 3).
/// Le repositories ne fait que stocker.
pub async fn create(
    db: &DatabaseConnection,
    email: String,
    password_hash: String,
) -> Result<users::Model, DbErr> {
    // Un ActiveModel décrit les colonnes qu'on veut écrire.
    // `Set(x)` = « écris cette valeur » ; les champs non listés restent NotSet.
    let new_user = users::ActiveModel {
        id: Set(Uuid::new_v4()), // on génère l'UUID côté Rust (pas de défaut SQL)
        email: Set(email),
        password_hash: Set(password_hash),
        ..Default::default() // created_at : NotSet -> le défaut SQL now() s'applique
    };

    // `.insert()` exécute le INSERT et renvoie le Model complet
    // (avec le created_at rempli par Postgres).
    new_user.insert(db).await
}

/// Cherche un utilisateur par email. `Ok(None)` s'il n'existe pas.
pub async fn find_by_email(
    db: &DatabaseConnection,
    email: &str,
) -> Result<Option<users::Model>, DbErr> {
    users::Entity::find()
        .filter(users::Column::Email.eq(email)) // WHERE email = ...
        .one(db) // au plus une ligne
        .await
}

/// Comme `find_by_email`, mais sans tenir compte de la casse.
/// `email` doit déjà être en minuscules.
pub async fn find_by_email_ci<C: ConnectionTrait>(
    db: &C,
    email: &str,
) -> Result<Option<users::Model>, DbErr> {
    users::Entity::find()
        .filter(Expr::expr(Func::lower(Expr::col(users::Column::Email))).eq(email))
        .one(db)
        .await
}

/// Cherche un utilisateur par id. `Ok(None)` s'il n'existe pas.
pub async fn find_by_id<C: ConnectionTrait>(
    db: &C,
    id: Uuid,
) -> Result<Option<users::Model>, DbErr> {
    users::Entity::find_by_id(id).one(db).await
}
