//! Abonnement Pro via Stripe : session de paiement, puis webhooks qui font
//! évoluer le plan de l'org. Les quotas lisent le plan en base à chaque
//! action : un changement de plan est donc effectif immédiatement.

use sea_orm::{DatabaseConnection, DbErr, Statement, TransactionTrait};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    dto::organizations::plan_of,
    error::AppError,
    repositories::{
        self,
        subscriptions::{Subscription, SubscriptionUpdate},
    },
};
use lodestone_billing::{
    BillingEvent, CheckoutRequest, PaymentProvider, SubscriptionState, SubscriptionStatus,
    WebhookEvent,
};
use lodestone_core::plan::Plan;
use sea_orm::ConnectionTrait;

const NOT_CONFIGURED: &str = "la facturation n'est pas configurée sur ce serveur";

#[derive(Debug, Serialize)]
pub struct CheckoutResponse {
    /// Page de paiement Stripe vers laquelle rediriger l'utilisateur.
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionResponse {
    pub status: String,
    pub seats: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct BillingResponse {
    pub plan: Plan,
    pub billing_enabled: bool,
    pub subscription: Option<SubscriptionResponse>,
}

/// Démarre le passage en Pro : session Stripe pour autant de sièges que de membres.
pub async fn start_checkout(
    db: &DatabaseConnection,
    provider: Option<&dyn PaymentProvider>,
    app_base_url: &str,
    org_id: Uuid,
    owner_id: Uuid,
) -> Result<CheckoutResponse, AppError> {
    let provider = provider.ok_or_else(|| AppError::Unavailable(NOT_CONFIGURED.into()))?;

    let org = repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    match plan_of(&org) {
        Plan::Free => {}
        Plan::Pro => return Err(AppError::Conflict("l'organisation est déjà en Pro".into())),
        Plan::Sovereign => {
            return Err(AppError::Conflict(
                "le plan Sovereign fait l'objet d'un contrat dédié".into(),
            ));
        }
    }

    let owner = repositories::users::find_by_id(db, owner_id)
        .await?
        .ok_or(AppError::Unauthorized)?;
    let usage = repositories::usage::current(db, org_id).await?;
    let seats = u32::try_from(usage.members.max(1)).unwrap_or(1);

    let session = provider
        .create_checkout(&CheckoutRequest {
            org_id,
            seats,
            customer_email: owner.email,
            success_url: format!("{app_base_url}/orgs/{org_id}/billing?checkout=success"),
            cancel_url: format!("{app_base_url}/orgs/{org_id}/billing?checkout=cancel"),
        })
        .await
        .map_err(|e| {
            tracing::error!(%org_id, error = %e, "création de la session de paiement impossible");
            AppError::Upstream("le service de paiement est momentanément indisponible".into())
        })?;

    Ok(CheckoutResponse { url: session.url })
}

pub async fn status(
    db: &DatabaseConnection,
    billing_enabled: bool,
    org_id: Uuid,
) -> Result<BillingResponse, AppError> {
    let org = repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let subscription = repositories::subscriptions::find_by_org(db, org_id)
        .await?
        .map(|s| SubscriptionResponse {
            status: s.status,
            seats: s.seats,
        });
    Ok(BillingResponse {
        plan: plan_of(&org),
        billing_enabled,
        subscription,
    })
}

/// Ce qu'il est advenu d'un événement (journalisé, utile en test).
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Applied {
        org_id: Uuid,
        plan: Plan,
    },
    Duplicate,
    /// Plus ancien que le dernier événement appliqué à cet abonnement.
    Stale,
    /// Org inconnue, ancien abonnement, plan Sovereign, type non exploité…
    Ignored(&'static str),
}

/// Applique un événement Stripe authentifié. Tout se passe dans une
/// transaction : l'événement n'est marqué traité que si son effet est écrit.
pub async fn apply_event(db: &DatabaseConnection, event: WebhookEvent) -> Result<Outcome, DbErr> {
    let txn = db.begin().await?;
    if !repositories::subscriptions::record_event(&txn, &event.id, &event.event_type).await? {
        return Ok(Outcome::Duplicate);
    }

    let outcome = match event.event {
        BillingEvent::CheckoutCompleted {
            org_id,
            customer_id,
            subscription_id,
        } => {
            apply_subscription(
                &txn,
                event.created,
                org_id,
                SubscriptionState {
                    subscription_id,
                    customer_id,
                    org_id: Some(org_id),
                    status: SubscriptionStatus::Active,
                    seats: None,
                },
            )
            .await?
        }
        BillingEvent::Subscription(state) => {
            let org_id = match state.org_id {
                Some(id) => Some(id),
                None => {
                    repositories::subscriptions::org_of_subscription(&txn, &state.subscription_id)
                        .await?
                }
            };
            match org_id {
                Some(org_id) => apply_subscription(&txn, event.created, org_id, state).await?,
                None => Outcome::Ignored("abonnement sans organisation connue"),
            }
        }
        BillingEvent::Ignored => Outcome::Ignored("type d'événement non exploité"),
    };

    txn.commit().await?;
    tracing::info!(event_id = %event.id, event_type = %event.event_type, ?outcome, "webhook Stripe");
    Ok(outcome)
}

async fn apply_subscription<C: ConnectionTrait>(
    conn: &C,
    event_at: i64,
    org_id: Uuid,
    state: SubscriptionState,
) -> Result<Outcome, DbErr> {
    // Verrou sur l'org : deux webhooks simultanés pour elle s'appliquent l'un après l'autre.
    let Some(row) = conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT plan FROM organizations WHERE id = $1 FOR UPDATE",
            [org_id.into()],
        ))
        .await?
    else {
        return Ok(Outcome::Ignored("organisation inconnue"));
    };
    let plan: String = row.try_get("", "plan")?;
    if plan == Plan::Sovereign.as_str() {
        return Ok(Outcome::Ignored("plan Sovereign géré hors Stripe"));
    }

    let current: Option<Subscription> =
        repositories::subscriptions::find_by_org_for_update(conn, org_id).await?;
    if let Some(current) = &current {
        if event_at < current.last_event_at {
            return Ok(Outcome::Stale);
        }
        // La fin d'un ANCIEN abonnement ne doit pas rétrograder l'org qui en a
        // un nouveau ; un nouvel abonnement actif, lui, remplace l'ancien.
        if current.stripe_subscription_id != state.subscription_id && !state.status.grants_pro() {
            return Ok(Outcome::Ignored("événement d'un ancien abonnement"));
        }
    }

    repositories::subscriptions::upsert(
        conn,
        SubscriptionUpdate {
            org_id,
            stripe_subscription_id: &state.subscription_id,
            stripe_customer_id: state.customer_id.as_deref(),
            status: state.status.as_str(),
            seats: state.seats.and_then(|s| i32::try_from(s).ok()),
            event_at,
        },
    )
    .await?;

    let new_plan = if state.status.grants_pro() {
        Plan::Pro
    } else {
        Plan::Free
    };
    repositories::organizations::set_plan(conn, org_id, new_plan).await?;
    Ok(Outcome::Applied {
        org_id,
        plan: new_plan,
    })
}
