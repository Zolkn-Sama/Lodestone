use axum::http::HeaderValue;

use crate::mail::{SmtpConfig, SmtpSecurity};
use lodestone_core::{
    plan::Plan,
    pricing::{ProPricing, SovereignPricing},
};

/// Origine autorisée par défaut : le front Next.js en développement.
pub const DEFAULT_CORS_ORIGIN: &str = "http://localhost:3000";

pub struct Config {
    pub database_url: String,
    /// Origines (navigateur) autorisées à appeler l'API.
    pub cors_allowed_origins: Vec<HeaderValue>,
    /// API compatible OpenAI : Ollama (`http://localhost:11434/v1`) ou OpenAI.
    pub llm_base_url: String,
    /// Clé de l'API LLM (OpenAI) ; inutile pour Ollama.
    pub llm_api_key: Option<String>,
    pub embedding_model: String,
    /// Modèle de génération des réponses (`llama3.1`, `qwen2.5`…).
    pub chat_model: String,
    pub(crate) jwt_secret: String,
    pub app_port: u16,
    /// SMTP pour les e-mails ; `None` : aucun e-mail envoyé (journalisé).
    pub smtp: Option<SmtpConfig>,
    /// Derrière un reverse proxy : l'IP du client est lue dans X-Forwarded-For.
    pub trust_proxy_headers: bool,
    /// Plan des orgs créées : Free (SaaS) ou Sovereign (auto-hébergé, sans limite).
    pub new_org_plan: Plan,
    /// Appliquer les migrations au démarrage (défaut : oui). À désactiver si
    /// elles sont lancées à part (ex. plusieurs instances).
    pub run_migrations: bool,
    /// URL du front : pages de retour après paiement.
    pub app_base_url: String,
    /// `None` : facturation désactivée (les routes de paiement répondent 503).
    pub stripe: Option<StripeConfig>,
    /// Tarif Pro (prix par siège, socle) : source de vérité côté plateforme.
    pub pro_pricing: ProPricing,
    /// Tarif Sovereign (licence annuelle, mise en service).
    pub sovereign_pricing: SovereignPricing,
}

/// Les trois valeurs vont ensemble : toutes ou aucune.
pub struct StripeConfig {
    pub secret_key: String,
    pub webhook_secret: String,
    /// Prix Stripe de l'abonnement Pro mensuel par siège (`price_…`).
    pub pro_price_id: String,
    /// Prix annuel (avec remise) ; `None` : seul le mensuel est proposé.
    pub pro_annual_price_id: Option<String>,
    /// Licence Sovereign (récurrente annuelle) et mise en service (ponctuelle) :
    /// les deux ou aucun. `None` : activation Sovereign hors Stripe.
    pub sovereign_prices: Option<SovereignPrices>,
}

pub struct SovereignPrices {
    pub license_price_id: String,
    pub setup_price_id: String,
}

impl Config {
    /// Config construite à la main, sans lire l'environnement (tests d'intégration).
    pub fn new(database_url: String, jwt_secret: String) -> Self {
        Self {
            database_url,
            cors_allowed_origins: vec![HeaderValue::from_static(DEFAULT_CORS_ORIGIN)],
            llm_base_url: "http://localhost:11434/v1".to_string(),
            llm_api_key: None,
            embedding_model: "nomic-embed-text".to_string(),
            chat_model: "llama3.1".to_string(),
            jwt_secret,
            app_port: 0,
            run_migrations: true,
            new_org_plan: Plan::Free,
            smtp: None,
            trust_proxy_headers: false,
            app_base_url: DEFAULT_CORS_ORIGIN.to_string(),
            stripe: None,
            pro_pricing: ProPricing::default(),
            sovereign_pricing: SovereignPricing::default(),
        }
    }

