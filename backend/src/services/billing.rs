//! Abonnements via Stripe : Pro (mensuel ou annuel, changement de périodicité)
//! et Sovereign (licence annuelle + mise en service facturée une seule fois),
//! puis webhooks qui font évoluer l'abonnement et le plan de l'org.
//! Les quotas lisent le plan en base à chaque action : un changement de plan
//! est donc effectif immédiatement.

use std::sync::Arc;

use sea_orm::{ConnectionTrait, DatabaseConnection, DbErr, Statement, TransactionTrait};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    config::StripeConfig,
    dto::organizations::plan_of,
    error::AppError,
    repositories::{
        self,
        subscriptions::{Subscription, SubscriptionUpdate},
    },
};
use lodestone_billing::{
    BillingEvent, CheckoutItem, CheckoutRequest, PaymentProvider, PriceInfo, PriceTier,
    SubscriptionState, SubscriptionStatus, WebhookEvent,
};
use lodestone_core::{
    plan::Plan,
    pricing::{BillingInterval, ProPricing, Quote, SovereignPricing},
};

/// Produit d'un abonnement (métadonnée `product` côté Stripe).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Product {
    Pro,
    Sovereign,
}

impl Product {
    fn as_str(self) -> &'static str {
        match self {
            Product::Pro => "pro",
            Product::Sovereign => "sovereign",
        }
    }

    /// Valeur absente ou inconnue : Pro (les premiers abonnements n'avaient pas l'étiquette).
    fn parse(s: Option<&str>) -> Self {
        if s == Some("sovereign") {
            Product::Sovereign
        } else {
            Product::Pro
        }
    }
}

const NOT_CONFIGURED: &str = "la facturation n'est pas configurée sur ce serveur";
const UNAVAILABLE: &str = "le service de paiement est momentanément indisponible";
const ANNUAL_NOT_OFFERED: &str = "l'abonnement annuel n'est pas proposé sur ce serveur";

/// Politique de changement de périodicité, renvoyée telle quelle par l'API.
pub const POLICY_TO_YEAR: &str = "Passage à l'annuel immédiat : le reste de la période mensuelle \
     est crédité au prorata et l'année est facturée tout de suite.";
pub const POLICY_TO_MONTH: &str = "Retour au mensuel à l'échéance annuelle : l'année payée reste \
     acquise (pas de remboursement), la facturation mensuelle démarre à la date d'échéance.";
pub const POLICY_SEATS: &str =
    "Sièges ajoutés ou retirés en cours de période : facturés ou crédités au prorata.";

