use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};

use crate::{
    error::AppError,
    extractors::org_member::{Admin, OrgMember, Owner},
    services::billing,
    state::AppState,
};
use lodestone_billing::{parse_event, verify_signature};
use lodestone_core::{plan::Plan, pricing::BillingInterval};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct CheckoutBody {
    /// `month` (par défaut) ou `year`.
    #[serde(default)]
    pub interval: Option<BillingInterval>,
}

/// POST /api/orgs/{id}/billing/checkout — owner : URL de paiement Stripe.
/// Corps optionnel : `{"interval": "month" | "year"}`.
pub async fn checkout(
    State(state): State<AppState>,
    member: OrgMember<Owner>,
    body: Option<Json<CheckoutBody>>,
) -> Result<impl IntoResponse, AppError> {
    let Json(body) = body.unwrap_or_default();
    let response = billing::start_checkout(
        &state.db,
        state.billing.as_deref(),
        state.config.stripe.as_ref(),
        &state.config.app_base_url,
        member.org_id,
        member.user_id,
        body.interval.unwrap_or(BillingInterval::Month),
    )
    .await?;
    Ok(Json(response))
}

#[derive(Debug, Deserialize)]
pub struct IntervalBody {
    pub interval: BillingInterval,
}

/// POST /api/orgs/{id}/billing/interval — owner : passer au mensuel / à l'annuel.
pub async fn change_interval(
    State(state): State<AppState>,
    member: OrgMember<Owner>,
    Json(body): Json<IntervalBody>,
) -> Result<impl IntoResponse, AppError> {
    let response = billing::change_interval(
        &state.db,
        state.billing.as_deref(),
        state.config.stripe.as_ref(),
        member.org_id,
        body.interval,
    )
    .await?;
    Ok(Json(response))
}

/// GET /api/orgs/{id}/billing — admin : plan, abonnement, tarifs et politique.
pub async fn status(
    State(state): State<AppState>,
    member: OrgMember<Admin>,
) -> Result<impl IntoResponse, AppError> {
    let annual_available = state
        .config
        .stripe
        .as_ref()
        .is_some_and(|s| s.pro_annual_price_id.is_some());
    let response = billing::status(
        &state.db,
        state.billing.is_some(),
        annual_available,
        state.config.pro_pricing,
        member.org_id,
    )
    .await?;
    Ok(Json(response))
}

/// POST /api/billing/webhook — appelé par Stripe, authentifié par la signature
/// `Stripe-Signature` (pas de token). Corps brut : la signature porte sur les
/// octets exacts reçus.
pub async fn webhook(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<impl IntoResponse, AppError> {
    let stripe = state
        .config
        .stripe
        .as_ref()
        .ok_or_else(|| AppError::Unavailable("la facturation n'est pas configurée".into()))?;

    let signature = headers
        .get("stripe-signature")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    verify_signature(&body, signature, &stripe.webhook_secret, now).map_err(|e| {
        tracing::warn!(error = %e, "webhook Stripe refusé");
        AppError::InvalidInput(e.to_string())
    })?;
    let event = parse_event(&body).map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let outcome = billing::apply_event(&state.db, event).await?;
    // Abonnement (re)devenu Pro : la quantité Stripe doit suivre les membres
    // (ex. après une bascule de périodicité programmée).
    if let billing::Outcome::Applied {
        org_id,
        plan: Plan::Pro,
    } = outcome
    {
        billing::spawn_seat_sync(state.db.clone(), state.billing.clone(), org_id);
    }
    // 200 dans tous les cas traités (y compris doublon / ignoré) : sinon Stripe réessaie.
    Ok(StatusCode::OK)
}
