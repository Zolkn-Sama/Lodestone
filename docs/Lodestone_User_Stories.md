# Lodestone — User Stories & Backlog

> Backlog produit calqué sur la roadmap (§11) et l'architecture en crates (§9) de la spec.
> **Convention GitHub suggérée** : 1 épic = 1 *milestone* · 1 story = 1 *issue* · critères d'acceptation = *checklist* dans l'issue.
> **Labels** proposés : `epic:auth`, `epic:orgs`, `epic:docs`, `epic:chat`, `epic:frontend`, `epic:billing`, `type:enabler`, `type:story`, `priority:P0/P1/P2`.

**Rôles du système** — `Visiteur` (non authentifié) · `Utilisateur` (identité globale) · `Member` / `Admin` / `Owner` (rôles par organisation) · `Développeur` (stories d'infrastructure).

**Format** : *En tant que [rôle], je veux [action] afin de [bénéfice].*

---

## ÉPIC 1 — Socle technique `epic:infra` `type:enabler`
> Roadmap §11.1 · Aucune valeur utilisateur directe, mais prérequis de tout le reste. Branche conseillée : `chore/socle`.

### INFRA-01 — Environnement de dev conteneurisé `P0`
En tant que développeur, je veux un `docker-compose` lançant Postgres+pgvector et Ollama afin de démarrer l'environnement complet d'une seule commande.
- [ ] `docker-compose up` démarre Postgres avec l'extension `vector` disponible
- [ ] Ollama accessible avec `nomic-embed-text` et un modèle de génération pré-tirés
- [ ] Variables d'env documentées dans `.env.example` (`DATABASE_URL`, `OLLAMA_URL`, `JWT_SECRET`)

### INFRA-02 — Squelette Axum + gestion d'erreurs `P0`
En tant que développeur, je veux un serveur Axum minimal avec `AppError` et `AppState` afin d'avoir la fondation web et les patterns imposés (§9) en place.
- [ ] `AppError` unique (`thiserror`) implémentant `IntoResponse`
- [ ] `AppState` partagé via `Arc`, injecté par l'extracteur `State`
- [ ] `lib.rs` expose `build_app(state) -> Router` (testable via `oneshot`)
- [ ] Endpoint `GET /health` répond `200`
- [ ] `tracing` configuré pour les logs structurés

### INFRA-03 — Workspace Cargo multi-crates `P0`
En tant que développeur, je veux le workspace multi-crates initialisé afin d'isoler la logique pure (crypto, RAG, LLM) du binaire serveur.
- [ ] `Cargo.toml` workspace avec les 7 membres (`entity`, `migration`, `core`, `auth`, `llm`, `rag`, `backend`)
- [ ] `[workspace.dependencies]` centralise les versions
- [ ] `rust-toolchain.toml` + `rustfmt.toml` pinnés

### INFRA-04 — Migrations SeaORM initiales `P0`
En tant que développeur, je veux les migrations créant le schéma complet afin que la base soit reproductible et versionnée.
- [ ] Migration crée `CREATE EXTENSION vector`, puis toutes les tables (§4)
- [ ] Index HNSW (`hnsw (embedding vector_cosine_ops)`) + index sur `org_id` créés
- [ ] Migrations rejouables (up/down) et lancées au démarrage du conteneur

### INFRA-05 — Pipeline CI GitHub Actions `P0`
En tant que développeur, je veux une CI qui bloque tout merge cassé afin de garder `main` toujours déployable.
- [ ] Workflow sur chaque PR : `fmt --check → clippy -D warnings → test`
- [ ] Base de test éphémère via `testcontainers` (Postgres+pgvector)
- [ ] Job front : `eslint → tsc --noEmit → vitest` (peut rester stub au début)
- [ ] Protection de branche `main` : PR obligatoire + CI verte requise

---

## ÉPIC 2 — Authentification `epic:auth`
> Roadmap §11.2 · Crates : `auth`, extracteur `auth_user`. Branche : `feat/auth`.

### AUTH-01 — Inscription `P0` `type:story`
En tant que Visiteur, je veux créer un compte avec e-mail et mot de passe afin d'accéder à la plateforme.
- [ ] `POST /api/auth/register` valide le DTO (`validator`)
- [ ] Mot de passe haché en `argon2id` (jamais stocké en clair)
- [ ] E-mail unique (contrainte `CITEXT UNIQUE`) → `409` si déjà pris
- [ ] Test unitaire du hachage/vérification (`crates/auth`)

### AUTH-02 — Connexion `P0`
En tant qu'Utilisateur, je veux me connecter afin de recevoir un access-token et un refresh-token.
- [ ] `POST /api/auth/login` renvoie access JWT (15 min) + refresh (7 j)
- [ ] Refresh posé en cookie `httpOnly`, access renvoyé au client
- [ ] Identifiants invalides → `401` sans fuite d'info
- [ ] Test unitaire génération/validation JWT

### AUTH-03 — Rotation du refresh-token `P1`
En tant qu'Utilisateur, je veux renouveler ma session afin de rester connecté sans me relogguer, en toute sécurité.
- [ ] `POST /api/auth/refresh` émet un nouveau couple et révoque l'ancien refresh (rotation)
- [ ] Refresh révoqué/expiré → `401`
- [ ] Le hash du token vit en base (`refresh_tokens`), jamais le token en clair

### AUTH-04 — Déconnexion `P1`
En tant qu'Utilisateur, je veux me déconnecter afin de révoquer ma session côté serveur.
- [ ] `POST /api/auth/logout` marque le refresh `revoked_at`
- [ ] Cookie de refresh effacé côté client

### AUTH-05 — Profil courant `P1`
En tant qu'Utilisateur, je veux consulter mon profil et mes organisations afin de savoir où je peux agir.
- [ ] `GET /api/auth/me` renvoie profil + liste des orgs avec le rôle dans chacune
- [ ] Requête sans access valide → `401`

### AUTH-06 — Extracteur `AuthUser` `P0` `type:enabler`
En tant que développeur, je veux un extracteur Axum qui décode l'access-JWT afin de centraliser l'authentification hors des handlers.
- [ ] `AuthUser` décode et valide le JWT, expose l'`user_id`
- [ ] Rejette en `401` si absent/invalide/expiré

---

## ÉPIC 3 — Organisations & RBAC `epic:orgs`
> Roadmap §11.3 · Crates/modules : `org_member`, `handlers/{orgs,members}`, `repositories/{organizations,memberships,invitations}`. Branche : `feat/orgs`.

### ORG-01 — Créer une organisation `P0`
En tant qu'Utilisateur, je veux créer une organisation afin de disposer d'un espace pour mon entreprise ; j'en deviens automatiquement `owner`.
- [ ] `POST /api/orgs` crée l'org + un `membership` `owner` pour le créateur
- [ ] `slug` unique généré/validé
- [ ] `plan` par défaut = `free`

### ORG-02 — Lister mes organisations `P1`
En tant que Member, je veux voir les organisations auxquelles j'appartiens afin de naviguer entre elles.
- [ ] `GET /api/orgs` ne renvoie que les orgs de l'utilisateur
- [ ] Isolation : aucune org d'un tiers ne fuite

### ORG-03 — Détail d'une organisation `P2`
En tant que Member, je veux consulter le détail d'une org afin d'en voir les infos.
- [ ] `GET /api/orgs/:id` accessible aux membres uniquement (`403` sinon)

### ORG-04 — Renommer une organisation `P2`
En tant qu'Admin, je veux renommer l'org afin de corriger/mettre à jour son nom.
- [ ] `PATCH /api/orgs/:id` réservé `admin`/`owner`
- [ ] `member` → `403`

### ORG-05 — Supprimer une organisation `P1`
En tant qu'Owner, je veux supprimer mon organisation afin de retirer toutes ses données.
- [ ] `DELETE /api/orgs/:id` réservé `owner` (`403` pour admin/member)
- [ ] Suppression en cascade (membres, docs, chunks, conversations…)

### MEMBER-01 — Lister les employés `P1`
En tant que Member, je veux voir la liste des membres de l'org afin de savoir qui en fait partie.
- [ ] `GET /api/orgs/:id/members` renvoie membres + rôles

### MEMBER-02 — Inviter un employé `P0`
En tant qu'Admin, je veux inviter un employé par e-mail afin d'agrandir l'équipe.
- [ ] `POST /api/orgs/:id/invitations` réservé `admin`/`owner`
- [ ] Génère un `token_hash` + `expires_at` ; token brut envoyé par e-mail
- [ ] Invitation expirée/réutilisée refusée

### MEMBER-03 — Accepter une invitation `P0`
En tant qu'Utilisateur, je veux accepter une invitation afin de rejoindre l'organisation.
- [ ] `POST /api/invitations/accept` valide le token → crée le `membership` avec le rôle prévu
- [ ] Marque `accepted_at` ; token à usage unique

### MEMBER-04 — Retirer un employé `P1`
En tant qu'Admin, je veux retirer un employé afin de révoquer son accès.
- [ ] `DELETE /api/orgs/:id/members/:user_id` réservé `admin`/`owner`
- [ ] L'utilisateur retiré perd immédiatement l'accès à l'org

### MEMBER-05 — Changer un rôle `P2`
En tant qu'Owner, je veux modifier le rôle d'un membre afin de déléguer l'administration.
- [ ] `PATCH /api/orgs/:id/members/:user_id` réservé `owner`
- [ ] Garde-fou : ne pas retirer le dernier `owner`

### RBAC-01 — Extracteur `OrgMember` + matrice de permissions `P0` `type:enabler`
En tant que développeur, je veux un extracteur qui charge l'appartenance et vérifie le rôle afin que l'autorisation soit centralisée (§7).
- [ ] `OrgMember { role }` charge le membership, rejette en `403` si rôle insuffisant
- [ ] Filtrage systématique par `org_id` (isolation multi-tenant)
- [ ] Tests d'intégration couvrant la matrice owner/admin/member (§7)

---

## ÉPIC 4 — Documents & ingestion `epic:docs`
> Roadmap §11.4 · Crates : `rag`, `llm`, `services/{documents,ingestion}`. Branche : `feat/documents`.

### DOC-01 — Uploader un document `P0`
En tant qu'Admin, je veux téléverser un document afin qu'il soit indexé et interrogeable.
- [ ] `POST /api/orgs/:id/documents` réservé `admin`/`owner`
- [ ] Crée le document en statut `pending` puis déclenche l'ingestion **asynchrone**
- [ ] Formats supportés : pdf / txt / md

### DOC-02 — Lister les documents `P1`
En tant que Member, je veux voir les documents de l'org afin de savoir ce qui est disponible.
- [ ] `GET /api/orgs/:id/documents` renvoie titre + statut

### DOC-03 — Consulter le statut d'un document `P1`
En tant que Member, je veux voir le statut d'un document afin de savoir s'il est prêt à être interrogé.
- [ ] `GET /api/documents/:id` renvoie `pending|processing|ready|failed`

### DOC-04 — Supprimer un document `P1`
En tant qu'Admin, je veux supprimer un document afin de retirer un contenu obsolète.
- [ ] `DELETE /api/documents/:id` réservé `admin`/`owner`
- [ ] Supprime en cascade les `document_chunks` associés

### ING-01 — Pipeline d'extraction + chunking `P0` `type:enabler`
En tant que développeur, je veux extraire et découper le texte afin de préparer les fragments à embedder.
- [ ] Extraction texte (PDF/txt/md) dans `crates/rag`
- [ ] Découpage ~500–800 tokens, chevauchement ~80, respect des frontières de paragraphes
- [ ] Tests unitaires du chunking (pur, sans réseau ni DB)

### ING-02 — Embeddings via Ollama `P0` `type:enabler`
En tant que développeur, je veux embedder chaque fragment via le client LLM afin de les stocker comme vecteurs.
- [ ] `trait LlmClient::embed` implémenté par `OpenAiCompatibleClient` (Ollama `nomic-embed-text`)
- [ ] `MockLlmClient` déterministe disponible pour les tests
- [ ] Insertion en `document_chunks` avec `embedding VECTOR(768)` + `org_id`

### ING-03 — Notification de statut par WebSocket `P1`
En tant que Member, je veux être notifié en temps réel de l'avancement de l'ingestion afin de savoir quand un doc devient interrogeable.
- [ ] Statut du document poussé via WS (`doc_status`) à chaque transition
- [ ] Statut `failed` remonté en cas d'erreur d'ingestion

---

## ÉPIC 5 — Chat RAG `epic:chat`
> Roadmap §11.5 · Modules : `ws/{protocol,handler}`, `services/chat`, `repositories/{conversations,messages,chunks}`. Branche : `feat/chat`.

### CHAT-01 — Créer une conversation `P1`
En tant que Member, je veux démarrer une conversation afin de poser mes questions.
- [ ] `POST /api/orgs/:id/conversations` crée une conversation liée à l'user + l'org

### CHAT-02 — Historique des conversations `P2`
En tant que Member, je veux retrouver mes conversations passées afin de reprendre un fil.
- [ ] `GET /api/orgs/:id/conversations` liste les conversations de l'utilisateur

### CHAT-03 — Détail d'une conversation `P2`
En tant que Member, je veux rouvrir une conversation afin de relire messages et sources citées.
- [ ] `GET /api/conversations/:id` renvoie messages + `message_sources`

### CHAT-04 — Supprimer une conversation `P2`
En tant que Member, je veux supprimer une conversation afin de nettoyer mon historique.
- [ ] `DELETE /api/conversations/:id` supprime la conversation en cascade

### WS-01 — Connexion WebSocket authentifiée `P0` `type:enabler`
En tant que développeur, je veux un endpoint WS authentifié afin de router le chat temps réel (§6).
- [ ] `GET /ws?token=<access_jwt>` : upgrade + auth via le token
- [ ] Messages typés par le champ `type` (`ClientMsg`/`ServerMsg`)
- [ ] Connexion rejetée si token invalide

### RAG-01 — Poser une question (retrieval isolé par org) `P0`
En tant que Member, je veux poser une question en langage naturel afin d'obtenir une réponse fondée sur les documents de mon org.
- [ ] Message WS `ask` → embedding de la question
- [ ] Recherche des k fragments les plus proches (cosinus pgvector) **filtrée par `org_id`**
- [ ] Aucun fragment d'une autre org ne peut remonter (test d'isolation)