#[derive(Debug, Serialize)]
pub struct CheckoutResponse {
    /// Page de paiement Stripe vers laquelle rediriger l'utilisateur.
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct SubscriptionResponse {
    pub status: String,
    pub seats: Option<i32>,
    pub interval: Option<String>,
    /// Fin de la période en cours (horodatage Unix).
    pub current_period_end: Option<i64>,
    /// Bascule programmée à l'échéance (`month`), sinon `null`.
    pub scheduled_interval: Option<String>,
}

/// Les deux formules pour un même nombre de sièges (aide au choix).
#[derive(Debug, Serialize)]
pub struct Quotes {
    pub month: Quote,
    /// `None` si l'annuel n'est pas proposé.
    pub year: Option<Quote>,
}

#[derive(Debug, Serialize)]
pub struct SwitchPolicy {
    pub to_year: &'static str,
    pub to_month: &'static str,
    pub seats: &'static str,
}

const SWITCH_POLICY: SwitchPolicy = SwitchPolicy {
    to_year: POLICY_TO_YEAR,
    to_month: POLICY_TO_MONTH,
    seats: POLICY_SEATS,
};

#[derive(Debug, Serialize)]
pub struct BillingResponse {
    pub plan: Plan,
    pub billing_enabled: bool,
    pub annual_available: bool,
    pub subscription: Option<SubscriptionResponse>,
    /// Tarif Pro en vigueur (prix par siège, socle, remise annuelle).
    pub pro_pricing: ProPricing,
    /// Ce que paie l'org : réel en Pro (selon sa périodicité), estimation
    /// mensuelle en Free, absent en Sovereign (contrat dédié, pas de socle Pro).
    pub current_quote: Option<Quote>,
    /// Mensuel et annuel pour les sièges actuels (absent en Sovereign).
    pub quotes: Option<Quotes>,
    pub switch_policy: SwitchPolicy,
    /// Tarif et mise en service, pour une org Sovereign uniquement.
    pub sovereign: Option<SovereignBilling>,
}

#[derive(Debug, Serialize)]
pub struct SovereignBilling {
    pub pricing: SovereignPricing,
    /// Paiement par Stripe possible (prix Sovereign configurés).
    pub checkout_available: bool,
    /// Date de paiement de la mise en service (horodatage Unix), si payée.
    pub setup_fee_paid_at: Option<i64>,
    /// Montant de la prochaine souscription : licence, plus la mise en service
    /// si elle n'a jamais été payée.
    pub next_checkout_cents: u64,
}

#[derive(Debug, Serialize)]
pub struct SovereignCheckoutResponse {
    pub url: String,
    /// La mise en service figure dans cette session (jamais si déjà payée).
    pub setup_fee_included: bool,
}

#[derive(Debug, Serialize)]
pub struct IntervalChangeResponse {
    pub interval: BillingInterval,
    /// `now` (vers l'annuel) ou `at_period_end` (vers le mensuel).
    pub effective: &'static str,
    /// Date de bascule (horodatage Unix) si programmée.
    pub effective_at: Option<i64>,
    pub policy: &'static str,
}

fn provider_or_unavailable<'a>(
    provider: Option<&'a dyn PaymentProvider>,
    stripe: Option<&'a StripeConfig>,
) -> Result<(&'a dyn PaymentProvider, &'a StripeConfig), AppError> {
    match (provider, stripe) {
        (Some(p), Some(s)) => Ok((p, s)),
        _ => Err(AppError::Unavailable(NOT_CONFIGURED.into())),
    }
}

/// Prix Stripe d'une périodicité.
fn price_id(stripe: &StripeConfig, interval: BillingInterval) -> Result<&str, AppError> {
    match interval {
        BillingInterval::Month => Ok(&stripe.pro_price_id),
        BillingInterval::Year => stripe
            .pro_annual_price_id
            .as_deref()
            .ok_or_else(|| AppError::InvalidInput(ANNUAL_NOT_OFFERED.into())),
    }
}

fn upstream(org_id: Uuid, what: &str) -> impl FnOnce(lodestone_billing::BillingError) -> AppError {
    let what = what.to_string();
    move |e| {
        tracing::error!(%org_id, error = %e, "{what}");
        AppError::Upstream(UNAVAILABLE.into())
    }
}

