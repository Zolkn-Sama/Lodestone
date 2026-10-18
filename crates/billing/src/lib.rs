//! lodestone-billing : abonnements (Stripe) derrière le trait `PaymentProvider`,
//! et vérification / décodage des webhooks. Rien ici ne touche à la base :
//! le backend applique les événements.

pub mod error;
pub mod mock;
pub mod provider;
pub mod stripe;
pub mod webhook;

pub use error::{BillingError, WebhookError};
pub use mock::{MOCK_PERIOD_END, MockPaymentProvider, PriceChange};
pub use provider::{CheckoutRequest, CheckoutSession, PaymentProvider, PriceInfo, PriceTier};
pub use stripe::StripeClient;
pub use webhook::{
    BillingEvent, SubscriptionState, SubscriptionStatus, WebhookEvent, parse_event, sign_payload,
    verify_signature,
};
