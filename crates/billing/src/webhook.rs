//! Webhooks Stripe : vérification de la signature, puis décodage des seuls
//! événements utiles au plan d'une org.

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2_hmac::Sha256;
use uuid::Uuid;

use crate::error::WebhookError;

/// Écart toléré entre l'horodatage signé et l'horloge du serveur (Stripe : 5 min).
pub const SIGNATURE_TOLERANCE_SECS: i64 = 300;

type HmacSha256 = Hmac<Sha256>;

/// Vérifie l'en-tête `Stripe-Signature` (`t=<horodatage>,v1=<hmac hex>[,v1=…]`) :
/// HMAC-SHA256 de `"<t>.<corps>"` avec le secret du webhook, comparé en temps
/// constant, et horodatage à moins de `SIGNATURE_TOLERANCE_SECS` (anti-rejeu).
pub fn verify_signature(
    payload: &[u8],
    header: &str,
    secret: &str,
    now: i64,
) -> Result<(), WebhookError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        match part.trim().split_once('=') {
            Some(("t", t)) => timestamp = t.parse::<i64>().ok(),
            Some(("v1", sig)) => signatures.push(sig),
            _ => {}
        }
    }
    let timestamp = timestamp.ok_or(WebhookError::MalformedHeader)?;
    if signatures.is_empty() {
        return Err(WebhookError::MalformedHeader);
    }
    if (now - timestamp).abs() > SIGNATURE_TOLERANCE_SECS {
        return Err(WebhookError::TimestampOutOfTolerance);
    }

    // Stripe peut envoyer plusieurs v1 (rotation du secret) : une seule suffit.
    for sig in signatures {
        let Ok(expected) = hex::decode(sig) else {
            continue;
        };
        // verify_slice compare en temps constant.
        if signer(secret, timestamp, payload)
            .verify_slice(&expected)
            .is_ok()
        {
            return Ok(());
        }
    }
    Err(WebhookError::InvalidSignature)
}

fn signer(secret: &str, timestamp: i64, payload: &[u8]) -> HmacSha256 {
    // HMAC accepte une clé de n'importe quelle longueur : ne peut pas échouer.
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes())
        .unwrap_or_else(|_| unreachable!("HMAC accepte toute longueur de clé"));
    mac.update(timestamp.to_string().as_bytes());
    mac.update(b".");
    mac.update(payload);
    mac
}

/// Construit un en-tête `Stripe-Signature` valide (tests, outils locaux) :
/// exactement ce que fait Stripe à l'envoi.
pub fn sign_payload(payload: &[u8], secret: &str, timestamp: i64) -> String {
    let sig = hex::encode(signer(secret, timestamp, payload).finalize().into_bytes());
    format!("t={timestamp},v1={sig}")
}

/// Statut d'un abonnement Stripe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscriptionStatus {
    Active,
    Trialing,
    /// Paiement en échec, Stripe relance : on garde le Pro pendant les relances.
    PastDue,
    Unpaid,
    Canceled,
    Incomplete,
    IncompleteExpired,
    Paused,
    Other(String),
}

impl SubscriptionStatus {
    pub fn parse(s: &str) -> Self {
        match s {
            "active" => Self::Active,
            "trialing" => Self::Trialing,
            "past_due" => Self::PastDue,
            "unpaid" => Self::Unpaid,
            "canceled" => Self::Canceled,
            "incomplete" => Self::Incomplete,
            "incomplete_expired" => Self::IncompleteExpired,
            "paused" => Self::Paused,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Trialing => "trialing",
            Self::PastDue => "past_due",
            Self::Unpaid => "unpaid",
            Self::Canceled => "canceled",
            Self::Incomplete => "incomplete",
            Self::IncompleteExpired => "incomplete_expired",
            Self::Paused => "paused",
            Self::Other(s) => s,
        }
    }

    /// L'abonnement donne-t-il droit au plan Pro ?
    pub fn grants_pro(&self) -> bool {
        matches!(self, Self::Active | Self::Trialing | Self::PastDue)
    }
}

