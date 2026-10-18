use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use validator::Validate;

use crate::{
    dto::auth::{
        ForgotPasswordRequest, LoginRequest, MeResponse, RefreshRequest, RegisterRequest,
        ResetPasswordRequest, UserResponse,
    },
    error::AppError,
    extractors::{auth_user::AuthUser, client_ip::ClientIp},
    repositories::users,
    services,
    state::AppState,
};

pub async fn register(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<RegisterRequest>,
) -> Result<impl IntoResponse, AppError> {
    if let Some(ip) = ip {
        state
            .limits
            .register_per_ip
            .hit(&ip.to_string())
            .map_err(AppError::TooManyRequests)?;
    }
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    let tokens = services::auth::register(
        &state.db,
        &state.config.jwt_secret,
        body.email,
        body.password,
    )
    .await?;

    Ok((StatusCode::CREATED, Json(tokens)))
}

/// POST /api/auth/login. Limité : 5 échecs par 15 min pour un même compte
/// depuis une même IP (un attaquant ne bloque pas la victime ailleurs), et un
/// plafond global par IP. Au-delà : 429, même avec le bon mot de passe.
pub async fn login(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<LoginRequest>,
) -> Result<impl IntoResponse, AppError> {
    let email = body.email.trim().to_lowercase();
    let account_key = match ip {
        Some(ip) => format!("{email}|{ip}"),
        None => email,
    };
    if let Some(ip) = ip {
        state
            .limits
            .login_per_ip
            .hit(&ip.to_string())
            .map_err(AppError::TooManyRequests)?;
    }
    state
        .limits
        .login_failures
        .check(&account_key)
        .map_err(AppError::TooManyRequests)?;

    match services::auth::login(
        &state.db,
        &state.config.jwt_secret,
        body.email,
        body.password,
    )
    .await
    {
        Ok(tokens) => {
            state.limits.login_failures.reset(&account_key);
            Ok(Json(tokens))
        }
        Err(AppError::Unauthorized) => {
            state.limits.login_failures.record(&account_key);
            Err(AppError::Unauthorized)
        }
        Err(e) => Err(e),
    }
}

/// POST /api/auth/password/forgot — 202 dans tous les cas : la réponse ne
/// révèle pas si un compte existe. Au-delà des limites (par e-mail, par IP),
/// la demande est ignorée silencieusement.
pub async fn forgot_password(
    State(state): State<AppState>,
    ClientIp(ip): ClientIp,
    Json(body): Json<ForgotPasswordRequest>,
) -> Result<impl IntoResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    let email = body.email.trim().to_lowercase();

    let within_limits = ip.is_none_or(|ip| {
        state
            .limits
            .password_reset_per_ip
            .hit(&ip.to_string())
            .is_ok()
    }) && state.limits.password_reset_per_email.hit(&email).is_ok();

    if within_limits {
        services::password_reset::request_reset(
            &state.db,
            state.mailer.clone(),
            &state.config.app_base_url,
            &email,
        )
        .await?;
    } else {
        tracing::warn!("mot de passe oublié : limite atteinte, demande ignorée");
    }
    Ok(StatusCode::ACCEPTED)
}

/// POST /api/auth/password/reset — nouveau mot de passe avec le jeton reçu par
/// e-mail ; toutes les sessions sont ensuite révoquées.
pub async fn reset_password(
    State(state): State<AppState>,
    Json(body): Json<ResetPasswordRequest>,
) -> Result<impl IntoResponse, AppError> {
    body.validate()
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;
    services::password_reset::reset(&state.db, &body.token, &body.password).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// GET /api/auth/me — profil + organisations de l'utilisateur (avec son rôle).
pub async fn me(
    State(state): State<AppState>,
    user: AuthUser, // exige un token valide
) -> Result<impl IntoResponse, AppError> {
    // L'extracteur nous a donné l'id ; on recharge l'utilisateur frais depuis la base.
    let profile = users::find_by_id(&state.db, user.user_id)
        .await?
        .ok_or(AppError::Unauthorized)?; // token valide mais user supprimé -> 401
    let organizations = services::organizations::list_for_user(&state.db, user.user_id).await?;

    Ok(Json(MeResponse {
        user: UserResponse::from(profile),
        organizations,
    }))
}

pub async fn refresh(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    let tokens =
        services::auth::refresh(&state.db, &state.config.jwt_secret, body.refresh_token).await?;
    Ok(Json(tokens))
}

pub async fn logout(
    State(state): State<AppState>,
    Json(body): Json<RefreshRequest>,
) -> Result<impl IntoResponse, AppError> {
    services::auth::logout(&state.db, body.refresh_token).await?;
    Ok(StatusCode::NO_CONTENT)
}
