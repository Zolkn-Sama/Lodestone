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

/// POST /api/orgs/{id}/billing/checkout — owner : URL de paiement Stripe.
pub async fn checkout(
    State(state): State<AppState>,
    member: OrgMember<Owner>,
) -> Result<impl IntoResponse, AppError> {
    let response = billing::start_checkout(
        &state.db,
        state.billing.as_deref(),
        &state.config.app_base_url,
        member.org_id,
        member.user_id,
    )
    .await?;
    Ok(Json(response))
}

/// GET /api/orgs/{id}/billing — admin : plan et abonnement.
pub async fn status(
    State(state): State<AppState>,
    member: OrgMember<Admin>,
) -> Result<impl IntoResponse, AppError> {
    let response = billing::status(&state.db, state.billing.is_some(), member.org_id).await?;
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

    billing::apply_event(&state.db, event).await?;
    // 200 dans tous les cas traités (y compris doublon / ignoré) : sinon Stripe réessaie.
    Ok(StatusCode::OK)
}
