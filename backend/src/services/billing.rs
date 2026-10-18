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
use std::sync::Arc;

use lodestone_billing::{
    BillingEvent, CheckoutRequest, PaymentProvider, PriceInfo, PriceTier, SubscriptionState,
    SubscriptionStatus, WebhookEvent,
};
use lodestone_core::{
    plan::Plan,
    pricing::{MonthlyQuote, ProPricing},
};
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
    /// Tarif Pro en vigueur (prix par siège, socle).
    pub pro_pricing: ProPricing,
    /// Montant mensuel Pro : réel en Pro, estimation en Free (sièges actuels),
    /// absent en Sovereign (contrat dédié, pas de socle Pro).
    pub monthly_quote: Option<MonthlyQuote>,
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
    pricing: ProPricing,
    org_id: Uuid,
) -> Result<BillingResponse, AppError> {
    let org = repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let plan = plan_of(&org);
    let subscription = repositories::subscriptions::find_by_org(db, org_id).await?;
    let members = repositories::usage::current(db, org_id).await?.members;

    let monthly_quote = match plan {
        Plan::Sovereign => None,
        Plan::Free | Plan::Pro => {
            // En Pro, les sièges facturés ; sinon (ou pas encore connus), les membres.
            let seats = subscription
                .as_ref()
                .filter(|_| plan == Plan::Pro)
                .and_then(|s| s.seats)
                .map_or(members, i64::from);
            Some(pricing.monthly(u32::try_from(seats.max(1)).unwrap_or(1)))
        }
    };

    Ok(BillingResponse {
        plan,
        billing_enabled,
        subscription: subscription.map(|s| SubscriptionResponse {
            status: s.status,
            seats: s.seats,
        }),
        pro_pricing: pricing,
        monthly_quote,
    })
}

/// Résultat d'une synchronisation des sièges (journalisé, utile en test).
#[derive(Debug, PartialEq, Eq)]
pub enum SeatSync {
    Updated {
        seats: u32,
    },
    Unchanged,
    /// Pas d'abonnement Pro actif, ou ligne d'abonnement encore inconnue.
    NotApplicable,
}

/// Aligne la quantité de l'abonnement Stripe sur le nombre de membres.
/// Sous verrou de l'abonnement : de deux synchronisations simultanées, la
/// dernière lit le nombre final de membres (Stripe ne garde pas une valeur périmée).
pub async fn sync_seats(
    db: &DatabaseConnection,
    provider: &dyn PaymentProvider,
    org_id: Uuid,
) -> Result<SeatSync, AppError> {
    let txn = db.begin().await?;
    let Some(sub) = repositories::subscriptions::find_by_org_for_update(&txn, org_id).await? else {
        return Ok(SeatSync::NotApplicable);
    };
    let (Some(item_id), true) = (
        sub.stripe_item_id.as_deref(),
        SubscriptionStatus::parse(&sub.status).grants_pro(),
    ) else {
        return Ok(SeatSync::NotApplicable);
    };

    let members = repositories::usage::current(&txn, org_id).await?.members;
    let seats = u32::try_from(members.max(1)).unwrap_or(1);
    if sub.seats == i32::try_from(seats).ok() {
        return Ok(SeatSync::Unchanged);
    }

    provider.update_seats(item_id, seats).await.map_err(|e| {
        tracing::error!(%org_id, error = %e, "synchronisation des sièges impossible");
        AppError::Upstream("le service de paiement est momentanément indisponible".into())
    })?;
    repositories::subscriptions::set_seats(&txn, org_id, i32::try_from(seats).unwrap_or(i32::MAX))
        .await?;
    txn.commit().await?;
    tracing::info!(%org_id, seats, "sièges de l'abonnement synchronisés");
    Ok(SeatSync::Updated { seats })
}

/// Lance `sync_seats` en tâche de fond (sans faire attendre la requête).
pub fn spawn_seat_sync(
    db: DatabaseConnection,
    provider: Option<Arc<dyn PaymentProvider>>,
    org_id: Uuid,
) {
    let Some(provider) = provider else {
        return;
    };
    tokio::spawn(async move {
        let _ = sync_seats(&db, provider.as_ref(), org_id).await;
    });
}