/// Démarre le passage en Pro : session Stripe (mensuelle ou annuelle) pour
/// autant de sièges que de membres.
pub async fn start_checkout(
    db: &DatabaseConnection,
    provider: Option<&dyn PaymentProvider>,
    stripe: Option<&StripeConfig>,
    app_base_url: &str,
    org_id: Uuid,
    owner_id: Uuid,
    interval: BillingInterval,
) -> Result<CheckoutResponse, AppError> {
    let (provider, stripe) = provider_or_unavailable(provider, stripe)?;
    let price_id = price_id(stripe, interval)?.to_string();

    let org = repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    match plan_of(&org) {
        Plan::Free => {}
        Plan::Pro => return Err(AppError::Conflict("l'organisation est déjà en Pro".into())),
        Plan::Sovereign => {
            return Err(AppError::Conflict(
                "organisation Sovereign : utilisez la souscription Sovereign".into(),
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
            items: vec![CheckoutItem {
                price_id,
                quantity: seats,
            }],
            metadata: vec![("product".into(), Product::Pro.as_str().into())],
            customer_email: owner.email,
            success_url: format!("{app_base_url}/orgs/{org_id}/billing?checkout=success"),
            cancel_url: format!("{app_base_url}/orgs/{org_id}/billing?checkout=cancel"),
        })
        .await
        .map_err(upstream(
            org_id,
            "création de la session de paiement impossible",
        ))?;

    Ok(CheckoutResponse { url: session.url })
}

fn seats_u32(n: i64) -> u32 {
    u32::try_from(n.max(1)).unwrap_or(1)
}

/// Activation Sovereign : licence annuelle, plus la mise en service si elle
/// n'a jamais été payée (ligne ponctuelle : Stripe ne la facture que sur la
/// première facture, jamais aux renouvellements).
pub async fn start_sovereign_checkout(
    db: &DatabaseConnection,
    provider: Option<&dyn PaymentProvider>,
    stripe: Option<&StripeConfig>,
    app_base_url: &str,
    org_id: Uuid,
    owner_id: Uuid,
) -> Result<SovereignCheckoutResponse, AppError> {
    let (provider, stripe) = provider_or_unavailable(provider, stripe)?;
    let prices = stripe.sovereign_prices.as_ref().ok_or_else(|| {
        AppError::Unavailable(
            "la souscription Sovereign n'est pas configurée sur ce serveur".into(),
        )
    })?;

    let org = repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    if plan_of(&org) != Plan::Sovereign {
        return Err(AppError::Conflict(
            "réservé aux organisations Sovereign (contrat préalable avec Lodestone)".into(),
        ));
    }
    if repositories::subscriptions::find_by_org(db, org_id)
        .await?
        .is_some_and(|s| {
            Product::parse(Some(&s.product)) == Product::Sovereign
                && SubscriptionStatus::parse(&s.status).grants_pro()
        })
    {
        return Err(AppError::Conflict(
            "la licence Sovereign est déjà active".into(),
        ));
    }

    let setup_paid = repositories::organizations::sovereign_setup_paid_at(db, org_id)
        .await?
        .is_some();
    let mut items = vec![CheckoutItem {
        price_id: prices.license_price_id.clone(),
        quantity: 1,
    }];
    if !setup_paid {
        items.push(CheckoutItem {
            price_id: prices.setup_price_id.clone(),
            quantity: 1,
        });
    }
    let owner = repositories::users::find_by_id(db, owner_id)
        .await?
        .ok_or(AppError::Unauthorized)?;

    let session = provider
        .create_checkout(&CheckoutRequest {
            org_id,
            items,
            metadata: vec![
                ("product".into(), Product::Sovereign.as_str().into()),
                (
                    "setup_fee".into(),
                    if setup_paid {
                        "already_paid"
                    } else {
                        "included"
                    }
                    .into(),
                ),
            ],
            customer_email: owner.email,
            success_url: format!("{app_base_url}/orgs/{org_id}/billing?checkout=success"),
            cancel_url: format!("{app_base_url}/orgs/{org_id}/billing?checkout=cancel"),
        })
        .await
        .map_err(upstream(
            org_id,
            "création de la session Sovereign impossible",
        ))?;

    Ok(SovereignCheckoutResponse {
        url: session.url,
        setup_fee_included: !setup_paid,
    })
}

pub struct StatusContext {
    pub billing_enabled: bool,
    pub annual_available: bool,
    pub sovereign_checkout_available: bool,
    pub pricing: ProPricing,
    pub sovereign_pricing: SovereignPricing,
}

pub async fn status(
    db: &DatabaseConnection,
    ctx: StatusContext,
    org_id: Uuid,
) -> Result<BillingResponse, AppError> {
    let StatusContext {
        billing_enabled,
        annual_available,
        sovereign_checkout_available,
        pricing,
        sovereign_pricing,
    } = ctx;
    let org = repositories::organizations::find_by_id(db, org_id)
        .await?
        .ok_or(AppError::NotFound)?;
    let plan = plan_of(&org);
    let subscription = repositories::subscriptions::find_by_org(db, org_id).await?;
    let members = repositories::usage::current(db, org_id).await?.members;

    let (current_quote, quotes) = match plan {
        Plan::Sovereign => (None, None),
        Plan::Free | Plan::Pro => {
            // En Pro, les sièges facturés et la périodicité réelle ;
            // sinon (ou pas encore connus), les membres et le mensuel.
            let pro_sub = subscription.as_ref().filter(|_| plan == Plan::Pro);
            let seats = seats_u32(pro_sub.and_then(|s| s.seats).map_or(members, i64::from));
            let interval = pro_sub
                .and_then(|s| s.billing_interval.as_deref())
                .and_then(BillingInterval::parse)
                .unwrap_or(BillingInterval::Month);
            (
                Some(pricing.quote(seats, interval)),
                Some(Quotes {
                    month: pricing.monthly(seats),
                    year: annual_available.then(|| pricing.annual(seats)),
                }),
            )
        }
    };

    let sovereign = if plan == Plan::Sovereign {
        let paid_at = repositories::organizations::sovereign_setup_paid_at(db, org_id).await?;
        Some(SovereignBilling {
            pricing: sovereign_pricing,
            checkout_available: sovereign_checkout_available,
            setup_fee_paid_at: paid_at.map(|d| d.timestamp()),
            next_checkout_cents: sovereign_pricing.first_invoice_cents(paid_at.is_some()),
        })
    } else {
        None
    };

    Ok(BillingResponse {
        sovereign,
        plan,
        billing_enabled,
        annual_available,
        subscription: subscription.map(|s| SubscriptionResponse {
            status: s.status,
            seats: s.seats,
            interval: s.billing_interval,
            current_period_end: s.current_period_end,
            scheduled_interval: s.scheduled_interval,
        }),
        pro_pricing: pricing,
        current_quote,
        quotes,
        switch_policy: SWITCH_POLICY,
    })
}

/// Change la périodicité d'un abonnement Pro actif (voir `POLICY_TO_*`).
pub async fn change_interval(
    db: &DatabaseConnection,
    provider: Option<&dyn PaymentProvider>,
    stripe: Option<&StripeConfig>,
    org_id: Uuid,
    target: BillingInterval,
) -> Result<IntervalChangeResponse, AppError> {
    let (provider, stripe) = provider_or_unavailable(provider, stripe)?;
    let monthly_price = price_id(stripe, BillingInterval::Month)?;
    let annual_price = price_id(stripe, BillingInterval::Year)?;

    // Verrou de l'abonnement pendant l'appel à Stripe : pas de double bascule,
    // ni de synchro de sièges concurrente.
    let txn = db.begin().await?;
    let sub = repositories::subscriptions::find_by_org_for_update(&txn, org_id)
        .await?
        .filter(|s| {
            Product::parse(Some(&s.product)) == Product::Pro
                && SubscriptionStatus::parse(&s.status).grants_pro()
        })
        .ok_or_else(|| AppError::Conflict("aucun abonnement Pro actif".into()))?;
    let (Some(item_id), Some(current)) = (
        sub.stripe_item_id.as_deref(),
        sub.billing_interval
            .as_deref()
            .and_then(BillingInterval::parse),
    ) else {
        return Err(AppError::Conflict(
            "abonnement pas encore synchronisé avec Stripe, réessayez dans un instant".into(),
        ));
    };
    if sub.scheduled_interval.is_some() {
        return Err(AppError::Conflict(
            "un changement de périodicité est déjà programmé".into(),
        ));
    }
    if current == target {
        return Err(AppError::Conflict(format!(
            "l'abonnement est déjà en facturation {}",
            match target {
                BillingInterval::Month => "mensuelle",
                BillingInterval::Year => "annuelle",
            }
        )));
    }
    let members = repositories::usage::current(&txn, org_id).await?.members;
    let seats = seats_u32(members);

    let response = match target {
        BillingInterval::Year => {
            provider
                .change_price_now(item_id, annual_price, seats)
                .await
                .map_err(upstream(org_id, "passage à l'annuel impossible"))?;
            repositories::subscriptions::set_interval(&txn, org_id, "year", None).await?;
            IntervalChangeResponse {
                interval: target,
                effective: "now",
                effective_at: None,
                policy: POLICY_TO_YEAR,
            }
        }
        BillingInterval::Month => {
            let at = provider
                .schedule_price_change(
                    &sub.stripe_subscription_id,
                    annual_price,
                    monthly_price,
                    seats,
                )
                .await
                .map_err(upstream(org_id, "retour au mensuel impossible"))?;
            repositories::subscriptions::set_interval(&txn, org_id, "year", Some("month")).await?;
            IntervalChangeResponse {
                interval: target,
                effective: "at_period_end",
                effective_at: Some(at),
                policy: POLICY_TO_MONTH,
            }
        }
    };
    txn.commit().await?;
    tracing::info!(%org_id, ?target, effective = response.effective, "périodicité modifiée");
    Ok(response)
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
    // Seul le Pro est facturé au siège (la licence Sovereign est forfaitaire).
    let (Some(item_id), true) = (
        sub.stripe_item_id.as_deref(),
        Product::parse(Some(&sub.product)) == Product::Pro
            && SubscriptionStatus::parse(&sub.status).grants_pro(),
    ) else {
        return Ok(SeatSync::NotApplicable);
    };

    let members = repositories::usage::current(&txn, org_id).await?.members;
    let seats = seats_u32(members);
    if sub.seats == i32::try_from(seats).ok() {
        return Ok(SeatSync::Unchanged);
    }

    provider
        .update_seats(item_id, seats)
        .await
        .map_err(upstream(org_id, "synchronisation des sièges impossible"))?;
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

/// Paliers attendus du prix Stripe d'une périodicité pour `max(socle, sièges × prix)` :
/// forfait = socle jusqu'à `seats_covered_by_floor` sièges, puis prix par siège
/// (mode « volume » : tout le volume au tarif du palier atteint).
pub fn expected_tiers(pricing: ProPricing, interval: BillingInterval) -> Vec<PriceTier> {
    let prices = pricing.unit_prices(interval);
    vec![
        PriceTier {
            up_to: Some(u64::from(pricing.seats_covered_by_floor(interval))),
            flat_amount: Some(prices.floor_cents),
            unit_amount: Some(0),
        },
        PriceTier {
            up_to: None,
            flat_amount: None,
            unit_amount: Some(prices.seat_cents),
        },
    ]
}

/// Compare un prix configuré chez Stripe au tarif de la plateforme.
/// `Err` décrit l'écart (montant facturé ≠ montant affiché).
pub fn check_price(
    price: &PriceInfo,
    pricing: ProPricing,
    interval: BillingInterval,
) -> Result<(), String> {
    if price.interval.as_deref() != Some(interval.as_str()) {
        return Err(format!(
            "périodicité Stripe {:?}, attendue {:?}",
            price.interval,
            interval.as_str()
        ));
    }
    if !price.tiered_volume {
        return Err("le prix doit être « par paliers » en mode volume".into());
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
    let expected = expected_tiers(pricing, interval);
    if normalize(&price.tiers) != normalize(&expected) {
        return Err(format!(
            "paliers Stripe {:?} ≠ attendus {:?}",
            normalize(&price.tiers),
            normalize(&expected)
        ));
    }
    Ok(())
}

/// Au démarrage : signale (sans bloquer) un prix Stripe incohérent avec la
/// config, ou injoignable — pour le mensuel et, s'il est proposé, l'annuel.
/// Licence Sovereign : prix simple, récurrent annuel, au montant configuré.
/// Mise en service : prix simple, PONCTUEL (sinon il serait refacturé chaque année).
pub fn check_sovereign_price(
    price: &PriceInfo,
    expected_cents: u32,
    one_time: bool,
) -> Result<(), String> {
    if price.tiered_volume || !price.tiers.is_empty() {
        return Err("prix simple attendu (pas de paliers)".into());
    }
    if one_time {
        if !price.one_time || price.interval.is_some() {
            return Err("la mise en service doit être un prix PONCTUEL (one-time)".into());
        }
    } else if price.one_time || price.interval.as_deref() != Some("year") {
        return Err(format!(
            "la licence doit être récurrente annuelle (périodicité Stripe : {:?})",
            price.interval
        ));
    }
    if price.unit_amount != Some(u64::from(expected_cents)) {
        return Err(format!(
            "montant Stripe {:?} c ≠ attendu {expected_cents} c",
            price.unit_amount
        ));
    }
    Ok(())
}

pub async fn verify_stripe_prices(
    provider: &dyn PaymentProvider,
    stripe: &StripeConfig,
    pricing: ProPricing,
    sovereign: SovereignPricing,
) {
    if let Some(prices) = &stripe.sovereign_prices {
        for (price_id, cents, one_time) in [
            (
                &prices.license_price_id,
                sovereign.license_cents_per_year,
                false,
            ),
            (&prices.setup_price_id, sovereign.setup_cents, true),
        ] {
            match provider.price(price_id).await {
                Ok(price) => match check_sovereign_price(&price, cents, one_time) {
                    Ok(()) => tracing::info!(price_id, "prix Sovereign conforme"),
                    Err(e) => tracing::error!(
                        price_id,
                        "PRIX STRIPE SOVEREIGN INCOHÉRENT (SOVEREIGN_*) : {e}"
                    ),
                },
                Err(e) => tracing::warn!(price_id, error = %e, "prix Sovereign non vérifié"),
            }
        }
    }

    let mut prices = vec![(BillingInterval::Month, stripe.pro_price_id.as_str())];
    if let Some(annual) = &stripe.pro_annual_price_id {
        prices.push((BillingInterval::Year, annual.as_str()));
    }
    for (interval, price_id) in prices {
        match provider.price(price_id).await {
            Ok(price) => match check_price(&price, pricing, interval) {
                Ok(()) => tracing::info!(
                    price_id,
                    interval = interval.as_str(),
                    "prix Stripe conforme"
                ),
                Err(e) => tracing::error!(
                    price_id,
                    "PRIX STRIPE INCOHÉRENT avec le tarif de la plateforme (PRO_*) : {e}"
                ),
            },
            Err(e) => tracing::warn!(price_id, error = %e, "prix Stripe non vérifié"),
        }
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
            product,
            setup_fee_included,
        } => {
            let product = Product::parse(product.as_deref());
            apply_subscription(
                &txn,
                event.created,
                org_id,
                product,
                // La mise en service est payée avec la session qui l'incluait.
                setup_fee_included,
                SubscriptionState {
                    subscription_id,
                    customer_id,
                    org_id: Some(org_id),
                    status: SubscriptionStatus::Active,
                    seats: None,
                    item_id: None,
                    price_id: None,
                    interval: None,
                    current_period_end: None,
                    product: Some(product.as_str().into()),
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
                Some(org_id) => {
                    // Produit : celui de l'événement, sinon celui déjà enregistré.
                    let known = repositories::subscriptions::find_by_org(&txn, org_id)
                        .await?
                        .map(|s| s.product);
                    let product = Product::parse(state.product.as_deref().or(known.as_deref()));
                    apply_subscription(&txn, event.created, org_id, product, false, state).await?
                }
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
    product: Product,
    setup_fee_paid: bool,
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
    let is_sovereign_org = plan == Plan::Sovereign.as_str();
    match product {
        Product::Pro if is_sovereign_org => {
            return Ok(Outcome::Ignored(
                "abonnement Pro pour une organisation Sovereign",
            ));
        }
        Product::Sovereign if !is_sovereign_org => {
            return Ok(Outcome::Ignored(
                "abonnement Sovereign pour une organisation non Sovereign",
            ));
        }
        _ => {}
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

    // Seules les périodicités connues sont enregistrées (contrainte CHECK).
    let interval = state
        .interval
        .as_deref()
        .and_then(BillingInterval::parse)
        .map(|i| i.as_str());
    repositories::subscriptions::upsert(
        conn,
        SubscriptionUpdate {
            org_id,
            stripe_subscription_id: &state.subscription_id,
            stripe_customer_id: state.customer_id.as_deref(),
            status: state.status.as_str(),
            seats: state.seats.and_then(|s| i32::try_from(s).ok()),
            item_id: state.item_id.as_deref(),
            interval,
            current_period_end: state.current_period_end,
            product: product.as_str(),
            event_at,
        },
    )
    .await?;

    if product == Product::Sovereign {
        // Le plan reste Sovereign (contrat) ; seule la mise en service est notée.
        if setup_fee_paid {
            repositories::organizations::mark_sovereign_setup_paid(conn, org_id).await?;
        }
        return Ok(Outcome::Applied {
            org_id,
            plan: Plan::Sovereign,
        });
    }

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

    fn price(interval: &str, tiers: Vec<PriceTier>) -> PriceInfo {
        PriceInfo {
            currency: "eur".into(),
            interval: Some(interval.into()),
            one_time: false,
            unit_amount: None,
            tiered_volume: true,
            tiers,
        }
    }

    #[test]
    fn prix_sovereign() {
        let license = lodestone_billing::MockPaymentProvider::simple_price(Some("year"), 1_800_000);
        let setup = lodestone_billing::MockPaymentProvider::simple_price(None, 600_000);
        assert_eq!(check_sovereign_price(&license, 1_800_000, false), Ok(()));
        assert_eq!(check_sovereign_price(&setup, 600_000, true), Ok(()));

        // Mise en service créée en récurrent : elle serait refacturée chaque année.
        let recurring_setup =
            lodestone_billing::MockPaymentProvider::simple_price(Some("year"), 600_000);
        assert!(check_sovereign_price(&recurring_setup, 600_000, true).is_err());
        // Licence mensuelle, montant faux, licence ponctuelle.
        let monthly =
            lodestone_billing::MockPaymentProvider::simple_price(Some("month"), 1_800_000);
        assert!(check_sovereign_price(&monthly, 1_800_000, false).is_err());
        assert!(check_sovereign_price(&license, 1_700_000, false).is_err());
        assert!(check_sovereign_price(&setup, 1_800_000, false).is_err());
    }

    #[test]
    fn paliers_attendus() {
        let p = ProPricing::default();
        let month = expected_tiers(p, BillingInterval::Month);
        assert_eq!(
            (month[0].up_to, month[0].flat_amount, month[1].unit_amount),
            (Some(5), Some(14900), Some(2900))
        );
        let year = expected_tiers(p, BillingInterval::Year);
        assert_eq!(
            (year[0].up_to, year[0].flat_amount, year[1].unit_amount),
            (Some(5), Some(151_980), Some(29_580))
        );
    }

    #[test]
    fn prix_conforme() {
        let p = ProPricing::default();
        for (interval, label) in [
            (BillingInterval::Month, "month"),
            (BillingInterval::Year, "year"),
        ] {
            assert_eq!(
                check_price(&price(label, expected_tiers(p, interval)), p, interval),
                Ok(())
            );
        }
        // Montants absents renvoyés à `null` par Stripe : équivalents à 0.
        let mut tiers = expected_tiers(p, BillingInterval::Month);
        tiers[0].unit_amount = None;
        assert_eq!(
            check_price(&price("month", tiers), p, BillingInterval::Month),
            Ok(())
        );
    }

    #[test]
    fn prix_incoherents() {
        let p = ProPricing::default();
        let month = BillingInterval::Month;
        let wrong_floor = expected_tiers(
            ProPricing {
                floor_cents: 9900,
                ..p
            },
            month,
        );
        assert!(check_price(&price("month", wrong_floor), p, month).is_err());

        // Annuel créé sans la remise : refusé.
        let no_discount = expected_tiers(
            ProPricing {
                annual_discount_percent: 0,
                ..p
            },
            BillingInterval::Year,
        );
        assert!(check_price(&price("year", no_discount), p, BillingInterval::Year).is_err());

        let mut graduated = price("month", expected_tiers(p, month));
        graduated.tiered_volume = false;
        assert!(check_price(&graduated, p, month).is_err());

        // Prix mensuel déclaré comme annuel (et inversement).
        assert!(check_price(&price("year", expected_tiers(p, month)), p, month).is_err());
    }
}
