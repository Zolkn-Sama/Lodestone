use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Ligne d'abonnement Stripe (`si_…`) : sa quantité = nombre de sièges.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE subscriptions ADD COLUMN IF NOT EXISTS stripe_item_id TEXT",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE subscriptions DROP COLUMN IF EXISTS stripe_item_id")
            .await?;
        Ok(())
    }
}
