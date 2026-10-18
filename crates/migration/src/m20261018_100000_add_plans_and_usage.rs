use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Plan de l'org. Les orgs existantes passent en `free`.
        // La contrainte CHECK refuse toute valeur hors des plans connus.
        db.execute_unprepared(
            "ALTER TABLE organizations
                ADD COLUMN IF NOT EXISTS plan TEXT NOT NULL DEFAULT 'free'
                CONSTRAINT organizations_plan_check CHECK (plan IN ('free', 'pro', 'sovereign'))",
        )
        .await?;

        // Compteur de questions par org et par mois (1er jour du mois, UTC).
        // Le mois fait partie de la clé : la remise à zéro est naturelle, et
        // supprimer des conversations ne « rend » pas de questions.
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS org_usage (
                org_id    UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                period    DATE NOT NULL,
                questions INTEGER NOT NULL DEFAULT 0 CHECK (questions >= 0),
                PRIMARY KEY (org_id, period)
            )",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS org_usage")
            .await?;
        db.execute_unprepared("ALTER TABLE organizations DROP COLUMN IF EXISTS plan")
            .await?;
        Ok(())
    }
}
