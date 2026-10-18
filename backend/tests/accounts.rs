//! Lot « comptes » : mot de passe oublié, e-mails, limitation des tentatives.

mod common;

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use http_body_util::BodyExt;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::{Value, json};

use common::{TestApp, TestUser};

const PASSWORD: &str = "password123";

/// Le jeton contenu dans le dernier e-mail envoyé à `to` (lien `…?token=…`).
fn token_in_last_email(app: &TestApp, to: &str) -> String {
    let email = app
        .mailer
        .sent()
        .into_iter()
        .rev()
        .find(|e| e.to == to)
        .expect("e-mail envoyé");
    email
        .body
        .split("token=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("lien avec jeton")
        .to_string()
}

/// Laisse partir les e-mails envoyés en tâche de fond.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
}

async fn forgot(app: &TestApp, email: &str) -> StatusCode {
    app.request(
        Method::POST,
        "/api/auth/password/forgot",
        None,
        Some(json!({ "email": email })),
    )
    .await
    .0
}

async fn reset(app: &TestApp, token: &str, password: &str) -> (StatusCode, Value) {
    app.request(
        Method::POST,
        "/api/auth/password/reset",
        None,
        Some(json!({ "token": token, "password": password })),
    )
    .await
}

async fn login(app: &TestApp, email: &str, password: &str) -> (StatusCode, Value) {
    app.request(
        Method::POST,
        "/api/auth/login",
        None,
        Some(json!({ "email": email, "password": password })),
    )
    .await
}

/// Requête depuis une IP donnée (vue par le reverse proxy).
async fn post_from(
    app: &TestApp,
    ip: &str,
    uri: &str,
    body: Value,
) -> (StatusCode, Option<String>) {
    let res = app
        .send(
            Request::builder()
                .method(Method::POST)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-forwarded-for", ip)
                .body(Body::from(body.to_string()))
                .expect("requête"),
        )
        .await;
    let status = res.status();
    let retry = res
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let _ = res.into_body().collect().await;
    (status, retry)
}

// ----- Mot de passe oublié ------------------------------------------------------------

#[tokio::test]
async fn reinitialisation_complete() {
    let app = TestApp::spawn().await;
    let user = app.register("alice").await;
    let (_, tokens) = login(&app, &user.email, PASSWORD).await;
    let old_refresh = tokens["refresh_token"]
        .as_str()
        .expect("refresh")
        .to_string();

    // Adresse saisie avec une autre casse : retrouvée quand même.
    assert_eq!(
        forgot(&app, &user.email.to_uppercase()).await,
        StatusCode::ACCEPTED
    );
    settle().await;
    let sent = app.mailer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, user.email);
    assert!(
        sent[0]
            .body
            .contains("https://app.test/reset-password?token="),
        "{}",
        sent[0].body
    );

    let token = token_in_last_email(&app, &user.email);
    let (status, body) = reset(&app, &token, "nouveau-mot-de-passe").await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    assert_eq!(
        login(&app, &user.email, PASSWORD).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        login(&app, &user.email, "nouveau-mot-de-passe").await.0,
        StatusCode::OK
    );

    // Toutes les sessions d'avant sont révoquées.
    let (status, _) = app
        .request(
            Method::POST,
            "/api/auth/refresh",
            None,
            Some(json!({ "refresh_token": old_refresh })),
        )
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // Le lien ne sert qu'une fois.
    let (status, body) = reset(&app, &token, "encore-un-autre").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|m| m.contains("invalide ou expiré"))
    );
}

#[tokio::test]
async fn compte_inconnu_meme_reponse_sans_e_mail() {
    let app = TestApp::spawn().await;
    assert_eq!(forgot(&app, "personne@test.io").await, StatusCode::ACCEPTED);
    settle().await;
    assert!(app.mailer.sent().is_empty());

    let (status, _) = app
        .request(
            Method::POST,
            "/api/auth/password/forgot",
            None,
            Some(json!({ "email": "pas-un-email" })),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn seul_le_dernier_lien_fonctionne_et_il_expire() {
    let app = TestApp::spawn().await;
    let user = app.register("bob").await;

    forgot(&app, &user.email).await;
    settle().await;
    let first = token_in_last_email(&app, &user.email);
    forgot(&app, &user.email).await;
    settle().await;
    let second = token_in_last_email(&app, &user.email);
    assert_ne!(first, second);

    assert_eq!(
        reset(&app, &first, "nouveau-mdp-1").await.0,
        StatusCode::BAD_REQUEST,
        "ancien lien"
    );

    // Mot de passe trop court : refusé, le lien reste valable.
    assert_eq!(
        reset(&app, &second, "court").await.0,
        StatusCode::BAD_REQUEST
    );

    // Lien expiré.
    app.db
        .execute(Statement::from_string(
            app.db.get_database_backend(),
            "UPDATE password_resets SET expires_at = now() - interval '1 minute'",
        ))
        .await
        .expect("expiration");
    assert_eq!(
        reset(&app, &second, "nouveau-mdp-2").await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        login(&app, &user.email, PASSWORD).await.0,
        StatusCode::OK,
        "inchangé"
    );
}

#[tokio::test]
async fn demandes_limitees_par_e_mail_sans_le_reveler() {
    let app = TestApp::spawn().await;
    let user = app.register("carol").await;
    for _ in 0..5 {
        assert_eq!(forgot(&app, &user.email).await, StatusCode::ACCEPTED);
    }
    settle().await;
    assert_eq!(
        app.mailer.sent().len(),
        3,
        "3 par heure, puis ignorées en silence"
    );
}

#[tokio::test]
async fn seul_le_hash_du_jeton_est_stocke() {
    let app = TestApp::spawn().await;
    let user = app.register("dave").await;
    forgot(&app, &user.email).await;
    settle().await;
    let token = token_in_last_email(&app, &user.email);
    let n: i64 = app
        .db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "SELECT count(*) AS n FROM password_resets WHERE token_hash = $1",
            [token.into()],
        ))
        .await
        .expect("requête")
        .expect("ligne")
        .try_get("", "n")
        .expect("n");
    assert_eq!(n, 0);
}

