use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE subscriptions
                    -- Périodicité du prix en cours : month | year.
                    ADD COLUMN IF NOT EXISTS billing_interval TEXT
                        CHECK (billing_interval IN ('month', 'year')),
                    -- Fin de la période en cours (horodatage Unix, fourni par Stripe).
                    ADD COLUMN IF NOT EXISTS current_period_end BIGINT,
                    -- Bascule programmée à l'échéance (annuel -> mensuel).
                    ADD COLUMN IF NOT EXISTS scheduled_interval TEXT
                        CHECK (scheduled_interval IN ('month', 'year'))",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE subscriptions
                    DROP COLUMN IF EXISTS billing_interval,
                    DROP COLUMN IF EXISTS current_period_end,
                    DROP COLUMN IF EXISTS scheduled_interval",
            )
            .await?;
        Ok(())
    }
}
