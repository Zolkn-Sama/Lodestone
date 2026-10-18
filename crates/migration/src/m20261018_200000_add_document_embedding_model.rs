use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Modèle d'embedding qui a produit les vecteurs du document : des
        // vecteurs de modèles différents ne sont pas comparables.
        // NULL pour les documents ingérés avant ce suivi.
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE documents ADD COLUMN IF NOT EXISTS embedding_model TEXT",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE documents DROP COLUMN IF EXISTS embedding_model")
            .await?;
        Ok(())
    }
}