/// Paliers attendus du prix Stripe pour `max(socle, sièges × prix)` :
/// forfait = socle jusqu'à `seats_covered_by_floor` sièges, puis prix par siège
/// (mode « volume » : tout le volume au tarif du palier atteint).
pub fn expected_tiers(pricing: ProPricing) -> Vec<PriceTier> {
    vec![
        PriceTier {
            up_to: Some(u64::from(pricing.seats_covered_by_floor())),
            flat_amount: Some(u64::from(pricing.floor_cents)),
            unit_amount: Some(0),
        },
        PriceTier {
            up_to: None,
            flat_amount: None,
            unit_amount: Some(u64::from(pricing.seat_price_cents)),
        },
    ]
}

/// Compare le prix configuré chez Stripe au tarif de la plateforme.
/// `Err` décrit l'écart (montant facturé ≠ montant affiché).
pub fn check_price(price: &PriceInfo, pricing: ProPricing) -> Result<(), String> {
    if price.interval.as_deref() != Some("month") {
        return Err(format!(
            "le prix Pro doit être mensuel (intervalle Stripe : {:?})",
            price.interval
        ));
    }
    if !price.tiered_volume {
        return Err("le prix Pro doit être « par paliers » en mode volume".into());
    }
    // Stripe renvoie parfois `null` au lieu de 0 pour un montant absent.
    let normalize = |tiers: &[PriceTier]| -> Vec<(Option<u64>, u64, u64)> {
        tiers
            .iter()
            .map(|t| {
                (
                    t.up_to,
                    t.flat_amount.unwrap_or(0),
                    t.unit_amount.unwrap_or(0),
                )
            })
            .collect()
    };
    let expected = expected_tiers(pricing);
    if normalize(&price.tiers) != normalize(&expected) {
        return Err(format!(
            "paliers Stripe {:?} ≠ attendus {:?} (socle {} c, siège {} c)",
            normalize(&price.tiers),
            normalize(&expected),
            pricing.floor_cents,
            pricing.seat_price_cents
        ));
    }
    Ok(())
}

/// Au démarrage : signale (sans bloquer) un prix Stripe incohérent avec la
/// config, ou injoignable.
pub async fn verify_stripe_price(provider: &dyn PaymentProvider, pricing: ProPricing) {
    match provider.pro_price().await {
        Ok(price) => match check_price(&price, pricing) {
            Ok(()) => tracing::info!(
                ?pricing,
                "prix Pro Stripe conforme au tarif de la plateforme"
            ),
            Err(e) => tracing::error!(
                "PRIX STRIPE INCOHÉRENT avec PRO_SEAT_PRICE_CENTS / PRO_FLOOR_CENTS : {e}"
            ),
        },
        Err(e) => tracing::warn!(error = %e, "prix Pro Stripe non vérifié"),
    }
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
                    item_id: None,
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
            item_id: state.item_id.as_deref(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn price(tiers: Vec<PriceTier>) -> PriceInfo {
        PriceInfo {
            currency: "eur".into(),
            interval: Some("month".into()),
            tiered_volume: true,
            tiers,
        }
    }

    #[test]
    fn paliers_attendus() {
        let tiers = expected_tiers(ProPricing::default());
        assert_eq!(tiers[0].up_to, Some(5));
        assert_eq!(tiers[0].flat_amount, Some(14900));
        assert_eq!(tiers[1].unit_amount, Some(2900));
    }

    #[test]
    fn prix_conforme() {
        let p = ProPricing::default();
        assert_eq!(check_price(&price(expected_tiers(p)), p), Ok(()));

        // Montants absents renvoyés à `null` par Stripe : équivalents à 0.
        let mut tiers = expected_tiers(p);
        tiers[0].unit_amount = None;
        assert_eq!(check_price(&price(tiers), p), Ok(()));
    }

    #[test]
    fn prix_incoherents() {
        let p = ProPricing::default();
        let wrong_floor = expected_tiers(ProPricing {
            floor_cents: 9900,
            ..p
        });
        assert!(
            check_price(&price(wrong_floor), p).is_err(),
            "socle différent"
        );

        let wrong_seat = expected_tiers(ProPricing {
            seat_price_cents: 2500,
            ..p
        });
        assert!(
            check_price(&price(wrong_seat), p).is_err(),
            "prix par siège différent"
        );

        let mut graduated = price(expected_tiers(p));
        graduated.tiered_volume = false;
        assert!(
            check_price(&graduated, p).is_err(),
            "mode graduated / prix simple"
        );

        let mut yearly = price(expected_tiers(p));
        yearly.interval = Some("year".into());
        assert!(check_price(&yearly, p).is_err(), "annuel");
    }
}
