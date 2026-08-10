use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(RefreshTokens::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(RefreshTokens::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(RefreshTokens::UserId).uuid().not_null())
                    .col(ColumnDef::new(RefreshTokens::TokenHash).string().not_null())
                    .col(ColumnDef::new(RefreshTokens::ExpiresAt).timestamp_with_time_zone().not_null())
                    .col(ColumnDef::new(RefreshTokens::RevokedAt).timestamp_with_time_zone().null())
                    .col(
                        ColumnDef::new(RefreshTokens::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_refresh_tokens_user")
                            .from(RefreshTokens::Table, RefreshTokens::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade), // user supprimé -> ses tokens aussi
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_refresh_tokens_hash")
                    .table(RefreshTokens::Table)
                    .col(RefreshTokens::TokenHash) // recherche rapide au refresh
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(RefreshTokens::Table).to_owned()).await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum RefreshTokens {
    Table, Id, UserId, TokenHash, ExpiresAt, RevokedAt, CreatedAt,
}

// Référence minimale pour la clé étrangère.
#[derive(DeriveIden)]
enum Users { Table, Id }