# Lodestone — guide pour le front

Ce document s'adresse à l'équipe qui construit le front (Next.js App Router, Shadcn/ui,
Tailwind, TanStack Query). Il récapitule **ce que le backend fournit déjà**, **ce qu'il
reste à faire côté front**, et les **user stories** à livrer.

Pour une vue d'ensemble en schémas : [`architecture.md`](architecture.md).

---

## 1. Ce que le backend fournit

Le backend est complet et testé (256 tests). Tout ce dont le front a besoin existe :

| Domaine | Fonctionnalités |
|---|---|
| Comptes | inscription, connexion, rotation des jetons, déconnexion, profil, mot de passe oublié, suppression du compte, export des données (RGPD) |
| Organisations | création, liste, détail, renommage, suppression, consommation du plan |
| Membres | liste, invitation par e-mail, acceptation, retrait, changement de rôle |
| Documents | upload (pdf, txt, md, 20 Mo), liste, détail, suppression, statut d'ingestion en temps réel |
| Chat | conversations (création, historique, détail, suppression), questions en streaming avec sources citées |
| Facturation | plans et quotas, souscription Pro (mensuelle ou annuelle), changement de périodicité, souscription Sovereign |

### Conventions générales

- **Base** : toutes les routes sont sous `/api`, sauf `/ws` et `/health`.
- **Authentification** : en-tête `Authorization: Bearer <access_token>`.
- **CORS** : seules les origines de `CORS_ALLOWED_ORIGINS` sont acceptées (par défaut
  `http://localhost:3000`, le serveur Next.js de dev).
- **Erreurs** : toujours du JSON `{ "error": "message lisible", "code": 404 }`.
  Le message peut être affiché tel quel (il est en français).
- **Identifiants** : UUID. **Dates** : ISO 8601 (`2026-10-18T13:53:58Z`).

| Code | Signification | À faire côté front |
|---|---|---|
| 400 | entrée invalide | afficher `error` près du champ |
| 401 | jeton absent ou expiré | tenter un refresh, sinon renvoyer vers la connexion |
| 402 | quota du plan atteint | proposer le passage en Pro (voir § 4.6) |
| 403 | rôle insuffisant | ne devrait pas arriver si les boutons sont masqués (§ 4.4) |
| 404 | introuvable **ou** pas accès | page « introuvable » |
| 409 | conflit (déjà membre, dernier owner…) | afficher `error` |
| 413 | fichier trop gros (> 20 Mo) | message dédié |
| 422 | JSON mal formé (champ manquant, type faux) | bug front |
| 429 | trop de tentatives | afficher l'attente : en-tête `Retry-After` (secondes) |
| 502 / 503 | service tiers indisponible / non configuré | message générique, réessayer |

---

## 2. Référence de l'API

### Comptes

| Méthode | Route | Corps | Réponse |
|---|---|---|---|
| POST | `/api/auth/register` | `{ email, password }` (8 car. min.) | 201 `TokenResponse` |
| POST | `/api/auth/login` | `{ email, password }` | 200 `TokenResponse` |
| POST | `/api/auth/refresh` | `{ refresh_token }` | 200 `TokenResponse` (nouvelle paire) |
| POST | `/api/auth/logout` | `{ refresh_token }` | 204 |
| GET | `/api/auth/me` | — | 200 `{ id, email, created_at, organizations: MyOrg[] }` |
| POST | `/api/auth/password/forgot` | `{ email }` | 202 (toujours) |
| POST | `/api/auth/password/reset` | `{ token, password }` | 204 |
| DELETE | `/api/auth/me` | `{ password }` | 204 |
| GET | `/api/auth/me/export` | — | 200, fichier JSON à télécharger |

`TokenResponse` : `{ access_token, refresh_token, token_type: "Bearer" }`.
L'access token dure **15 minutes**, le refresh token **7 jours** (et change à chaque refresh).

### Organisations

| Méthode | Route | Rôle | Corps | Réponse |
|---|---|---|---|---|
| POST | `/api/orgs` | connecté | `{ name }` | 201 `{ id, name, plan, created_at }` |
| GET | `/api/orgs` | connecté | — | `MyOrg[]` |
| GET | `/api/orgs/{id}` | membre | — | `MyOrg` |
| PATCH | `/api/orgs/{id}` | admin | `{ name }` | `MyOrg` |
| DELETE | `/api/orgs/{id}` | owner | — | 204 |
| GET | `/api/orgs/{id}/usage` | membre | — | `Usage` |

`MyOrg` : `{ id, name, plan: "free"|"pro"|"sovereign", role: "owner"|"admin"|"member", created_at }`.