### RAG-02 — Citations avant streaming `P0`
En tant que Member, je veux voir les sources citées dès le départ afin de faire confiance à la réponse.
- [ ] Message `sources` (document_id, title, score) envoyé **avant** le premier token
- [ ] Sources persistées en `message_sources` avec leur score

### RAG-03 — Réponse en streaming `P0`
En tant que Member, je veux voir la réponse s'afficher au fur et à mesure afin d'avoir un ressenti réactif.
- [ ] Appel LLM en streaming relayé token par token (`token` / `delta`)
- [ ] Message `done` avec `message_id` en fin de génération
- [ ] Erreur LLM → message `error` propre (pas de crash de la socket)

### RAG-04 — Persistance des échanges `P1`
En tant que Member, je veux que mes échanges soient sauvegardés afin de les retrouver plus tard.
- [ ] Message `user` + message `assistant` persistés à la fin de la génération
- [ ] `message_sources` liés au message assistant

---

## ÉPIC 6 — Frontend `epic:frontend`
> Roadmap §11.6 · Hors workspace Rust. Branche : `feat/frontend`. (Découpe possible en sous-issues par écran.)

### FE-01 — Pages marketing (SSR) `P2`
En tant que Visiteur, je veux découvrir le produit afin de comprendre l'offre avant de m'inscrire.
- [ ] Pages statiques/SSR (App Router + Server Components) rendues pour le SEO

