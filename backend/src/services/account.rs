//! Droits RGPD sur son compte : effacement (art. 17) et portabilité (art. 20).

use chrono::Utc;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder,
    Statement, TransactionTrait, prelude::DateTimeWithTimeZone,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    dto::{auth::UserResponse, conversations::ConversationDetailResponse},
    error::AppError,
    repositories, services,
};
use lodestone_auth::password;
use lodestone_entity::{conversations, documents, invitations};

/// Ce que la suppression a emporté (journalisé, utile en test).
#[derive(Debug, PartialEq, Eq)]
pub struct Deletion {
    /// Orgs dont l'utilisateur était le seul membre, supprimées avec lui.
    pub deleted_orgs: Vec<Uuid>,
}

/// Une org de l'utilisateur, vue au moment de la suppression.
struct OrgSituation {
    id: Uuid,
    name: String,
    is_owner: bool,
    members: i64,
    owners: i64,
    /// Abonnement Stripe encore actif (il continuerait d'être facturé).
    paying: bool,
}

/// Supprime le compte après vérification du mot de passe. Refusé (409, sans
/// rien supprimer) si l'utilisateur est le seul owner d'une org qui a d'autres
/// membres, ou si une org dont il est le seul membre a un abonnement actif.
/// Les orgs dont il est le seul membre sont supprimées avec lui ; dans les
/// autres, ses documents et invitations restent (auteur effacé).
pub async fn delete_account(
    db: &DatabaseConnection,
    user_id: Uuid,
    plain_password: &str,
) -> Result<Deletion, AppError> {
    let user = repositories::users::find_by_id(db, user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let ok = password::verify_password(plain_password, &user.password_hash)
        .map_err(|_| AppError::Internal)?;
    if !ok {
        return Err(AppError::Unauthorized);
    }

    let txn = db.begin().await?;
    // Verrou des orgs : personne ne rejoint une org qu'on s'apprête à
    // supprimer parce qu'elle n'avait qu'un membre (l'acceptation d'une
    // invitation verrouille la même ligne).
    let rows = txn
        .query_all(Statement::from_sql_and_values(
            txn.get_database_backend(),
            "SELECT o.id, o.name, m.role = 'owner' AS is_owner,
                    (SELECT count(*) FROM memberships WHERE org_id = o.id) AS members,
                    (SELECT count(*) FROM memberships WHERE org_id = o.id AND role = 'owner') AS owners,
                    EXISTS (SELECT 1 FROM subscriptions s WHERE s.org_id = o.id
                            AND s.status IN ('active', 'trialing', 'past_due')) AS paying
             FROM memberships m
             JOIN organizations o ON o.id = m.org_id
             WHERE m.user_id = $1
             ORDER BY o.name
             FOR UPDATE OF o",
            [user_id.into()],
        ))
        .await?;
    let orgs = rows
        .into_iter()
        .map(|r| {
            Ok(OrgSituation {
                id: r.try_get("", "id")?,
                name: r.try_get("", "name")?,
                is_owner: r.try_get("", "is_owner")?,
                members: r.try_get("", "members")?,
                owners: r.try_get("", "owners")?,
                paying: r.try_get("", "paying")?,
            })
        })
        .collect::<Result<Vec<_>, sea_orm::DbErr>>()?;

    let sole_owner: Vec<&str> = orgs
        .iter()
        .filter(|o| o.is_owner && o.owners == 1 && o.members > 1)
        .map(|o| o.name.as_str())
        .collect();
    let paying_alone: Vec<&str> = orgs
        .iter()
        .filter(|o| o.members == 1 && o.paying)
        .map(|o| o.name.as_str())
        .collect();
    if !sole_owner.is_empty() || !paying_alone.is_empty() {
        let mut reasons = Vec::new();
        if !sole_owner.is_empty() {
            reasons.push(format!(
                "vous êtes le seul owner de {} : nommez un autre owner avant de partir",
                sole_owner.join(", ")
            ));
        }
        if !paying_alone.is_empty() {
            reasons.push(format!(
                "l'abonnement de {} est actif : résiliez-le d'abord",
                paying_alone.join(", ")
            ));
        }
        return Err(AppError::Conflict(format!(
            "suppression du compte impossible : {}",
            reasons.join(" ; ")
        )));
    }

    // Orgs dont il est le seul membre : leurs données n'appartiennent qu'à lui.
    let deleted_orgs: Vec<Uuid> = orgs
        .iter()
        .filter(|o| o.members == 1)
        .map(|o| o.id)
        .collect();
    for org_id in &deleted_orgs {
        repositories::organizations::delete(&txn, *org_id).await?;
    }
    // Le reste suit en cascade (sessions, appartenances, conversations,
    // jetons) ou est détaché (documents et invitations : auteur -> NULL).
    txn.execute(Statement::from_sql_and_values(
        txn.get_database_backend(),
        "DELETE FROM users WHERE id = $1",
        [user_id.into()],
    ))
    .await?;
    txn.commit().await?;

    tracing::info!(%user_id, deleted_orgs = deleted_orgs.len(), "compte supprimé (RGPD)");
    Ok(Deletion { deleted_orgs })
}

// ----- Export (portabilité) ----------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct AccountExport {
    pub exported_at: DateTimeWithTimeZone,
    pub profile: UserResponse,
    pub organizations: Vec<crate::dto::organizations::MyOrgResponse>,
    /// Ses conversations, avec messages et sources citées.
    pub conversations: Vec<ExportedConversation>,
    /// Documents qu'il a apportés (métadonnées : le contenu appartient à l'org).
    pub documents_uploaded: Vec<ExportedDocument>,
    pub invitations_sent: Vec<ExportedInvitation>,
}

