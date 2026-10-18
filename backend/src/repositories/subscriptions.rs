use sea_orm::{ConnectionTrait, DbErr, Statement};
use uuid::Uuid;

pub struct Subscription {
    pub org_id: Uuid,
    pub stripe_subscription_id: String,
    pub status: String,
    pub seats: Option<i32>,
    pub last_event_at: i64,
    pub stripe_item_id: Option<String>,
}

fn from_row(row: sea_orm::QueryResult) -> Result<Subscription, DbErr> {
    Ok(Subscription {
        org_id: row.try_get("", "org_id")?,
        stripe_subscription_id: row.try_get("", "stripe_subscription_id")?,
        status: row.try_get("", "status")?,
        seats: row.try_get("", "seats")?,
        last_event_at: row.try_get("", "last_event_at")?,
        stripe_item_id: row.try_get("", "stripe_item_id")?,
    })
}

const COLUMNS: &str =
    "org_id, stripe_subscription_id, status, seats, last_event_at, stripe_item_id";

/// L'abonnement de l'org, verrouillé jusqu'à la fin de la transaction.
pub async fn find_by_org_for_update<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<Option<Subscription>, DbErr> {
    conn.query_one(Statement::from_sql_and_values(
        conn.get_database_backend(),
        format!("SELECT {COLUMNS} FROM subscriptions WHERE org_id = $1 FOR UPDATE"),
        [org_id.into()],
    ))
    .await?
    .map(from_row)
    .transpose()
}

pub async fn find_by_org<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
) -> Result<Option<Subscription>, DbErr> {
    conn.query_one(Statement::from_sql_and_values(
        conn.get_database_backend(),
        format!("SELECT {COLUMNS} FROM subscriptions WHERE org_id = $1"),
        [org_id.into()],
    ))
    .await?
    .map(from_row)
    .transpose()
}

/// L'org d'un abonnement Stripe déjà connu.
pub async fn org_of_subscription<C: ConnectionTrait>(
    conn: &C,
    stripe_subscription_id: &str,
) -> Result<Option<Uuid>, DbErr> {
    match conn
        .query_one(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "SELECT org_id FROM subscriptions WHERE stripe_subscription_id = $1",
            [stripe_subscription_id.into()],
        ))
        .await?
    {
        Some(row) => Ok(Some(row.try_get("", "org_id")?)),
        None => Ok(None),
    }
}

pub struct SubscriptionUpdate<'a> {
    pub org_id: Uuid,
    pub stripe_subscription_id: &'a str,
    pub stripe_customer_id: Option<&'a str>,
    pub status: &'a str,
    pub seats: Option<i32>,
    pub item_id: Option<&'a str>,
    pub event_at: i64,
}

/// Crée ou remplace l'abonnement de l'org. Le client Stripe et les sièges
/// déjà connus sont gardés si l'événement ne les précise pas.
pub async fn upsert<C: ConnectionTrait>(conn: &C, u: SubscriptionUpdate<'_>) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "INSERT INTO subscriptions
            (org_id, stripe_subscription_id, stripe_customer_id, status, seats, last_event_at, stripe_item_id)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT (org_id) DO UPDATE SET
            stripe_subscription_id = EXCLUDED.stripe_subscription_id,
            stripe_customer_id = COALESCE(EXCLUDED.stripe_customer_id, subscriptions.stripe_customer_id),
            status = EXCLUDED.status,
            seats = COALESCE(EXCLUDED.seats, subscriptions.seats),
            last_event_at = EXCLUDED.last_event_at,
            stripe_item_id = COALESCE(EXCLUDED.stripe_item_id, subscriptions.stripe_item_id),
            updated_at = now()",
        [
            u.org_id.into(),
            u.stripe_subscription_id.into(),
            u.stripe_customer_id.map(str::to_string).into(),
            u.status.into(),
            u.seats.into(),
            u.event_at.into(),
            u.item_id.map(str::to_string).into(),
        ],
    ))
    .await?;
    Ok(())
}

/// Enregistre un événement Stripe. `false` s'il avait déjà été traité.
pub async fn record_event<C: ConnectionTrait>(
    conn: &C,
    event_id: &str,
    event_type: &str,
) -> Result<bool, DbErr> {
    let res = conn
        .execute(Statement::from_sql_and_values(
            conn.get_database_backend(),
            "INSERT INTO billing_events (event_id, event_type) VALUES ($1, $2)
             ON CONFLICT (event_id) DO NOTHING",
            [event_id.into(), event_type.into()],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Enregistre le nombre de sièges envoyé au fournisseur.
pub async fn set_seats<C: ConnectionTrait>(
    conn: &C,
    org_id: Uuid,
    seats: i32,
) -> Result<(), DbErr> {
    conn.execute(Statement::from_sql_and_values(
        conn.get_database_backend(),
        "UPDATE subscriptions SET seats = $1, updated_at = now() WHERE org_id = $2",
        [seats.into(), org_id.into()],
    ))
    .await?;
    Ok(())
}