    pub fn from_env() -> anyhow::Result<Self> {
        fn required(key: &str) -> anyhow::Result<String> {
            std::env::var(key)
                .map_err(|_| anyhow::anyhow!("variable d'environnement manquante: {key}"))
        }
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            cors_allowed_origins: parse_origins(
                &std::env::var("CORS_ALLOWED_ORIGINS")
                    .unwrap_or_else(|_| DEFAULT_CORS_ORIGIN.to_string()),
            )?,
            llm_base_url: llm_base_url_from(|k| std::env::var(k).ok()),
            llm_api_key: std::env::var("LLM_API_KEY").ok().filter(|k| !k.is_empty()),
            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string()),
            chat_model: std::env::var("CHAT_MODEL").unwrap_or_else(|_| "llama3.1".to_string()),
            jwt_secret: required("JWT_SECRET")?,
            app_port: std::env::var("APP_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(8090),
            new_org_plan: new_org_plan_from(|k| std::env::var(k).ok())?,
            smtp: smtp_from(|k| std::env::var(k).ok())?,
            trust_proxy_headers: matches!(
                std::env::var("TRUST_PROXY_HEADERS")
                    .ok()
                    .as_deref()
                    .map(str::trim),
                Some("true" | "1" | "yes")
            ),
            run_migrations: !matches!(
                std::env::var("RUN_MIGRATIONS")
                    .ok()
                    .as_deref()
                    .map(str::trim),
                Some("false" | "0" | "no")
            ),
            app_base_url: std::env::var("APP_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_CORS_ORIGIN.to_string())
                .trim_end_matches('/')
                .to_string(),
            stripe: stripe_from(|k| std::env::var(k).ok())?,
            pro_pricing: pricing_from(|k| std::env::var(k).ok())?,
            sovereign_pricing: sovereign_pricing_from(|k| std::env::var(k).ok())?,
        })
    }
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("database_url", &"***") // contient le mot de passe DB
            .field("cors_allowed_origins", &self.cors_allowed_origins)
            .field("llm_base_url", &self.llm_base_url)
            .field("llm_api_key", &self.llm_api_key.as_ref().map(|_| "***"))
            .field("embedding_model", &self.embedding_model)
            .field("chat_model", &self.chat_model)
            .field("jwt_secret", &"***")
            .field("app_port", &self.app_port)
            .field("app_base_url", &self.app_base_url)
            .field("new_org_plan", &self.new_org_plan)
            .field("run_migrations", &self.run_migrations)
            .field(
                "smtp",
                &self.smtp.as_ref().map(|s| format!("{}:{}", s.host, s.port)),
            )
            .field("trust_proxy_headers", &self.trust_proxy_headers)
            .field("stripe", &self.stripe.as_ref().map(|_| "configuré (***)"))
            .field("pro_pricing", &self.pro_pricing)
            .field("sovereign_pricing", &self.sovereign_pricing)
            .finish()
    }
}

