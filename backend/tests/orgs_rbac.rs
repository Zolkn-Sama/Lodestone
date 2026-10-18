//! RBAC-01 : matrice owner / admin / member / non-membre sur toutes les routes
//! d'organisation (§7), isolation entre orgs et règles du dernier owner.

mod common;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use uuid::Uuid;

use common::{TestApp, TestUser};
use lodestone_backend::repositories::organizations;
use lodestone_core::plan::Plan;

#[derive(Clone, Copy, Debug)]
enum Actor {
    Owner,
    Admin,
    Member,
    Outsider,
}

const ACTORS: [Actor; 4] = [Actor::Owner, Actor::Admin, Actor::Member, Actor::Outsider];

struct Users {
    owner: TestUser,
    admin: TestUser,
    member: TestUser,
    outsider: TestUser,
}

impl Users {
    async fn register(app: &TestApp) -> Self {
        Self {
            owner: app.register("owner").await,
            admin: app.register("admin").await,
            member: app.register("member").await,
            outsider: app.register("outsider").await,
        }
    }

    fn get(&self, actor: Actor) -> &TestUser {
        match actor {
            Actor::Owner => &self.owner,
            Actor::Admin => &self.admin,
            Actor::Member => &self.member,
            Actor::Outsider => &self.outsider,
        }
    }
}

/// Une org neuve avec un owner, un admin et un member.
/// L'org passe en Pro : ces tests portent sur les rôles, et le même owner
/// crée une org par cas (le plan Free limite à une org par utilisateur).
async fn seed_org(app: &TestApp, users: &Users) -> Uuid {
    let org = app.create_org(&users.owner).await;
    organizations::set_plan(&app.db, org, Plan::Pro)
        .await
        .expect("plan Pro");
    app.add_member(org, &users.owner, &users.admin, "admin")
        .await;
    app.add_member(org, &users.owner, &users.member, "member")
        .await;
    org
}

struct Case {
    name: &'static str,
    method: Method,
    /// Construit l'URI à partir de l'org et des utilisateurs.
    uri: fn(Uuid, &Users) -> String,
    body: fn() -> Option<Value>,
    /// Statut attendu pour owner, admin, member, non-membre.
    expected: [StatusCode; 4],
}

const OK: StatusCode = StatusCode::OK;
const CREATED: StatusCode = StatusCode::CREATED;
const NO_CONTENT: StatusCode = StatusCode::NO_CONTENT;
const FORBIDDEN: StatusCode = StatusCode::FORBIDDEN;
const NOT_FOUND: StatusCode = StatusCode::NOT_FOUND;

fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "GET /api/orgs/{id}",
            method: Method::GET,
            uri: |org, _| format!("/api/orgs/{org}"),
            body: || None,
            expected: [OK, OK, OK, NOT_FOUND],
        },
        Case {
            name: "GET /api/orgs/{id}/members",
            method: Method::GET,
            uri: |org, _| format!("/api/orgs/{org}/members"),
            body: || None,
            expected: [OK, OK, OK, NOT_FOUND],
        },
        Case {
            name: "PATCH /api/orgs/{id} (renommer)",
            method: Method::PATCH,
            uri: |org, _| format!("/api/orgs/{org}"),
            body: || Some(json!({ "name": "Nouveau nom" })),
            expected: [OK, OK, FORBIDDEN, NOT_FOUND],
        },
        Case {
            name: "POST /api/orgs/{id}/invitations (member)",
            method: Method::POST,
            uri: |org, _| format!("/api/orgs/{org}/invitations"),
            body: || Some(json!({ "email": format!("new-{}@test.io", Uuid::new_v4()) })),
            expected: [CREATED, CREATED, FORBIDDEN, NOT_FOUND],
        },
        Case {
            name: "PATCH /api/orgs/{id}/members/{member} (changer un rôle)",
            method: Method::PATCH,
            uri: |org, u| format!("/api/orgs/{org}/members/{}", u.member.id),
            body: || Some(json!({ "role": "admin" })),
            expected: [OK, FORBIDDEN, FORBIDDEN, NOT_FOUND],
        },
        Case {
            name: "DELETE /api/orgs/{id}/members/{member} (retirer)",
            method: Method::DELETE,
            uri: |org, u| format!("/api/orgs/{org}/members/{}", u.member.id),
            body: || None,
            expected: [NO_CONTENT, NO_CONTENT, FORBIDDEN, NOT_FOUND],
        },
        Case {
            name: "DELETE /api/orgs/{id} (supprimer l'org)",
            method: Method::DELETE,
            uri: |org, _| format!("/api/orgs/{org}"),
            body: || None,
            expected: [NO_CONTENT, FORBIDDEN, FORBIDDEN, NOT_FOUND],
        },
    ]
}

