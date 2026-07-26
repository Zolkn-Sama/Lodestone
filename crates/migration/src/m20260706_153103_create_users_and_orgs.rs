use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // --- Table organizations ---
        manager
            .create_table(
                Table::create()
                    .table(Organizations::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Organizations::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Organizations::Name).string().not_null())
                    .col(
                        ColumnDef::new(Organizations::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // --- Table users ---
        manager
            .create_table(
                Table::create()
                    .table(Users::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Users::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Users::Email).string().not_null())
                    .col(ColumnDef::new(Users::PasswordHash).string().not_null())
                    .col(
                        ColumnDef::new(Users::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        // --- Index unique sur users.email ---
        manager
            .create_index(
                Index::create()
                    .name("idx_users_email_unique")
                    .table(Users::Table)
                    .col(Users::Email)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Ordre inverse de up() : users d'abord (aucune dépendance ici, mais on garde le réflexe).
        manager.drop_table(Table::drop().table(Users::Table).to_owned()).await?;
        manager.drop_table(Table::drop().table(Organizations::Table).to_owned()).await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Organizations {
    Table,
    Id,
    Name,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
    Email,
    PasswordHash,
    CreatedAt,
}