# Lodestone

> La mémoire interrogeable de votre organisation.

**Lodestone** transforme la documentation interne éparse d'une organisation — procédures, comptes-rendus, documentation technique, contrats — en une base de connaissances que l'on interroge en **langage naturel**. Chaque réponse **cite ses sources**, pour qu'on puisse lui faire confiance.

## Le problème

Dans la plupart des organisations, l'information existe… mais personne ne sait *où c'est écrit*. On perd un temps considérable à chercher, à redemander, ou à réécrire ce qui existe déjà quelque part.

## La visée

Offrir à chaque équipe un point d'entrée unique vers son propre savoir : poser une question, obtenir une réponse **fiable et sourcée** en quelques secondes — sans fouiller dix outils.

## État du projet

Projet en **développement actif**, construit en public à des fins d'apprentissage et de portfolio. Le backend est écrit en Rust ; le frontend et le déploiement viendront plus tard.

**Déjà en place :**
- **Socle** — workspace multi-crates, configuration, Docker (PostgreSQL + pgvector), intégration continue (format · lint · tests).
- **Base de données** — migrations versionnées, schéma utilisateurs / organisations.
- **Authentification** — inscription, connexion, hachage `argon2id`, JWT (access + refresh avec rotation), routes protégées par extracteur.

**Prochaines étapes :** organisations & rôles (RBAC), ingestion de documents, recherche sémantique et interface de chat.

## Stack technique

Rust · Axum · SeaORM · PostgreSQL + pgvector · JWT. *(Détails dans `Cargo.toml`.)*

## Contact

*LANDRECY — enzo.landrecy@gmail.com · [linkedin.com/in/enzo-landrecy](https://www.linkedin.com/in/enzo-landrecy)*

---

<sub>© 2026 LANDRECY. Tous droits réservés. Nom de projet provisoire.</sub>
