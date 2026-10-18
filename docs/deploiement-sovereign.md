# Déploiement Sovereign (auto-hébergé)

Lodestone Sovereign tourne **entièrement sur l'infrastructure du client** : base de
données, modèles d'IA et serveur. Aucun document, aucune question, aucune réponse
ne quitte son réseau : pas d'API OpenAI, pas de Stripe, aucune télémétrie.

Fichiers fournis :

| Fichier | Rôle |
|---|---|
| `deploy/sovereign/docker-compose.yml` | Pile de production : Postgres + pgvector, Ollama, serveur Lodestone |
| `deploy/sovereign/sovereign.env.example` | Modèle de configuration (secrets, URLs, modèles) |

---

## 1. Prérequis

**Logiciels** : Linux x86_64, Docker ≥ 24 avec Docker Compose v2.

**Matériel** (ordre de grandeur pour une PME, quelques milliers de documents) :

| | Minimum (CPU seul) | Conseillé |
|---|---|---|
| CPU | 8 cœurs | 16 cœurs |
| RAM | 16 Go | 32 Go |
| GPU | — | NVIDIA ≥ 12 Go de VRAM |
| Disque | 50 Go SSD | 200 Go SSD |

Sans GPU, la recherche reste rapide (embeddings légers), mais la **génération des
réponses est lente** : plusieurs dizaines de secondes par réponse avec `llama3.1`.
Avec un GPU, quelques secondes. Le GPU s'active en décommentant le bloc `deploy`
du service `ollama` (nécessite le *NVIDIA Container Toolkit*).

---

## 2. Installation

```bash
cp deploy/sovereign/sovereign.env.example deploy/sovereign/.env
```

Remplir `deploy/sovereign/.env` :

- `POSTGRES_PASSWORD` : `openssl rand -hex 32` (hexadécimal obligatoire : le mot de
  passe est inséré dans une URL de connexion) ;
- `JWT_SECRET` : `openssl rand -hex 48` ;
- `APP_BASE_URL` et `CORS_ALLOWED_ORIGINS` : l'URL publique du front, par exemple
  `https://lodestone.mon-entreprise.local`.

Démarrer :

```bash
docker compose -f deploy/sovereign/docker-compose.yml --env-file deploy/sovereign/.env up -d --build
```

Au **premier démarrage**, le service `ollama-models` télécharge les modèles
(environ 5 Go pour `nomic-embed-text` + `llama3.1`) : le serveur Lodestone attend
la fin du téléchargement avant de démarrer. Suivre la progression :

```bash
docker compose -f deploy/sovereign/docker-compose.yml logs -f ollama-models backend
```

### Ce qui se passe au démarrage du serveur

1. **Migrations** de la base appliquées automatiquement (log `migrations appliquées`).
2. Documents dont l'ingestion a été interrompue par un arrêt : passés en `failed`
   (à ré-uploader).
3. Contrôle du modèle d'embedding (voir § 6).
4. Écoute sur le port 8090 (log `Lodestone à l'écoute`).

### Vérifier

```bash
curl -s http://127.0.0.1:8090/health/ready
# {"db":"up","status":"ready"}
```

Les orgs créées sur une installation Sovereign sont **directement en plan
Sovereign** (`NEW_ORG_PLAN=sovereign`) : aucun quota de membres, de documents ou
de questions, aucune facturation.

---

## 3. Reverse proxy HTTPS

Le serveur n'écoute que sur `127.0.0.1` (variable `LODESTONE_BIND`). Le placer
derrière le reverse proxy HTTPS du client. Deux points comptent :

- **WebSocket** sur `/ws` (chat en streaming) : l'upgrade doit être relayé ;
- **taille d'upload** : 20 Mo maximum par document.

Exemple nginx :

```nginx
server {
    listen 443 ssl;
    server_name lodestone.mon-entreprise.local;
    # ssl_certificate / ssl_certificate_key : certificats du client

    client_max_body_size 21m;

    location /ws {
        proxy_pass http://127.0.0.1:8090;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_read_timeout 600s;   # une génération peut durer plusieurs minutes sans GPU
    }

    location / {
        proxy_pass http://127.0.0.1:8090;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
    }
}
```

> **Ne pas journaliser les URLs de `/ws`** dans le reverse proxy : le jeton
> d'accès y passe en paramètre (`/ws?token=…`). Pour la même raison, garder
> `RUST_LOG=info` côté serveur (jamais `tower_http=debug`).

---

## 4. Installation hors ligne (serveur sans accès Internet)

Préparer sur une machine connectée, puis transférer :

```bash
# Images
docker compose -f deploy/sovereign/docker-compose.yml build
docker pull pgvector/pgvector:pg16
docker pull ollama/ollama:latest
docker save lodestone:local pgvector/pgvector:pg16 ollama/ollama:latest -o lodestone-images.tar

# Modèles : démarrer ollama une fois ici, puis archiver son volume
docker compose -f deploy/sovereign/docker-compose.yml --env-file deploy/sovereign/.env up -d ollama ollama-models
docker run --rm -v lodestone-sovereign_ollama:/data -v "$PWD":/out alpine \
  tar czf /out/ollama-models.tgz -C /data .
```