### FE-02 — Parcours d'authentification `P1`
En tant que Visiteur, je veux m'inscrire et me connecter via une UI afin d'accéder à mon espace.
- [ ] Formulaires register/login (Shadcn/ui) avec gestion d'erreurs
- [ ] Access-token en mémoire, refresh en cookie `httpOnly`
- [ ] Tests composant du formulaire d'auth (`vitest` + Testing Library)

### FE-03 — Dashboard organisation `P2`
En tant que Member, je veux un tableau de bord afin de naviguer dans mon org (docs, chat, membres).
- [ ] Sélecteur d'org + navigation principale

### FE-04 — Gestion des employés `P2`
En tant qu'Admin, je veux gérer les membres depuis l'UI afin d'inviter/retirer/changer les rôles.
- [ ] Liste des membres + actions conditionnées au rôle (boutons masqués si non autorisé)

### FE-05 — Upload de documents `P1`
En tant qu'Admin, je veux téléverser des documents via l'UI afin de nourrir la base.
- [ ] Upload + affichage du statut en temps réel (WS `doc_status`)

### FE-06 — Interface de chat streaming `P0`
En tant que Member, je veux une interface de chat afin d'interroger la base et voir la réponse se construire.
- [ ] Hook WebSocket maison (pas de Socket.IO)
- [ ] Affichage des sources puis streaming token par token
- [ ] Tests composant de la vue chat