// ----- Invitations par e-mail -----------------------------------------------------------

#[tokio::test]
async fn invitation_envoyee_par_e_mail() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;

    let (status, body) = app
        .post(
            &format!("/api/orgs/{org}/invitations"),
            &owner.token,
            json!({ "email": "Nouveau@Test.io" }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["email_sent"], true);
    settle().await;

    let sent = app.mailer.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, "nouveau@test.io");
    assert!(sent[0].subject.contains("Acme"), "{}", sent[0].subject);
    assert!(sent[0].body.contains(&owner.email), "nom de l'invitant");
    let token = body["token"].as_str().expect("token");
    assert!(sent[0].body.contains(&format!(
        "https://app.test/invitations/accept?token={token}"
    )));
}

// ----- Limitation des connexions ----------------------------------------------------------

#[tokio::test]
async fn cinq_echecs_puis_blocage() {
    let app = TestApp::spawn().await;
    let victim = app.register("victim").await;
    let other = app.register("other").await;

    for i in 0..5 {
        assert_eq!(
            login(&app, &victim.email, "mauvais").await.0,
            StatusCode::UNAUTHORIZED,
            "essai {i}"
        );
    }
    // Bloqué, même avec le bon mot de passe (sinon la force brute continue).
    let (status, body) = login(&app, &victim.email, PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");

    // Un autre compte n'est pas touché.
    assert_eq!(login(&app, &other.email, PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn une_connexion_reussie_remet_le_compteur_a_zero() {
    let app = TestApp::spawn().await;
    let user = app.register("erin").await;
    for _ in 0..4 {
        login(&app, &user.email, "mauvais").await;
    }
    assert_eq!(login(&app, &user.email, PASSWORD).await.0, StatusCode::OK);
    for i in 0..4 {
        assert_eq!(
            login(&app, &user.email, "mauvais").await.0,
            StatusCode::UNAUTHORIZED,
            "essai {i}"
        );
    }
    assert_eq!(login(&app, &user.email, PASSWORD).await.0, StatusCode::OK);
}

#[tokio::test]
async fn derriere_un_proxy_l_attaquant_ne_bloque_pas_la_victime() {
    let app = TestApp::spawn_with_config(|c| c.trust_proxy_headers = true).await;
    let victim = app.register("victim").await;
    let body = |password: &str| json!({ "email": victim.email, "password": password });

    // L'attaquant (IP A) épuise ses essais sur le compte de la victime.
    for _ in 0..5 {
        post_from(&app, "198.51.100.66", "/api/auth/login", body("mauvais")).await;
    }
    let (status, retry_after) =
        post_from(&app, "198.51.100.66", "/api/auth/login", body(PASSWORD)).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    let secs: u64 = retry_after.expect("Retry-After").parse().expect("secondes");
    assert!((1..=900).contains(&secs), "{secs}");

    // La victime, depuis son IP, se connecte normalement.
    let (status, _) = post_from(&app, "203.0.113.7", "/api/auth/login", body(PASSWORD)).await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn plafond_d_inscriptions_par_ip() {
    let app = TestApp::spawn_with_config(|c| c.trust_proxy_headers = true).await;
    for i in 0..20 {
        let (status, _) = post_from(
            &app,
            "198.51.100.10",
            "/api/auth/register",
            json!({ "email": format!("bot{i}@test.io"), "password": PASSWORD }),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "inscription {i}");
    }
    let (status, _) = post_from(
        &app,
        "198.51.100.10",
        "/api/auth/register",
        json!({ "email": "bot21@test.io", "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
    // Une autre IP peut toujours s'inscrire.
    let (status, _) = post_from(
        &app,
        "203.0.113.7",
        "/api/auth/register",
        json!({ "email": "humain@test.io", "password": PASSWORD }),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
}

/// Sans confiance dans le proxy, l'en-tête X-Forwarded-For est ignoré : un
/// attaquant ne peut pas contourner la limite en changeant d'IP « déclarée ».
#[tokio::test]
async fn x_forwarded_for_ignore_par_defaut() {
    let app = TestApp::spawn().await;
    let user: TestUser = app.register("frank").await;
    let body = json!({ "email": user.email, "password": "mauvais" });
    for i in 0..5 {
        post_from(
            &app,
            &format!("198.51.100.{i}"),
            "/api/auth/login",
            body.clone(),
        )
        .await;
    }
    let (status, _) = post_from(&app, "198.51.100.99", "/api/auth/login", body).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}
