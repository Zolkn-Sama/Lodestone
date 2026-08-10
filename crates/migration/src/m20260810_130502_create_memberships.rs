use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Memberships::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Memberships::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Memberships::UserId).uuid().not_null())
                    .col(ColumnDef::new(Memberships::OrgId).uuid().not_null())
                    .col(ColumnDef::new(Memberships::Role).string().not_null())
                    .col(
                        ColumnDef::new(Memberships::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memberships_user")
                            .from(Memberships::Table, Memberships::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_memberships_org")
                            .from(Memberships::Table, Memberships::OrgId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Un utilisateur ne peut avoir qu'UNE appartenance par org (pas de doublon).
        manager
            .create_index(
                Index::create()
                    .name("idx_memberships_user_org_unique")
                    .table(Memberships::Table)
                    .col(Memberships::UserId)
                    .col(Memberships::OrgId)
                    .unique()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(Memberships::Table).to_owned()).await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Memberships {
    Table, Id, UserId, OrgId, Role, CreatedAt,
}
#[derive(DeriveIden)]
enum Users { Table, Id }
#[derive(DeriveIden)]
enum Organizations { Table, Id }