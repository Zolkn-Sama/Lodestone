# Lodestone — architecture en schémas

Vue d'ensemble visuelle du backend. Les diagrammes sont en [Mermaid](https://mermaid.js.org/) :
GitHub et la plupart des éditeurs (RustRover avec le plugin Markdown, VS Code) les
affichent directement.

Sommaire :

1. [Vue d'ensemble](#1-vue-densemble)
2. [Crates du workspace](#2-crates-du-workspace)
3. [Couches du backend](#3-couches-du-backend)
4. [Modèle de données](#4-modèle-de-données)
5. [Authentification](#5-authentification)
6. [Rôles et autorisations](#6-rôles-et-autorisations)
7. [Documents et ingestion](#7-documents-et-ingestion)
8. [Chat RAG en streaming](#8-chat-rag-en-streaming)
9. [Plans, quotas et facturation](#9-plans-quotas-et-facturation)
10. [Comptes et RGPD](#10-comptes-et-rgpd)
11. [Déploiements](#11-déploiements)

---

## 1. Vue d'ensemble

```mermaid
flowchart LR
    user(["Utilisateur<br/>navigateur"])
    front["Front Next.js<br/>(dépôt séparé)"]

    subgraph backend["Serveur Lodestone (Rust, Axum)"]
        rest["API REST<br/>/api/..."]
        ws["WebSocket<br/>/ws"]
        bus["Bus d'événements<br/>en mémoire"]
    end

    db[("PostgreSQL<br/>+ pgvector")]
    llm["LLM compatible OpenAI<br/>Ollama (local) ou OpenAI"]
    smtp["Serveur SMTP"]
    stripe["Stripe"]

    user --> front
    front -- "HTTPS + JWT" --> rest
    front -- "WSS ?token=" --> ws
    rest --> db
    ws --> db
    rest -- "embeddings" --> llm
    ws -- "embeddings + génération" --> llm
    rest -- "invitations, mot de passe oublié" --> smtp
    rest -- "Checkout, sièges" --> stripe
    stripe -- "webhooks signés" --> rest
    rest -- "doc_status" --> bus
    bus --> ws
```

- **Une seule base** pour les données relationnelles ET les vecteurs (pgvector).
- **Le LLM est interchangeable par configuration** : Ollama en local (mode souverain) ou OpenAI.
- **Stripe et SMTP sont facultatifs** : sans eux, la facturation est désactivée et aucun e-mail ne part.

---

## 2. Crates du workspace

```mermaid
flowchart TD
    backend["backend<br/>serveur HTTP + WebSocket"]
    core["core<br/>Role, Plan, DocumentStatus, tarifs"]
    entity["entity<br/>entités SeaORM"]
    migration["migration<br/>schéma versionné"]
    auth["auth<br/>argon2id, JWT, jetons opaques"]
    rag["rag<br/>extraction, découpage, prompt<br/>(pur, sans réseau)"]
    llm["llm<br/>trait LlmClient<br/>Ollama / OpenAI / mock"]
    billing["billing<br/>trait PaymentProvider<br/>Stripe / mock, webhooks"]

    backend --> core
    backend --> entity
    backend --> migration
    backend --> auth
    backend --> rag
    backend --> llm
    backend --> billing
    migration -.-> entity
```

Les crates `rag`, `llm` et `billing` ne connaissent ni Axum ni la base : elles se testent
seules, et `llm` / `billing` exposent un **mock** utilisé par les tests d'intégration.

---

## 3. Couches du backend

```mermaid
flowchart LR
    req["Requête HTTP"] --> ext["Extracteurs<br/>AuthUser, OrgMember, DocumentAccess,<br/>ConversationAccess, ClientIp"]
    ext --> h["Handlers<br/>(validation, codes HTTP)"]
    h --> s["Services<br/>(règles métier, transactions)"]
    s --> r["Repositories<br/>(SQL via SeaORM)"]
    r --> pg[("PostgreSQL")]
    s --> ext2["Crates externes<br/>llm, rag, billing, mail"]
```

- **L'autorisation vit dans les extracteurs** : un handler qui reçoit un `OrgMember<Admin>`
  sait déjà que l'appelant est admin de l'org de l'URL.
- **Une erreur unique** (`AppError`) donne une réponse JSON `{ "error", "code" }`.

---

## 4. Modèle de données

```mermaid
erDiagram
    users ||--o{ memberships : "appartient à"
    organizations ||--o{ memberships : "a pour membres"
    organizations ||--o{ invitations : "invite"
    users |o--o{ invitations : "a invité"
    users ||--o{ refresh_tokens : "sessions"
    users ||--o{ password_resets : "liens de réinitialisation"
    organizations ||--o{ documents : "possède"
    users |o--o{ documents : "a apporté"
    documents ||--o{ document_chunks : "découpé en"
    organizations ||--o{ document_chunks : "isolation"
    organizations ||--o{ conversations : "contient"
    users ||--o{ conversations : "auteur"
    conversations ||--o{ messages : "contient"
    messages ||--o{ message_sources : "cite"
    document_chunks ||--o{ message_sources : "cité dans"
    organizations ||--o| subscriptions : "abonnement"
    organizations ||--o{ org_usage : "compteur mensuel"

    users {
        uuid id PK
        text email UK
        text password_hash
    }
    organizations {
        uuid id PK
        text name
        text plan "free | pro | sovereign"
        timestamptz sovereign_setup_paid_at
    }
    memberships {
        uuid user_id FK
        uuid org_id FK
        text role "owner | admin | member"
    }
    invitations {
        uuid org_id FK
        text email
        text role
        text token_hash UK
        timestamptz expires_at
        timestamptz accepted_at
    }
    documents {
        uuid id PK
        uuid org_id FK
        uuid uploaded_by FK "NULL si compte supprimé"
        text title
        text status "pending | processing | ready | failed"
        text embedding_model
    }
    document_chunks {
        uuid id PK
        uuid document_id FK
        uuid org_id FK
        int chunk_index
        text content
        vector embedding "768 dimensions, index HNSW"
    }
    conversations {
        uuid id PK
        uuid org_id FK
        uuid user_id FK
        text title
    }
    messages {
        uuid id PK
        uuid conversation_id FK
        text role "user | assistant"
        text content
    }
    message_sources {
        uuid message_id FK
        uuid chunk_id FK
        real score
    }
    subscriptions {
        uuid org_id PK
        text stripe_subscription_id UK
        text product "pro | sovereign"
        text status
        int seats
        text billing_interval "month | year"
        text scheduled_interval
    }
    org_usage {
        uuid org_id PK
        date period PK
        int questions
    }
```

Règles de suppression importantes :

- supprimer une **org** supprime en cascade ses documents, fragments, conversations,
  invitations, abonnement et compteurs ;
- supprimer un **compte** supprime ses sessions, appartenances et conversations, mais
  **ses documents et invitations restent à l'org** (auteur mis à `NULL`).

---

## 5. Authentification

```mermaid
sequenceDiagram
    autonumber
    actor U as Utilisateur
    participant F as Front
    participant API as API
    participant DB as Base

    U->>F: email + mot de passe
    F->>API: POST /api/auth/login
    API->>API: limite (5 échecs / 15 min par compte et IP)
    API->>DB: utilisateur + vérification argon2id
    API->>DB: enregistre le hash du refresh token (7 jours)
    API-->>F: access_token (JWT 15 min) + refresh_token
    F->>API: GET /api/... (Authorization: Bearer access_token)
    Note over F,API: 15 minutes plus tard, l'access token expire
    F->>API: POST /api/auth/refresh (refresh_token)
    API->>DB: révoque l'ancien, enregistre le nouveau (rotation)
    API-->>F: nouvelle paire de jetons
    F->>API: POST /api/auth/logout (refresh_token)
    API->>DB: révoque le refresh token
```

---

## 6. Rôles et autorisations

| Action | owner | admin | member | non-membre |
|---|:---:|:---:|:---:|:---:|
| Voir l'org, ses membres, ses documents, sa consommation | ✅ | ✅ | ✅ | 404 |
| Poser des questions, gérer **ses** conversations | ✅ | ✅ | ✅ | 404 |
| Renommer l'org | ✅ | ✅ | 403 | 404 |
| Uploader / supprimer des documents | ✅ | ✅ | 403 | 404 |
| Inviter (rôle `member`) / retirer un membre | ✅ | ✅ | 403 | 404 |
| Inviter en `admin`/`owner`, changer un rôle | ✅ | 403 | 403 | 404 |
| Voir la facturation | ✅ | ✅ | 403 | 404 |
| Souscrire, changer de périodicité | ✅ | 403 | 403 | 404 |
| Supprimer l'org | ✅ | 403 | 403 | 404 |

Un non-membre reçoit **404** (et non 403) : on ne révèle pas l'existence de l'org.
Une conversation est **privée à son auteur** : même l'owner reçoit 404.

```mermaid
flowchart TD
    a["Requête /api/orgs/{id}/..."] --> b{"Jeton valide ?"}
    b -- non --> e401["401"]
    b -- oui --> c{"Membre de l'org ?"}
    c -- non --> e404["404"]
    c -- oui --> d{"Rôle suffisant ?"}
    d -- non --> e403["403"]
    d -- oui --> ok["Handler"]
```

---

## 7. Documents et ingestion

```mermaid
sequenceDiagram
    autonumber
    actor A as Admin
    participant F as Front
    participant API as API
    participant T as Tâche d'ingestion
    participant LLM as LLM (embeddings)
    participant DB as Base
    participant WS as WebSocket des membres

    A->>F: choisit un fichier (pdf, txt, md, 20 Mo max)
    F->>API: POST /api/orgs/{id}/documents (multipart)
    API->>DB: quota documents (sous verrou) + document "pending"
    API-->>F: 202 Accepted (document pending)
    API->>WS: doc_status pending
    API->>T: lance l'ingestion en tâche de fond
    T->>DB: status = processing
    T->>WS: doc_status processing
    T->>T: extraction du texte + découpage (~800 tokens, chevauchement 80)
    T->>LLM: embeddings par lots de 32
    T->>DB: fragments + vecteurs (une transaction)
    T->>DB: status = ready
    T->>WS: doc_status ready
```

```mermaid
stateDiagram-v2
    [*] --> pending: upload
    pending --> processing: tâche lancée
    processing --> ready: fragments enregistrés
    processing --> failed: fichier illisible, LLM en panne
    pending --> failed: redémarrage du serveur
    processing --> failed: redémarrage du serveur
    ready --> [*]: suppression
    failed --> [*]: suppression
```

---

## 8. Chat RAG en streaming

```mermaid
sequenceDiagram
    autonumber
    actor M as Membre
    participant F as Front
    participant WS as /ws
    participant DB as Base (pgvector)
    participant LLM as LLM

    F->>WS: connexion ?token=access_token
    M->>F: pose une question
    F->>WS: { type: "ask", conversation_id, question }
    WS->>DB: vérifie la conversation (auteur, encore membre) + quota de questions
    WS->>LLM: embedding de la question
    WS->>DB: 5 fragments les plus proches, filtrés par org
    WS-->>F: { type: "sources", citations }
    WS->>LLM: prompt (consignes + extraits [1], [2]... + question)
    loop morceau par morceau
        LLM-->>WS: delta
        WS-->>F: { type: "token", delta }
    end
    WS->>DB: question + réponse + sources + compteur (une transaction)
    WS-->>F: { type: "done", message_id }
```

- Sans document pertinent, la réponse est fixe et **le modèle n'est pas appelé** (pas d'invention).
- Une erreur (modèle indisponible, quota atteint…) donne `{ type: "error", message }` et
  **rien n'est enregistré** ; la connexion reste ouverte.
- Une seule question à la fois par connexion ; la génération s'arrête si le client part.

---

## 9. Plans, quotas et facturation

| | Free | Pro | Sovereign |
|---|---|---|---|
| Membres | 3 (invitations en attente comprises) | illimité (facturé au siège) | illimité |
| Documents | 50 | illimité | illimité |
| Questions / mois | 100 | illimité | illimité |
| Orgs possédées | 1 | — | — |
| Prix HT | 0 € | max(149 €, sièges × 29 €) par mois, −15 % à l'année | 18 000 €/an + 6 000 € de mise en service |

Dépassement d'un quota : **402** avec `{ "quota": { "resource", "limit", "plan", "upgrade_to": "pro" } }`.

```mermaid
sequenceDiagram
    autonumber
    actor O as Owner
    participant F as Front
    participant API as API
    participant S as Stripe
    participant DB as Base

    O->>F: « Passer en Pro » (mensuel ou annuel)
    F->>API: POST /api/orgs/{id}/billing/checkout { interval }
    API->>S: session Checkout (quantité = nombre de membres)
    API-->>F: { url }
    F->>S: redirection vers la page de paiement
    O->>S: paie
    S->>API: webhook checkout.session.completed (signé)
    API->>DB: abonnement actif, plan = pro (quotas levés aussitôt)
    S->>API: webhook customer.subscription.updated
    API->>DB: sièges, périodicité, fin de période
    Note over API,S: Arrivée / départ d'un membre : quantité Stripe mise à jour (prorata)
```

```mermaid
stateDiagram-v2
    [*] --> free: création de l'org
    free --> pro: paiement Stripe (webhook)
    pro --> pro: past_due (relances, Pro conservé)
    pro --> free: unpaid / canceled
    free --> sovereign: contrat (équipe Lodestone)
    sovereign --> sovereign: licence annuelle (Stripe ou hors ligne)
```

---

## 10. Comptes et RGPD

```mermaid
sequenceDiagram
    autonumber
    actor U as Utilisateur
    participant F as Front
    participant API as API
    participant M as SMTP
    participant DB as Base

    U->>F: « Mot de passe oublié »
    F->>API: POST /api/auth/password/forgot { email }
    API-->>F: 202 (toujours, compte existant ou non)
    API->>DB: jeton haché, valable 1 h (les précédents invalidés)
    API->>M: e-mail avec le lien /reset-password?token=...
    U->>F: ouvre le lien, saisit un nouveau mot de passe
    F->>API: POST /api/auth/password/reset { token, password }
    API->>DB: nouveau mot de passe, jeton consommé, TOUTES les sessions révoquées
    API-->>F: 204
```

```mermaid
flowchart TD
    a["DELETE /api/auth/me { password }"] --> b{"Mot de passe correct ?"}
    b -- non --> e401["401 (essais limités)"]
    b -- oui --> c{"Seul owner d'une org<br/>avec d'autres membres ?"}
    c -- oui --> e409a["409 : nommer un autre owner"]
    c -- non --> d{"Seul membre d'une org<br/>avec abonnement actif ?"}
    d -- oui --> e409b["409 : résilier d'abord"]
    d -- non --> f["Suppression :<br/>orgs dont il est seul membre,<br/>compte, sessions, conversations"]
    f --> g["Documents et invitations<br/>restent à l'org (auteur NULL)"]
```

---

## 11. Déploiements

```mermaid
flowchart LR
    subgraph saas["SaaS (hébergé par Lodestone)"]
        direction TB
        b1["Serveur"] --> p1[("Postgres")]
        b1 --> o1["Ollama ou OpenAI"]
        b1 --> s1["Stripe"]
        b1 --> m1["SMTP"]
    end

    subgraph sov["Sovereign (chez le client)"]
        direction TB
        rp["Reverse proxy HTTPS<br/>du client"] --> b2["Serveur<br/>NEW_ORG_PLAN=sovereign"]
        b2 --> p2[("Postgres")]
        b2 --> o2["Ollama local"]
        b2 --> m2["SMTP interne"]
    end
```

En Sovereign, **rien ne sort du réseau du client** : pas d'OpenAI, pas de Stripe.
Voir [`deploiement-sovereign.md`](deploiement-sovereign.md).