/// SMTP_HOST (absent : e-mails désactivés), SMTP_SECURITY (`starttls` par
/// défaut, `tls`, `none`), SMTP_PORT (défaut selon la sécurité),
/// SMTP_USERNAME / SMTP_PASSWORD (ensemble ou aucun), MAIL_FROM (obligatoire).
fn smtp_from(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<SmtpConfig>> {
    let get = |k: &str| {
        var(k)
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
    };
    let Some(host) = get("SMTP_HOST") else {
        return Ok(None);
    };
    let security = match get("SMTP_SECURITY").as_deref() {
        None | Some("starttls") => SmtpSecurity::StartTls,
        Some("tls") => SmtpSecurity::Tls,
        Some("none") => SmtpSecurity::None,
        Some(other) => {
            anyhow::bail!("SMTP_SECURITY : starttls, tls ou none attendu, reçu « {other} »")
        }
    };
    let port = match get("SMTP_PORT") {
        Some(p) => p
            .parse::<u16>()
            .map_err(|_| anyhow::anyhow!("SMTP_PORT : port invalide « {p} »"))?,
        None => match security {
            SmtpSecurity::Tls => 465,
            SmtpSecurity::StartTls => 587,
            SmtpSecurity::None => 25,
        },
    };
    let (username, password) = match (get("SMTP_USERNAME"), get("SMTP_PASSWORD")) {
        (Some(u), Some(p)) => (Some(u), Some(p)),
        (None, None) => (None, None),
        _ => anyhow::bail!("SMTP_USERNAME et SMTP_PASSWORD vont ensemble"),
    };
    let from = get("MAIL_FROM")
        .ok_or_else(|| anyhow::anyhow!("MAIL_FROM est obligatoire quand SMTP_HOST est défini"))?;
    Ok(Some(SmtpConfig {
        host,
        port,
        security,
        username,
        password,
        from,
    }))
}

/// NEW_ORG_PLAN : `free` (défaut, SaaS) ou `sovereign` (installation
/// auto-hébergée : pas de quotas, pas de Stripe). `pro` est refusé : il
/// suppose un abonnement payé.
fn new_org_plan_from(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Plan> {
    match var("NEW_ORG_PLAN")
        .map(|v| v.trim().to_lowercase())
        .as_deref()
    {
        None | Some("") | Some("free") => Ok(Plan::Free),
        Some("sovereign") => Ok(Plan::Sovereign),
        Some(other) => {
            anyhow::bail!("NEW_ORG_PLAN : « free » ou « sovereign » attendu, reçu « {other} »")
        }
    }
}

/// URL de l'API LLM compatible OpenAI. `LLM_BASE_URL` (nom neutre : Ollama ou
/// OpenAI) prime ; `OLLAMA_BASE_URL` reste accepté (anciennes configurations).
fn llm_base_url_from(var: impl Fn(&str) -> Option<String>) -> String {
    let get = |k: &str| var(k).filter(|v| !v.trim().is_empty());
    get("LLM_BASE_URL")
        .or_else(|| get("OLLAMA_BASE_URL"))
        .unwrap_or_else(|| "http://localhost:11434/v1".to_string())
}

/// STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET, STRIPE_PRICE_PRO : toutes ou
/// aucune. Une configuration partielle fait échouer le démarrage plutôt que de
/// laisser une facturation à moitié branchée.
fn stripe_from(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<Option<StripeConfig>> {
    let get = |k: &str| var(k).filter(|v| !v.trim().is_empty());
    match (
        get("STRIPE_SECRET_KEY"),
        get("STRIPE_WEBHOOK_SECRET"),
        get("STRIPE_PRICE_PRO"),
    ) {
        (Some(secret_key), Some(webhook_secret), Some(pro_price_id)) => Ok(Some(StripeConfig {
            secret_key,
            webhook_secret,
            pro_price_id,
            pro_annual_price_id: get("STRIPE_PRICE_PRO_ANNUAL"),
            sovereign_prices: match (
                get("STRIPE_PRICE_SOVEREIGN_LICENSE"),
                get("STRIPE_PRICE_SOVEREIGN_SETUP"),
            ) {
                (Some(license_price_id), Some(setup_price_id)) => Some(SovereignPrices {
                    license_price_id,
                    setup_price_id,
                }),
                (None, None) => None,
                _ => anyhow::bail!(
                    "STRIPE_PRICE_SOVEREIGN_LICENSE et STRIPE_PRICE_SOVEREIGN_SETUP vont ensemble"
                ),
            },
        })),
        (None, None, None) => {
            for key in [
                "STRIPE_PRICE_PRO_ANNUAL",
                "STRIPE_PRICE_SOVEREIGN_LICENSE",
                "STRIPE_PRICE_SOVEREIGN_SETUP",
            ] {
                if get(key).is_some() {
                    anyhow::bail!("{key} sans le reste de la configuration Stripe");
                }
            }
            Ok(None)
        }
        _ => anyhow::bail!(
            "configuration Stripe incomplète : STRIPE_SECRET_KEY, STRIPE_WEBHOOK_SECRET et STRIPE_PRICE_PRO vont ensemble"
        ),
    }
}

/// PRO_SEAT_PRICE_CENTS / PRO_FLOOR_CENTS (centimes HT), défauts 2900 / 14900.
/// SOVEREIGN_LICENSE_CENTS / SOVEREIGN_SETUP_CENTS (centimes HT), défauts
/// 1 800 000 (licence annuelle) / 600 000 (mise en service).
fn sovereign_pricing_from(
    var: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<SovereignPricing> {
    let default = SovereignPricing::default();
    let read = |key: &str, default: u32| -> anyhow::Result<u32> {
        match var(key).filter(|v| !v.trim().is_empty()) {
            None => Ok(default),
            Some(v) => v
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    anyhow::anyhow!("{key} : entier positif attendu (centimes), reçu « {v} »")
                }),
        }
    };
    Ok(SovereignPricing {
        license_cents_per_year: read("SOVEREIGN_LICENSE_CENTS", default.license_cents_per_year)?,
        setup_cents: read("SOVEREIGN_SETUP_CENTS", default.setup_cents)?,
    })
}

fn pricing_from(var: impl Fn(&str) -> Option<String>) -> anyhow::Result<ProPricing> {
    let default = ProPricing::default();
    let read = |key: &str, default: u32| -> anyhow::Result<u32> {
        match var(key).filter(|v| !v.trim().is_empty()) {
            None => Ok(default),
            Some(v) => v
                .trim()
                .parse::<u32>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    anyhow::anyhow!("{key} : entier positif attendu (centimes), reçu « {v} »")
                }),
        }
    };
    let discount = match var("PRO_ANNUAL_DISCOUNT_PERCENT").filter(|v| !v.trim().is_empty()) {
        None => default.annual_discount_percent,
        Some(v) => v
            .trim()
            .parse::<u32>()
            .ok()
            .filter(|n| *n < 100)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "PRO_ANNUAL_DISCOUNT_PERCENT : entier de 0 à 99 attendu, reçu « {v} »"
                )
            })?,
    };
    Ok(ProPricing {
        seat_price_cents: read("PRO_SEAT_PRICE_CENTS", default.seat_price_cents)?,
        floor_cents: read("PRO_FLOOR_CENTS", default.floor_cents)?,
        annual_discount_percent: discount,
    })
}