---

## ÉPIC 7 — Monétisation `epic:billing`
> Roadmap §11.7 · Nouveau `services/billing` + `repositories/subscriptions`. Branche : `feat/billing`.

### BILL-01 — Plans & quotas `P1`
En tant qu'Owner, je veux que mon plan détermine mes limites afin d'aligner l'usage sur mon abonnement.
- [ ] Plans `free` / `pro` / `sovereign` avec limites (users, docs, questions/mois pour Free)
- [ ] Quotas Free appliqués : 3 users, 50 docs, 100 questions/mois

### BILL-02 — Application des limites `P1`
En tant que plateforme, je veux bloquer les actions au-delà du quota afin de faire respecter le plan.
- [ ] Dépassement de quota → erreur explicite (upgrade suggéré)
- [ ] Compteurs (users, docs, questions) fiables et remis à zéro au bon rythme

### BILL-03 — Abonnement Stripe par siège `P2`
En tant qu'Owner, je veux payer un abonnement par siège afin de passer au plan Pro.
- [ ] Intégration Stripe (checkout + webhooks) → mise à jour du `plan` de l'org
- [ ] Changement de plan reflété sur les quotas en quasi temps réel

### BILL-04 — Déploiement Sovereign `P2` `type:enabler`
En tant que client secteur sensible, je veux un LLM auto-hébergé afin que mes documents ne quittent jamais mon infra.
- [ ] Bascule Ollama ↔ OpenAI par simple config (URL de base du `LlmClient`)
- [ ] Documentation de déploiement self-hosted