`Usage` : `{ plan, limits, period: "2026-10", members: Quota, documents: Quota, questions: Quota }`
avec `Quota = { used, limit }` (`limit: null` = illimité).

### Membres et invitations

| Méthode | Route | Rôle | Corps | Réponse |
|---|---|---|---|---|
| GET | `/api/orgs/{id}/members` | membre | — | `[{ user_id, email, role, joined_at }]` |
| POST | `/api/orgs/{id}/invitations` | admin | `{ email, role? }` | 201 `{ id, email, role, expires_at, token, email_sent }` |
| POST | `/api/invitations/accept` | connecté | `{ token }` | 200 `MyOrg` |
| PATCH | `/api/orgs/{id}/members/{user_id}` | owner | `{ role }` | `{ user_id, email, role, joined_at }` |
| DELETE | `/api/orgs/{id}/members/{user_id}` | admin | — | 204 |

- `role` à l'invitation : `member` par défaut ; seul un owner peut inviter en `admin`/`owner`.
- `email_sent: false` (SMTP non configuré) : afficher le lien
  `/invitations/accept?token=…` pour que l'admin le transmette lui-même.
- L'invitation n'est acceptable **que connecté avec l'adresse invitée** (sinon 403).

### Documents

| Méthode | Route | Rôle | Corps | Réponse |
|---|---|---|---|---|
| POST | `/api/orgs/{id}/documents` | admin | multipart : `file`, `title?` | 202 `Document` (status `pending`) |
| GET | `/api/orgs/{id}/documents` | membre | — | `Document[]` (plus récent d'abord) |
| GET | `/api/documents/{id}` | membre | — | `Document` |
| DELETE | `/api/documents/{id}` | admin | — | 204 |

`Document` : `{ id, title, source_type: "pdf"|"txt"|"md", status: "pending"|"processing"|"ready"|"failed", created_at }`.

### Conversations

| Méthode | Route | Rôle | Corps | Réponse |
|---|---|---|---|---|
| POST | `/api/orgs/{id}/conversations` | membre | `{ title? }` | 201 `{ id, title, created_at }` |
| GET | `/api/orgs/{id}/conversations` | membre | — | ses conversations `[{ id, title, created_at, updated_at }]` |
| GET | `/api/conversations/{id}` | auteur | — | `{ id, title, created_at, messages: Message[] }` |
| DELETE | `/api/conversations/{id}` | auteur | — | 204 |

`Message` : `{ id, role: "user"|"assistant", content, created_at, sources: [{ document_id, title, score }] }`.
Sans titre fourni, la première question devient le titre.

### Facturation

| Méthode | Route | Rôle | Corps | Réponse |
|---|---|---|---|---|
| GET | `/api/orgs/{id}/billing` | admin | — | `Billing` |
| POST | `/api/orgs/{id}/billing/checkout` | owner | `{ interval?: "month"\|"year" }` | `{ url }` |
| POST | `/api/orgs/{id}/billing/interval` | owner | `{ interval }` | `{ interval, effective: "now"\|"at_period_end", effective_at, policy }` |
| POST | `/api/orgs/{id}/billing/sovereign/checkout` | owner | — | `{ url, setup_fee_included }` |

`Billing` contient notamment : `plan`, `billing_enabled`, `annual_available`, `subscription`
(statut, sièges, périodicité, fin de période, bascule programmée), `current_quote`,
`quotes.month` / `quotes.year` (montants en **centimes HT**), `switch_policy` (textes à
afficher tels quels) et, pour une org Sovereign, `sovereign`.

---

## 3. WebSocket `/ws`

Connexion : `wss://<api>/ws?token=<access_token>` (un navigateur ne peut pas envoyer
d'en-tête `Authorization` en WebSocket). Jeton invalide → la connexion est refusée (401).

Messages JSON, typés par le champ `type`.

**Front → serveur**

```jsonc
{ "type": "ask", "conversation_id": "uuid", "question": "Quelle est la procédure de congés ?" }
{ "type": "ping" }
```

**Serveur → front**

```jsonc
{ "type": "sources", "citations": [ { "document_id": "uuid", "title": "RH.pdf", "score": 0.87 } ] }
{ "type": "token", "delta": "La procédure" }          // répété, à concaténer
{ "type": "done", "message_id": "uuid" }              // réponse enregistrée
{ "type": "error", "message": "…" }                   // fin de l'échange, sans done
{ "type": "doc_status", "document_id": "uuid", "status": "ready" }
{ "type": "pong" }
```

Règles à respecter :

- **`sources` arrive avant le premier `token`** : afficher les sources tout de suite.
- Dans la réponse, `[1]`, `[2]`… renvoient aux citations **dans l'ordre** de `sources`.
- **Une seule question à la fois** par connexion : désactiver l'envoi entre `ask` et
  `done`/`error` (sinon `error` « déjà en cours »).
- `doc_status` arrive pour **tous les documents des orgs de l'utilisateur**, sans
  abonnement explicite.
- Après `done`, la conversation est à jour côté serveur : invalider le cache de
  `GET /api/conversations/{id}` et de la liste des conversations (le titre a pu changer).
- À la reconnexion (jeton rafraîchi, réseau coupé) : rouvrir la socket avec le nouveau jeton.

---

## 4. Ce qu'il reste à faire côté front

### 4.1 Gestion des jetons

- Garder **l'access token en mémoire** (pas de `localStorage`, exposé au XSS).
- **Refresh token** : la spec prévoit un cookie `httpOnly`. **Le backend le renvoie
  aujourd'hui dans le corps JSON** ; deux options :
  1. un *route handler* Next.js (BFF) qui reçoit le refresh token et le pose en cookie
     `httpOnly` côté serveur Next ;
  2. ou demander l'évolution du backend (poser/lire le cookie lui-même) — **à arbitrer
     avant FE-02**.
- Rafraîchir **avant** l'expiration (≈ 14 min) ou sur un 401, puis rejouer la requête.
  Une seule rafraîchissement à la fois (le refresh token tourne : deux refresh en parallèle
  avec le même jeton, le second échoue).

### 4.2 Pages à créer

| Page | Route suggérée | API utilisée |
|---|---|---|
| Accueil, tarifs | `/`, `/tarifs` (SSR) | — (tarifs : `GET …/billing` une fois connecté) |
| Inscription, connexion | `/inscription`, `/connexion` | `register`, `login` |
| Mot de passe oublié | `/mot-de-passe-oublie` | `password/forgot` |
| Nouveau mot de passe | `/reset-password?token=` | `password/reset` |
| Accepter une invitation | `/invitations/accept?token=` | `invitations/accept` (connexion d'abord) |
| Tableau de bord d'org | `/orgs/[id]` | `orgs/{id}`, `usage` |
| Documents | `/orgs/[id]/documents` | `documents`, WS `doc_status` |
| Chat | `/orgs/[id]/chat/[conversationId]` | `conversations`, WS |
| Membres | `/orgs/[id]/membres` | `members`, `invitations` |
| Facturation | `/orgs/[id]/billing` | `billing`, `checkout`, `interval` |
| Mon compte | `/compte` | `me`, `me/export`, `DELETE me` |

Les liens envoyés par e-mail pointent vers `APP_BASE_URL` + `/reset-password?token=…`
et `/invitations/accept?token=…` : **ces deux routes doivent exister**. Le retour de Stripe
arrive sur `/orgs/{id}/billing?checkout=success` (ou `cancel`).

### 4.3 Temps réel

- Un **hook WebSocket maison** (pas de Socket.IO) : connexion, reconnexion avec backoff,
  `ping` périodique, file des messages.
- Le hook alimente le cache TanStack Query (`doc_status` → document à jour dans la liste).

### 4.4 Affichage selon le rôle

Le rôle de l'utilisateur dans chaque org vient de `GET /api/auth/me` ou `GET /api/orgs`.
**Masquer les actions non autorisées** (tableau dans [`architecture.md` § 6](architecture.md#6-rôles-et-autorisations)) :
upload et suppression de documents (admin+), invitations (admin+, `admin`/`owner` réservés
aux owners), changement de rôle et suppression d'org (owner), facturation (admin+ en lecture,
owner pour payer).

### 4.5 Chat

- Afficher les sources dès `sources`, puis le texte qui se construit token par token.
- Rendre les références `[n]` cliquables vers la source correspondante.
- Gérer `error` (message affiché, saisie réactivée) et la perte de connexion pendant une
  réponse (la question n'est **pas** enregistrée : proposer de la reposer).

### 4.6 Quotas et passage en Pro

- Sur un **402**, lire `quota.resource` (`members`, `documents`, `questions`, `owned_orgs`)
  et proposer « Passer en Pro » (`quota.upgrade_to`).
- Une question refusée pour quota arrive **par la WebSocket** en `error` (pas de 402).
- Afficher la consommation (`GET /api/orgs/{id}/usage`) dans le tableau de bord.
- Passage en Pro : `POST …/billing/checkout` → rediriger vers `url` (page Stripe).
  Au retour, le plan passe à `pro` **après le webhook** (quelques secondes) : recharger
  `GET …/billing` jusqu'au changement.
- Montants en **centimes HT** : `14900` → « 149,00 € HT ».

### 4.7 Points d'attention

- **Upload** : 20 Mo maximum (413), formats pdf, txt, md (400 sinon). Afficher le statut
  `pending` → `processing` → `ready` / `failed`.
- **Mot de passe oublié** : afficher toujours le même message (« si un compte existe, un
  e-mail a été envoyé »), même pour une adresse inconnue.
- **429** : afficher « réessayez dans N minutes » à partir de `Retry-After`.
- **Suppression du compte** : demander le mot de passe ; un 409 explique quoi faire
  (nommer un autre owner, résilier l'abonnement).

---

## 5. User stories

Priorités : **P0** indispensable à la démo, **P1** important, **P2** souhaitable.
Premier jalon « démo » conseillé : **FE-02 + FE-06**, puis FE-05 et FE-03.

### FE-01 — Pages marketing (SSR) `P2` (#39)
> En tant que **Visiteur**, je veux découvrir le produit afin de comprendre l'offre avant de m'inscrire.

- [ ] Pages statiques / SSR (App Router + Server Components) rendues pour le SEO
- [ ] Page tarifs : Free, Pro (149 € de socle, 29 €/siège, −15 % à l'année), Sovereign
- [ ] Mise en avant de la souveraineté (LLM local, données qui ne quittent pas l'infra)

### FE-02 — Parcours d'authentification `P1` (#40)
> En tant que **Visiteur**, je veux m'inscrire et me connecter via une UI afin d'accéder à mon espace.

- [ ] Formulaires inscription / connexion (Shadcn/ui) avec gestion d'erreurs (400, 401, 429)
- [ ] Access token en mémoire, refresh en cookie `httpOnly` (voir § 4.1, à arbitrer)
- [ ] Rafraîchissement automatique et déconnexion
- [ ] Mot de passe oublié + page de réinitialisation (liens des e-mails)
- [ ] Page « Mon compte » : export des données, suppression du compte
- [ ] Tests composant du formulaire d'auth (`vitest` + Testing Library)

### FE-03 — Tableau de bord d'organisation `P2` (#41)
> En tant que **Membre**, je veux un tableau de bord afin de naviguer dans mon org (documents, chat, membres).

- [ ] Sélecteur d'org (`GET /api/orgs`) + création d'une org
- [ ] Navigation principale : documents, chat, membres, facturation (selon le rôle)
- [ ] Consommation du plan (`usage`) et proposition de passage en Pro sur 402
- [ ] Page facturation : formules mensuelle / annuelle, souscription, changement de périodicité

### FE-04 — Gestion des employés `P2` (#42)
> En tant qu'**Admin**, je veux gérer les membres depuis l'UI afin d'inviter, retirer et changer les rôles.

- [ ] Liste des membres + actions conditionnées au rôle (boutons masqués si non autorisé)
- [ ] Invitation par e-mail ; si `email_sent: false`, lien à copier
- [ ] Page d'acceptation d'invitation (`/invitations/accept?token=`)
- [ ] Messages clairs pour 409 (déjà membre, dernier owner) et 402 (3 sièges en Free)

### FE-05 — Upload de documents `P1` (#43)
> En tant qu'**Admin**, je veux téléverser des documents via l'UI afin de nourrir la base.

- [ ] Upload (glisser-déposer, pdf / txt / md, 20 Mo max) avec titre optionnel
- [ ] Affichage du statut en temps réel (WS `doc_status`)
- [ ] Liste et suppression des documents

### FE-06 — Interface de chat en streaming `P0` (#44)
> En tant que **Membre**, je veux une interface de chat afin d'interroger la base et voir la réponse se construire.

- [ ] Hook WebSocket maison (pas de Socket.IO), reconnexion automatique
- [ ] Affichage des sources, puis streaming token par token
- [ ] Références `[n]` reliées aux sources
- [ ] Historique des conversations (liste, reprise, suppression)
- [ ] Tests composant de la vue chat

---

## 6. Pour démarrer en local

```bash
docker compose up -d          # Postgres + pgvector, Ollama, Mailpit
cargo run -p lodestone-backend # API sur http://localhost:8090 (migrations au démarrage)
```

- E-mails de dev (invitations, mot de passe oublié) : <http://localhost:8025> (Mailpit).
- Modèles Ollama : `ollama pull nomic-embed-text` et le modèle de chat (`CHAT_MODEL`).
- Variables : voir `.env.example` (CORS déjà ouvert à `http://localhost:3000`).