/// État d'un abonnement tel que décrit par un événement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubscriptionState {
    pub subscription_id: String,
    pub customer_id: Option<String>,
    /// Lu dans les métadonnées posées à la création de la session.
    pub org_id: Option<Uuid>,
    pub status: SubscriptionStatus,
    pub seats: Option<u32>,
    /// Ligne d'abonnement (`si_…`) dont la quantité = nombre de sièges.
    pub item_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BillingEvent {
    /// Paiement initial réussi : l'abonnement existe.
    CheckoutCompleted {
        org_id: Uuid,
        customer_id: Option<String>,
        subscription_id: String,
    },
    /// Création, mise à jour (statut, sièges) ou résiliation d'un abonnement.
    Subscription(SubscriptionState),
    /// Type d'événement que l'on n'exploite pas (accusé réception quand même).
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookEvent {
    /// `evt_…` : sert à ignorer les doublons (Stripe peut renvoyer un événement).
    pub id: String,
    pub event_type: String,
    /// Horodatage de l'événement : sert à ignorer un événement plus ancien que
    /// le dernier appliqué (Stripe ne garantit pas l'ordre).
    pub created: i64,
    pub event: BillingEvent,
}

/// Décode un événement (corps déjà authentifié par `verify_signature`).
pub fn parse_event(payload: &[u8]) -> Result<WebhookEvent, WebhookError> {
    let json: Value =
        serde_json::from_slice(payload).map_err(|e| WebhookError::InvalidPayload(e.to_string()))?;
    let field = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let invalid = |m: &str| WebhookError::InvalidPayload(m.to_string());

    let id = field(&json, "id").ok_or_else(|| invalid("id manquant"))?;
    let event_type = field(&json, "type").ok_or_else(|| invalid("type manquant"))?;
    let created = json["created"]
        .as_i64()
        .ok_or_else(|| invalid("created manquant"))?;
    let object = &json["data"]["object"];
    let org_id = |obj: &Value| {
        obj["metadata"]["org_id"]
            .as_str()
            .and_then(|s| s.parse::<Uuid>().ok())
    };

    let event = match event_type.as_str() {
        "checkout.session.completed" => {
            // Une session hors abonnement (paiement unique) ne nous concerne pas ici.
            match (org_id(object), field(object, "subscription")) {
                (Some(org_id), Some(subscription_id)) => BillingEvent::CheckoutCompleted {
                    org_id,
                    customer_id: field(object, "customer"),
                    subscription_id,
                },
                _ => BillingEvent::Ignored,
            }
        }
        "customer.subscription.created"
        | "customer.subscription.updated"
        | "customer.subscription.deleted" => {
            let subscription_id =
                field(object, "id").ok_or_else(|| invalid("abonnement sans id"))?;
            let status = if event_type == "customer.subscription.deleted" {
                SubscriptionStatus::Canceled
            } else {
                SubscriptionStatus::parse(
                    &field(object, "status").ok_or_else(|| invalid("statut manquant"))?,
                )
            };
            let item = &object["items"]["data"][0];
            let seats = item["quantity"]
                .as_u64()
                .or_else(|| object["quantity"].as_u64())
                .and_then(|q| u32::try_from(q).ok());
            BillingEvent::Subscription(SubscriptionState {
                subscription_id,
                customer_id: field(object, "customer"),
                org_id: org_id(object),
                status,
                seats,
                item_id: field(item, "id"),
            })
        }
        _ => BillingEvent::Ignored,
    };

    Ok(WebhookEvent {
        id,
        event_type,
        created,
        event,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const SECRET: &str = "whsec_test";
    const NOW: i64 = 1_760_000_000;

    /// Valeur calculée indépendamment (module `hmac` de Python) : protège
    /// contre une erreur « symétrique » (même bug à la signature et à la
    /// vérification) qui ferait rejeter tous les vrais webhooks Stripe.
    #[test]
    fn vecteur_de_reference() {
        let payload = br#"{"id":"evt_vector","object":"event"}"#;
        assert_eq!(
            sign_payload(payload, "whsec_vector_secret", 1_700_000_000),
            "t=1700000000,v1=495e6a9f2ae5410508d8fbee872cbf4050da9b6ada87e6d091cfc84dc5328839"
        );
    }

    #[test]
    fn signature_valide() {
        let payload = br#"{"id":"evt_1"}"#;
        let header = sign_payload(payload, SECRET, NOW);
        assert_eq!(verify_signature(payload, &header, SECRET, NOW), Ok(()));
        // Dans la tolérance.
        assert_eq!(
            verify_signature(payload, &header, SECRET, NOW + 299),
            Ok(())
        );
    }

    #[test]
    fn signatures_refusees() {
        let payload = br#"{"id":"evt_1"}"#;
        let header = sign_payload(payload, SECRET, NOW);

        // Corps modifié, mauvais secret.
        assert_eq!(
            verify_signature(br#"{"id":"evt_2"}"#, &header, SECRET, NOW),
            Err(WebhookError::InvalidSignature)
        );
        assert_eq!(
            verify_signature(payload, &header, "whsec_autre", NOW),
            Err(WebhookError::InvalidSignature)
        );
        // Rejeu tardif.
        assert_eq!(
            verify_signature(payload, &header, SECRET, NOW + 301),
            Err(WebhookError::TimestampOutOfTolerance)
        );
        // En-têtes mal formés.
        for bad in [
            "",
            "v1=abc",
            "t=abc,v1=00",
            &format!("t={NOW}"),
            &format!("t={NOW},v1=pas-hex"),
        ] {
            let res = verify_signature(payload, bad, SECRET, NOW);
            assert!(res.is_err(), "{bad:?} -> {res:?}");
        }
    }

    #[test]
    fn plusieurs_signatures_une_suffit() {
        let payload = br#"{"id":"evt_1"}"#;
        let good = sign_payload(payload, SECRET, NOW);
        let sig = good.split("v1=").nth(1).expect("v1");
        let header = format!("t={NOW},v1={},v1={sig},v0=ignore", "00".repeat(32));
        assert_eq!(verify_signature(payload, &header, SECRET, NOW), Ok(()));
    }

    fn event(event_type: &str, object: serde_json::Value) -> Vec<u8> {
        json!({ "id": "evt_123", "type": event_type, "created": NOW, "data": { "object": object } })
            .to_string()
            .into_bytes()
    }

    #[test]
    fn checkout_complete() {
        let org = Uuid::new_v4();
        let parsed = parse_event(&event(
            "checkout.session.completed",
            json!({ "id": "cs_1", "customer": "cus_1", "subscription": "sub_1", "metadata": { "org_id": org } }),
        ))
        .expect("événement");
        assert_eq!(parsed.id, "evt_123");
        assert_eq!(parsed.created, NOW);
        assert_eq!(
            parsed.event,
            BillingEvent::CheckoutCompleted {
                org_id: org,
                customer_id: Some("cus_1".into()),
                subscription_id: "sub_1".into()
            }
        );

        // Sans org (session créée ailleurs) : ignoré, pas une erreur.
        let parsed = parse_event(&event(
            "checkout.session.completed",
            json!({ "id": "cs_2", "mode": "payment" }),
        ))
        .expect("événement");
        assert_eq!(parsed.event, BillingEvent::Ignored);
    }

    #[test]
    fn abonnement_mis_a_jour_et_resilie() {
        let org = Uuid::new_v4();
        let object = json!({
            "id": "sub_1", "customer": "cus_1", "status": "past_due",
            "metadata": { "org_id": org },
            "items": { "data": [ { "id": "si_1", "quantity": 7 } ] }
        });
        let parsed = parse_event(&event("customer.subscription.updated", object.clone()))
            .expect("événement");
        let BillingEvent::Subscription(state) = parsed.event else {
            panic!("abonnement attendu");
        };
        assert_eq!(state.subscription_id, "sub_1");
        assert_eq!(state.org_id, Some(org));
        assert_eq!(state.status, SubscriptionStatus::PastDue);
        assert!(state.status.grants_pro(), "relances : on garde le Pro");
        assert_eq!(state.seats, Some(7));
        assert_eq!(state.item_id.as_deref(), Some("si_1"));

        let parsed =
            parse_event(&event("customer.subscription.deleted", object)).expect("événement");
        let BillingEvent::Subscription(state) = parsed.event else {
            panic!("abonnement attendu");
        };
        assert_eq!(state.status, SubscriptionStatus::Canceled);
        assert!(!state.status.grants_pro());
    }

    #[test]
    fn statuts_et_droits() {
        for (s, pro) in [
            ("active", true),
            ("trialing", true),
            ("past_due", true),
            ("unpaid", false),
            ("canceled", false),
            ("incomplete", false),
            ("incomplete_expired", false),
            ("paused", false),
            ("nouveau_statut", false),
        ] {
            let status = SubscriptionStatus::parse(s);
            assert_eq!(status.as_str(), s);
            assert_eq!(status.grants_pro(), pro, "{s}");
        }
    }

    #[test]
    fn evenements_inconnus_et_invalides() {
        let parsed =
            parse_event(&event("invoice.paid", json!({ "id": "in_1" }))).expect("événement");
        assert_eq!(parsed.event, BillingEvent::Ignored);
        assert_eq!(parsed.event_type, "invoice.paid");

        assert!(parse_event(b"pas du json").is_err());
        assert!(
            parse_event(br#"{"type":"x","created":1}"#).is_err(),
            "id manquant"
        );
        assert!(
            parse_event(&event(
                "customer.subscription.updated",
                json!({ "status": "active" })
            ))
            .is_err()
        );
    }
}
