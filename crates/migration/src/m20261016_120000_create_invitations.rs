use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Invitations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Invitations::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Invitations::OrgId).uuid().not_null())
                    // Stocké en minuscules par le service (pas de CITEXT).
                    .col(ColumnDef::new(Invitations::Email).string().not_null())
                    .col(
                        ColumnDef::new(Invitations::Role)
                            .string()
                            .not_null()
                            .default("member"),
                    )
                    .col(ColumnDef::new(Invitations::TokenHash).string().not_null())
                    .col(ColumnDef::new(Invitations::InvitedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(Invitations::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Invitations::AcceptedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Invitations::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_invitations_org")
                            .from(Invitations::Table, Invitations::OrgId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Cascade), // org supprimée -> invitations aussi
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_invitations_invited_by")
                            .from(Invitations::Table, Invitations::InvitedBy)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Le lien d'invitation est retrouvé par le hash de son jeton.
        manager
            .create_index(
                Index::create()
                    .name("idx_invitations_token_hash_unique")
                    .table(Invitations::Table)
                    .col(Invitations::TokenHash)
                    .unique()
                    .to_owned(),
            )
            .await?;

        // Ré-inviter quelqu'un = chercher ses invitations en attente dans l'org.
        manager
            .create_index(
                Index::create()
                    .name("idx_invitations_org_email")
                    .table(Invitations::Table)
                    .col(Invitations::OrgId)
                    .col(Invitations::Email)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Invitations::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Invitations {
    Table,
    Id,
    OrgId,
    Email,
    Role,
    TokenHash,
    InvitedBy,
    ExpiresAt,
    AcceptedAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Organizations {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}
