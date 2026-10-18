use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        // Mise en service Sovereign payée : jamais refacturée ensuite.
        db.execute_unprepared(
            "ALTER TABLE organizations ADD COLUMN IF NOT EXISTS sovereign_setup_paid_at TIMESTAMPTZ",
        )
        .await?;
        // Produit de l'abonnement : les abonnements Sovereign ne touchent ni au
        // plan, ni aux sièges, ni à la périodicité.
        db.execute_unprepared(
            "ALTER TABLE subscriptions ADD COLUMN IF NOT EXISTS product TEXT NOT NULL DEFAULT 'pro'
                CHECK (product IN ('pro', 'sovereign'))",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE subscriptions DROP COLUMN IF EXISTS product")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE organizations DROP COLUMN IF EXISTS sovereign_setup_paid_at",
        )
        .await?;
        Ok(())
    }
}