#[derive(Debug, Serialize)]
pub struct ExportedConversation {
    pub org_id: Uuid,
    #[serde(flatten)]
    pub detail: ConversationDetailResponse,
}

#[derive(Debug, Serialize)]
pub struct ExportedDocument {
    pub id: Uuid,
    pub org_id: Uuid,
    pub title: String,
    pub source_type: String,
    pub status: String,
    pub created_at: DateTimeWithTimeZone,
}

#[derive(Debug, Serialize)]
pub struct ExportedInvitation {
    pub org_id: Uuid,
    pub email: String,
    pub role: String,
    pub created_at: DateTimeWithTimeZone,
    pub accepted: bool,
}

/// Toutes les données personnelles de l'utilisateur, dans un format lisible
/// et réutilisable (JSON). Jamais de secret (mot de passe, jetons).
pub async fn export(db: &DatabaseConnection, user_id: Uuid) -> Result<AccountExport, AppError> {
    let user = repositories::users::find_by_id(db, user_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let organizations = services::organizations::list_for_user(db, user_id).await?;

    let mut exported = Vec::new();
    for conversation in conversations::Entity::find()
        .filter(conversations::Column::UserId.eq(user_id))
        .order_by_asc(conversations::Column::CreatedAt)
        .all(db)
        .await?
    {
        let org_id = conversation.org_id;
        let detail = services::conversations::detail(db, conversation).await?;
        exported.push(ExportedConversation { org_id, detail });
    }

    let documents_uploaded = documents::Entity::find()
        .filter(documents::Column::UploadedBy.eq(user_id))
        .order_by_asc(documents::Column::CreatedAt)
        .all(db)
        .await?
        .into_iter()
        .map(|d| ExportedDocument {
            id: d.id,
            org_id: d.org_id,
            title: d.title,
            source_type: d.source_type,
            status: d.status,
            created_at: d.created_at,
        })
        .collect();

    let invitations_sent = invitations::Entity::find()
        .filter(invitations::Column::InvitedBy.eq(user_id))
        .order_by_asc(invitations::Column::CreatedAt)
        .all(db)
        .await?
        .into_iter()
        .map(|i| ExportedInvitation {
            org_id: i.org_id,
            email: i.email,
            role: i.role,
            created_at: i.created_at,
            accepted: i.accepted_at.is_some(),
        })
        .collect();

    Ok(AccountExport {
        exported_at: Utc::now().into(),
        profile: user.into(),
        organizations,
        conversations: exported,
        documents_uploaded,
        invitations_sent,
    })
}
