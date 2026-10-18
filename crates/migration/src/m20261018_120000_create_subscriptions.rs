use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Un abonnement (le plus récent) par org.
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS subscriptions (
                org_id                 UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
                stripe_subscription_id TEXT NOT NULL UNIQUE,
                stripe_customer_id     TEXT,
                status                 TEXT NOT NULL,
                seats                  INTEGER CHECK (seats >= 0),
                -- Horodatage Stripe du dernier événement appliqué : un événement
                -- plus ancien, arrivé en retard, est ignoré.
                last_event_at          BIGINT NOT NULL,
                updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
        )
        .await?;

        // Événements déjà traités : Stripe peut renvoyer le même événement.
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS billing_events (
                event_id    TEXT PRIMARY KEY,
                event_type  TEXT NOT NULL,
                received_at TIMESTAMPTZ NOT NULL DEFAULT now()
            )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS billing_events")
            .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS subscriptions")
            .await?;
        Ok(())
    }
}