/// `"https://app.exemple.fr, http://localhost:3000"` -> liste d'origines.
/// Une origine invalide fait échouer le démarrage plutôt que d'être ignorée.
pub fn parse_origins(raw: &str) -> anyhow::Result<Vec<HeaderValue>> {
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(|o| {
            let o = o.trim_end_matches('/');
            if !(o.starts_with("http://") || o.starts_with("https://"))
                || o.contains(char::is_whitespace)
            {
                anyhow::bail!("CORS_ALLOWED_ORIGINS : origine invalide « {o} » (attendu http(s)://hôte[:port])");
            }
            HeaderValue::from_str(o)
                .map_err(|_| anyhow::anyhow!("CORS_ALLOWED_ORIGINS : origine invalide « {o} »"))
        })
        .collect::<anyhow::Result<_>>()?;
    if origins.is_empty() {
        anyhow::bail!("CORS_ALLOWED_ORIGINS est vide");
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn tarif_configurable() {
        assert_eq!(
            pricing_from(vars(&[])).expect("défauts"),
            ProPricing::default()
        );
        let p = pricing_from(vars(&[
            ("PRO_SEAT_PRICE_CENTS", "2500"),
            ("PRO_FLOOR_CENTS", " 9900 "),
        ]))
        .expect("valide");
        assert_eq!((p.seat_price_cents, p.floor_cents), (2500, 9900));
        let p = pricing_from(vars(&[("PRO_ANNUAL_DISCOUNT_PERCENT", "0")])).expect("0 % autorisé");
        assert_eq!(p.annual_discount_percent, 0);
        for bad in ["100", "-5", "15.5"] {
            assert!(
                pricing_from(vars(&[("PRO_ANNUAL_DISCOUNT_PERCENT", bad)])).is_err(),
                "remise {bad}"
            );
        }
        for bad in ["0", "-1", "29.00", "abc"] {
            assert!(
                pricing_from(vars(&[("PRO_FLOOR_CENTS", bad)])).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn configuration_smtp() {
        assert!(
            smtp_from(vars(&[])).expect("ok").is_none(),
            "désactivé par défaut"
        );

        let smtp = smtp_from(vars(&[
            ("SMTP_HOST", "smtp.exemple.fr"),
            ("MAIL_FROM", "Lodestone <no-reply@exemple.fr>"),
        ]))
        .expect("ok")
        .expect("configuré");
        assert_eq!((smtp.port, smtp.security), (587, SmtpSecurity::StartTls));
        assert_eq!(smtp.username, None);

        let mailpit = smtp_from(vars(&[
            ("SMTP_HOST", "localhost"),
            ("SMTP_PORT", "1025"),
            ("SMTP_SECURITY", "none"),
            ("MAIL_FROM", "dev@lodestone.local"),
        ]))
        .expect("ok")
        .expect("configuré");
        assert_eq!((mailpit.port, mailpit.security), (1025, SmtpSecurity::None));

        for bad in [
            vec![("SMTP_HOST", "h")], // sans MAIL_FROM
            vec![
                ("SMTP_HOST", "h"),
                ("MAIL_FROM", "a@b.fr"),
                ("SMTP_USERNAME", "u"),
            ], // sans mot de passe
            vec![
                ("SMTP_HOST", "h"),
                ("MAIL_FROM", "a@b.fr"),
                ("SMTP_SECURITY", "ssl"),
            ],
            vec![
                ("SMTP_HOST", "h"),
                ("MAIL_FROM", "a@b.fr"),
                ("SMTP_PORT", "99999"),
            ],
        ] {
            assert!(smtp_from(vars(&bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn plan_des_nouvelles_orgs() {
        assert_eq!(new_org_plan_from(vars(&[])).expect("défaut"), Plan::Free);
        assert_eq!(
            new_org_plan_from(vars(&[("NEW_ORG_PLAN", " Sovereign ")])).expect("ok"),
            Plan::Sovereign
        );
        for bad in ["pro", "enterprise"] {
            assert!(
                new_org_plan_from(vars(&[("NEW_ORG_PLAN", bad)])).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn url_du_llm() {
        assert_eq!(llm_base_url_from(vars(&[])), "http://localhost:11434/v1");
        assert_eq!(
            llm_base_url_from(vars(&[("OLLAMA_BASE_URL", "http://gpu-box:11434/v1")])),
            "http://gpu-box:11434/v1",
            "ancien nom accepté"
        );
        assert_eq!(
            llm_base_url_from(vars(&[
                ("OLLAMA_BASE_URL", "http://ollama:11434/v1"),
                ("LLM_BASE_URL", "https://api.openai.com/v1"),
            ])),
            "https://api.openai.com/v1",
            "LLM_BASE_URL prime"
        );
    }

    #[test]
    fn tarif_sovereign_configurable() {
        assert_eq!(
            sovereign_pricing_from(vars(&[])).expect("défauts"),
            SovereignPricing::default()
        );
        let p = sovereign_pricing_from(vars(&[("SOVEREIGN_SETUP_CENTS", "500000")])).expect("ok");
        assert_eq!(p.setup_cents, 500_000);
        assert!(sovereign_pricing_from(vars(&[("SOVEREIGN_LICENSE_CENTS", "0")])).is_err());
    }

    #[test]
    fn prix_sovereign_ensemble() {
        let base = [
            ("STRIPE_SECRET_KEY", "sk"),
            ("STRIPE_WEBHOOK_SECRET", "whsec"),
            ("STRIPE_PRICE_PRO", "price_pro"),
        ];
        let stripe = stripe_from(vars(&base)).expect("ok").expect("configuré");
        assert!(stripe.sovereign_prices.is_none(), "facultatif");

        let mut both = base.to_vec();
        both.push(("STRIPE_PRICE_SOVEREIGN_LICENSE", "price_lic"));
        both.push(("STRIPE_PRICE_SOVEREIGN_SETUP", "price_setup"));
        let prices = stripe_from(vars(&both))
            .expect("ok")
            .expect("configuré")
            .sovereign_prices
            .expect("Sovereign");
        assert_eq!(
            (
                prices.license_price_id.as_str(),
                prices.setup_price_id.as_str()
            ),
            ("price_lic", "price_setup")
        );

        let mut license_only = base.to_vec();
        license_only.push(("STRIPE_PRICE_SOVEREIGN_LICENSE", "price_lic"));
        assert!(
            stripe_from(vars(&license_only)).is_err(),
            "licence sans mise en service"
        );
        assert!(
            stripe_from(vars(&[("STRIPE_PRICE_SOVEREIGN_SETUP", "x")])).is_err(),
            "sans le reste de Stripe"
        );
    }

    #[test]
    fn stripe_tout_ou_rien() {
        let complete = [
            ("STRIPE_SECRET_KEY", "sk_test_1"),
            ("STRIPE_WEBHOOK_SECRET", "whsec_1"),
            ("STRIPE_PRICE_PRO", "price_1"),
        ];
        let stripe = stripe_from(vars(&complete))
            .expect("ok")
            .expect("configuré");
        assert_eq!(stripe.pro_annual_price_id, None, "annuel facultatif");
        let mut with_annual = complete.to_vec();
        with_annual.push(("STRIPE_PRICE_PRO_ANNUAL", "price_year"));
        let stripe = stripe_from(vars(&with_annual))
            .expect("ok")
            .expect("configuré");
        assert_eq!(stripe.pro_annual_price_id.as_deref(), Some("price_year"));
        assert!(
            stripe_from(vars(&[("STRIPE_PRICE_PRO_ANNUAL", "price_year")])).is_err(),
            "annuel seul"
        );
        assert_eq!(stripe.pro_price_id, "price_1");

        assert!(
            stripe_from(vars(&[])).expect("ok").is_none(),
            "aucune : désactivé"
        );
        assert!(
            stripe_from(vars(&[("STRIPE_SECRET_KEY", "  ")]))
                .expect("ok")
                .is_none(),
            "valeur vide = absente"
        );
        for missing in 0..3 {
            let partial: Vec<(&str, &str)> = complete
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != missing)
                .map(|(_, p)| *p)
                .collect();
            assert!(
                stripe_from(vars(&partial)).is_err(),
                "sans {}",
                complete[missing].0
            );
        }
    }

    #[test]
    fn origines_valides() {
        let origins =
            parse_origins(" https://app.lodestone.fr/ ,http://localhost:3000").expect("valide");
        assert_eq!(
            origins,
            vec![
                HeaderValue::from_static("https://app.lodestone.fr"),
                HeaderValue::from_static("http://localhost:3000")
            ]
        );
    }

    #[test]
    fn origines_invalides() {
        for raw in ["", " , ", "*", "app.lodestone.fr", "ftp://x", "http://a b"] {
            assert!(parse_origins(raw).is_err(), "{raw:?}");
        }
    }
}