### BILL-05 — Socle minimum par organisation `P1`
En tant qu'Owner, je veux qu'un plancher mensuel s'applique à mon organisation afin que le coût d'infra soit couvert même avec peu de sièges.
- [ ] Facturation Pro = `max(socle, sièges × prix_siège)` — jamais sous le socle (149 €/mois HT à 1–4 sièges)
- [ ] Au-delà du seuil, la facturation par siège (29 €) prend le relais
- [ ] Montant du socle configurable côté plateforme (pas en dur)
- [ ] Plans Free et Sovereign non concernés par le socle Pro

### BILL-06 — Remise sur engagement annuel `P2`
En tant qu'Owner, je veux payer à l'année afin de bénéficier d'une remise sur l'abonnement.
- [ ] Choix mensuel / annuel proposé à la souscription et au changement de plan
- [ ] Remise (%) configurable appliquée au tarif annuel, reflétée dans Stripe (price dédié)
- [ ] Facture annuelle reprend le socle minimum ramené à l'année
- [ ] Bascule annuel → mensuel gérée à l'échéance (politique de prorata explicite)

### BILL-07 — Frais de mise en service Sovereign `P2`
En tant qu'Owner d'une org Sovereign, je veux une prestation d'installation facturée une seule fois afin de déployer sur mon infra.
- [ ] Frais unique de mise en service (6 000 € HT) facturé à l'activation Sovereign
- [ ] Distinct de la licence annuelle récurrente (18 000 €/an)
- [ ] Émis comme *line item* one-time Stripe (pas d'abonnement récurrent pour ce montant)
- [ ] Non re-facturé lors des renouvellements annuels

---

## Ordre de traitement conseillé

Respecte l'ordre des épics (dépendances techniques) :
**INFRA → AUTH → ORGS/RBAC → DOCS → CHAT → FRONTEND → BILLING.**

Priorise les `P0` de chaque épic pour obtenir une *tranche verticale* fonctionnelle au plus vite. Un premier jalon « démo » réaliste : INFRA (tout) + AUTH-01/02/06 + ORG-01 + MEMBER-02/03 + DOC-01 + ING-01/02 + RAG-01/02/03 + FE-02/06.