Sur le serveur du client :

```bash
docker load -i lodestone-images.tar
docker volume create lodestone-sovereign_ollama
docker run --rm -v lodestone-sovereign_ollama:/data -v "$PWD":/in alpine \
  tar xzf /in/ollama-models.tgz -C /data
docker compose -f deploy/sovereign/docker-compose.yml --env-file deploy/sovereign/.env up -d
```

`ollama pull` trouve alors les modèles déjà présents et ne télécharge rien.
Une fois les modèles en place, **tout le trafic sortant peut être bloqué** par le
pare-feu : Lodestone n'en a pas besoin.

---

## 5. Bascule Ollama ↔ OpenAI (déploiements SaaS uniquement)

Le client LLM parle l'API compatible OpenAI : le fournisseur se change par la
seule configuration, sans recompiler.

| Variable | Ollama (défaut) | OpenAI |
|---|---|---|
| `LLM_BASE_URL` | `http://ollama:11434/v1` | `https://api.openai.com/v1` |
| `LLM_API_KEY` | (vide) | `sk-…` |
| `EMBEDDING_MODEL` | `nomic-embed-text` | `text-embedding-3-small` |
| `CHAT_MODEL` | `llama3.1` | `gpt-4o-mini` |

La base stocke des vecteurs de **768 dimensions**. Les modèles `text-embedding-3-*`
en produisent davantage par défaut : Lodestone leur demande automatiquement 768
dimensions (paramètre `dimensions`), aucune migration n'est nécessaire.

> Une installation **Sovereign n'utilise jamais OpenAI** : c'est précisément la
> garantie vendue au client.

`OLLAMA_BASE_URL` reste accepté (anciennes configurations) ; `LLM_BASE_URL` prime.

---

## 6. Changer de modèle d'embedding

Les vecteurs de deux modèles différents **ne sont pas comparables**. Après un
changement de `EMBEDDING_MODEL` (y compris la bascule Ollama → OpenAI), les
documents déjà indexés donneraient des résultats de recherche faux, sans erreur
visible.

Lodestone enregistre le modèle utilisé pour chaque document et, **au démarrage**,
signale en erreur ceux qui ont été indexés avec un autre modèle :

```
ERROR DOCUMENTS À RÉ-UPLOADER : indexés avec un autre modèle d'embedding, …
      current_model=text-embedding-3-small other_model=nomic-embed-text count=42
```

Les fichiers d'origine ne sont pas conservés par Lodestone : ces documents sont à
**supprimer puis ré-uploader**. Le nouveau modèle doit produire 768 dimensions
(ou être un `text-embedding-3-*`), sinon l'ingestion échoue explicitement.

---

## 7. Exploitation

**Sauvegarde** : seule la base est à sauvegarder (les modèles se re-téléchargent).

```bash
docker compose -f deploy/sovereign/docker-compose.yml exec -T db \
  pg_dump -U lodestone -Fc lodestone > lodestone-$(date +%F).dump
```

Restauration : `pg_restore -U lodestone -d lodestone --clean` dans le conteneur `db`.

**Mise à jour** : récupérer la nouvelle version, puis relancer avec `--build`. Les
migrations s'appliquent au démarrage ; pour les lancer à part (plusieurs
instances), mettre `RUN_MIGRATIONS=false` sur les serveurs.

**Versions** : en production, figer `OLLAMA_VERSION` et `LODESTONE_VERSION` dans
`.env` pour des mises à jour maîtrisées.

---

## 8. Variables d'environnement

| Variable | Défaut | Rôle |
|---|---|---|
| `DATABASE_URL` | — (obligatoire) | Connexion Postgres (fournie par le compose) |
| `JWT_SECRET` | — (obligatoire) | Signature des jetons d'accès |
| `NEW_ORG_PLAN` | `free` | `sovereign` : orgs sans quotas ni facturation |
| `LLM_BASE_URL` | `http://localhost:11434/v1` | API compatible OpenAI (Ollama, OpenAI) |
| `LLM_API_KEY` | (vide) | Clé de l'API (OpenAI) ; inutile pour Ollama |
| `EMBEDDING_MODEL` | `nomic-embed-text` | Modèle d'embedding (768 dimensions) |
| `CHAT_MODEL` | `llama3.1` | Modèle de génération |
| `CORS_ALLOWED_ORIGINS` | `http://localhost:3000` | Origines autorisées (navigateur) |
| `APP_BASE_URL` | `http://localhost:3000` | URL publique du front |
| `APP_PORT` | `8090` | Port d'écoute |
| `RUN_MIGRATIONS` | `true` | Migrations au démarrage |
| `RUST_LOG` | `info` | Niveau de logs |

Variables de facturation (`STRIPE_*`, `PRO_*`, `SOVEREIGN_*`) : sans objet pour une
installation Sovereign, laisser vides.
