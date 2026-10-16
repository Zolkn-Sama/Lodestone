//! MEMBER-02 / MEMBER-03 : création et acceptation des invitations.

mod common;

use axum::http::StatusCode;
use sea_orm::{ConnectionTrait, Statement};
use serde_json::json;

use common::TestApp;

#[tokio::test]
async fn parcours_complet() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let bob = app.register("bob").await;
    let org = app.create_org(&owner).await;

    // Rôle absent -> member ; l'email est normalisé en minuscules.
    let (status, inv) = app
        .post(
            &format!("/api/orgs/{org}/invitations"),
            &owner.token,
            json!({ "email": bob.email.to_uppercase() }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(inv["role"], "member");
    assert_eq!(inv["email"], bob.email.to_lowercase());
    let token = inv["token"].as_str().expect("token").to_string();

    let (status, joined) = app
        .post(
            "/api/invitations/accept",
            &bob.token,
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(joined["id"], org.to_string());
    assert_eq!(joined["role"], "member");

    // Bob apparaît dans les membres.
    let (_, members) = app
        .get(&format!("/api/orgs/{org}/members"), &bob.token)
        .await;
    assert!(
        members
            .as_array()
            .expect("liste")
            .iter()
            .any(|m| m["user_id"] == bob.id.to_string() && m["role"] == "member")
    );

    // Un jeton ne sert qu'une fois.
    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &bob.token,
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn jeton_reserve_a_l_email_invite() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let bob = app.register("bob").await;
    let eve = app.register("eve").await;
    let org = app.create_org(&owner).await;

    let token = app.invite(org, &owner, &bob.email, "member").await;
    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &eve.token,
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Le jeton n'a pas été consommé : Bob peut toujours l'utiliser.
    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &bob.token,
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn reinviter_invalide_l_ancien_lien() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let bob = app.register("bob").await;
    let org = app.create_org(&owner).await;

    let old = app.invite(org, &owner, &bob.email, "member").await;
    let new = app.invite(org, &owner, &bob.email, "member").await;

    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &bob.token,
            json!({ "token": old }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &bob.token,
            json!({ "token": new }),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn invitation_expiree() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let bob = app.register("bob").await;
    let org = app.create_org(&owner).await;
    let token = app.invite(org, &owner, &bob.email, "member").await;

    app.db
        .execute(Statement::from_string(
            app.db.get_database_backend(),
            "UPDATE invitations SET expires_at = now() - interval '1 minute'",
        ))
        .await
        .expect("expiration forcée");

    let (status, _) = app
        .post(
            "/api/invitations/accept",
            &bob.token,
            json!({ "token": token }),
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn regles_de_creation() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let admin = app.register("admin").await;
    let org = app.create_org(&owner).await;
    app.add_member(org, &owner, &admin, "admin").await;
    let uri = format!("/api/orgs/{org}/invitations");

    // Un admin ne peut inviter que des members.
    for role in ["admin", "owner"] {
        let (status, _) = app
            .post(
                &uri,
                &admin.token,
                json!({ "email": "x@test.io", "role": role }),
            )
            .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "admin invite en {role}");
    }
    // Un owner peut inviter à n'importe quel rôle.
    app.invite(org, &owner, "futur-owner@test.io", "owner")
        .await;

    // Déjà membre -> 409, quelle que soit la casse de l'email.
    let (status, _) = app
        .post(
            &uri,
            &owner.token,
            json!({ "email": admin.email.to_uppercase() }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Email invalide -> 400.
    let (status, _) = app
        .post(&uri, &owner.token, json!({ "email": "pas-un-email" }))
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn seul_le_hash_du_jeton_est_stocke() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    let token = app.invite(org, &owner, "bob@test.io", "member").await;

    let row = app
        .db
        .query_one(Statement::from_sql_and_values(
            app.db.get_database_backend(),
            "SELECT count(*) AS n FROM invitations WHERE token_hash = $1",
            [token.into()],
        ))
        .await
        .expect("requête")
        .expect("ligne");
    let n: i64 = row.try_get("", "n").expect("count");
    assert_eq!(n, 0, "le jeton en clair ne doit pas être en base");
}

#[tokio::test]
async fn suppression_de_l_org_supprime_ses_invitations() {
    let app = TestApp::spawn().await;
    let owner = app.register("owner").await;
    let org = app.create_org(&owner).await;
    app.invite(org, &owner, "bob@test.io", "member").await;

    let (status, _) = app
        .request(
            axum::http::Method::DELETE,
            &format!("/api/orgs/{org}"),
            Some(&owner.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let row = app
        .db
        .query_one(Statement::from_string(
            app.db.get_database_backend(),
            "SELECT count(*) AS n FROM invitations",
        ))
        .await
        .expect("requête")
        .expect("ligne");
    let n: i64 = row.try_get("", "n").expect("count");
    assert_eq!(n, 0);
}