#[tokio::test]
async fn matrice_des_roles() {
    let app = TestApp::spawn().await;
    let users = Users::register(&app).await;

    let mut failures = Vec::new();
    for case in cases() {
        for (actor, expected) in ACTORS.into_iter().zip(case.expected) {
            // Org neuve à chaque appel : les routes destructrices ne polluent
            // pas les cas suivants.
            let org = seed_org(&app, &users).await;
            let (status, body) = app
                .request(
                    case.method.clone(),
                    &(case.uri)(org, &users),
                    Some(&users.get(actor).token),
                    (case.body)(),
                )
                .await;
            if status != expected {
                failures.push(format!(
                    "{} en {actor:?} : attendu {expected}, obtenu {status} ({body})",
                    case.name
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[tokio::test]
async fn sans_token_valide_401() {
    let app = TestApp::spawn().await;
    let users = Users::register(&app).await;
    let org = seed_org(&app, &users).await;
    let member = users.member.id;

    let routes = [
        (Method::GET, "/api/orgs".to_string()),
        (Method::POST, "/api/orgs".to_string()),
        (Method::GET, format!("/api/orgs/{org}")),
        (Method::PATCH, format!("/api/orgs/{org}")),
        (Method::DELETE, format!("/api/orgs/{org}")),
        (Method::GET, format!("/api/orgs/{org}/members")),
        (Method::PATCH, format!("/api/orgs/{org}/members/{member}")),
        (Method::DELETE, format!("/api/orgs/{org}/members/{member}")),
        (Method::POST, format!("/api/orgs/{org}/invitations")),
        (Method::POST, "/api/invitations/accept".to_string()),
    ];

    for (method, uri) in routes {
        for token in [None, Some("pas.un.jwt")] {
            // Corps valide quand la route en attend un : seul le token doit faire échouer.
            let body =
                Some(json!({ "name": "x", "role": "admin", "email": "a@b.io", "token": "t" }));
            let (status, _) = app.request(method.clone(), &uri, token, body).await;
            assert_eq!(
                status,
                StatusCode::UNAUTHORIZED,
                "{method} {uri} avec token {token:?}"
            );
        }
    }
}

#[tokio::test]
async fn isolation_entre_organisations() {
    let app = TestApp::spawn().await;
    let alice = app.register("alice").await;
    let bob = app.register("bob").await;
    let org_a = app.create_org(&alice).await;
    let org_b = app.create_org(&bob).await;

    // Alice ne voit que son org.
    let (status, list) = app.get("/api/orgs", &alice.token).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<&str> = list
        .as_array()
        .expect("liste")
        .iter()
        .filter_map(|o| o["id"].as_str())
        .collect();
    assert_eq!(ids, vec![org_a.to_string()]);

    // Aucune route de l'org B n'est accessible à Alice, même pas son existence.
    for (method, uri, body) in [
        (Method::GET, format!("/api/orgs/{org_b}"), None),
        (Method::GET, format!("/api/orgs/{org_b}/members"), None),
        (
            Method::PATCH,
            format!("/api/orgs/{org_b}"),
            Some(json!({ "name": "piraté" })),
        ),
        (Method::DELETE, format!("/api/orgs/{org_b}"), None),
    ] {
        let (status, _) = app
            .request(method.clone(), &uri, Some(&alice.token), body)
            .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }

    // Owner de A, Alice ne peut pas toucher un membre de B en passant par
    // l'URL de sa propre org : la recherche est filtrée par org_id.
    let (status, _) = app
        .request(
            Method::DELETE,
            &format!("/api/orgs/{org_a}/members/{}", bob.id),
            Some(&alice.token),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (_, org) = app.get(&format!("/api/orgs/{org_b}"), &bob.token).await;
    assert_eq!(org["name"], "Acme", "l'org B est intacte");
    assert_eq!(org["role"], "owner", "Bob est toujours owner de B");
}

#[tokio::test]
async fn dernier_owner_protege() {
    let app = TestApp::spawn().await;
    let users = Users::register(&app).await;
    let org = seed_org(&app, &users).await;
    let owner = &users.owner;
    let owner_uri = format!("/api/orgs/{org}/members/{}", owner.id);

    // Ni rétrogradé, ni retiré tant qu'il est seul owner.
    let (status, _) = app
        .request(
            Method::PATCH,
            &owner_uri,
            Some(&owner.token),
            Some(json!({ "role": "admin" })),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = app
        .request(Method::DELETE, &owner_uri, Some(&owner.token), None)
        .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Un admin ne peut jamais retirer un owner.
    let (status, _) = app
        .request(Method::DELETE, &owner_uri, Some(&users.admin.token), None)
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    // Avec un second owner, le premier peut partir.
    let (status, _) = app
        .request(
            Method::PATCH,
            &format!("/api/orgs/{org}/members/{}", users.member.id),
            Some(&owner.token),
            Some(json!({ "role": "owner" })),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = app
        .request(Method::DELETE, &owner_uri, Some(&owner.token), None)
        .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let (status, _) = app.get(&format!("/api/orgs/{org}"), &owner.token).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "l'ex-owner n'a plus accès");
}

#[tokio::test]
async fn role_invalide_rejete() {
    let app = TestApp::spawn().await;
    let users = Users::register(&app).await;
    let org = seed_org(&app, &users).await;

    let (status, _) = app
        .request(
            Method::PATCH,
            &format!("/api/orgs/{org}/members/{}", users.member.id),
            Some(&users.owner.token),
            Some(json!({ "role": "superadmin" })),
        )
        .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
