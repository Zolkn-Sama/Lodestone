use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Jetons de réinitialisation de mot de passe : seul le hash est stocké,
        // usage unique, durée de vie courte.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS password_resets (
                    id         UUID PRIMARY KEY,
                    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
                    token_hash TEXT NOT NULL UNIQUE,
                    expires_at TIMESTAMPTZ NOT NULL,
                    used_at    TIMESTAMPTZ,
                    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
                )",
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS idx_password_resets_user ON password_resets (user_id)",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS password_resets")
            .await?;
        Ok(())
    }
}
