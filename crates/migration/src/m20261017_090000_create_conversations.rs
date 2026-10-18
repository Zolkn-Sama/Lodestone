use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Une conversation appartient à UN utilisateur, dans UNE org.
        manager
            .create_table(
                Table::create()
                    .table(Conversations::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Conversations::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Conversations::OrgId).uuid().not_null())
                    .col(ColumnDef::new(Conversations::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(Conversations::Title)
                            .string()
                            .not_null()
                            .default("Nouvelle conversation"),
                    )
                    .col(
                        ColumnDef::new(Conversations::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_conversations_org")
                            .from(Conversations::Table, Conversations::OrgId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_conversations_user")
                            .from(Conversations::Table, Conversations::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Historique (CHAT-02) : « mes conversations dans cette org ».
        manager
            .create_index(
                Index::create()
                    .name("idx_conversations_org_user")
                    .table(Conversations::Table)
                    .col(Conversations::OrgId)
                    .col(Conversations::UserId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_table(
                Table::create()
                    .table(Messages::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Messages::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Messages::ConversationId).uuid().not_null())
                    // user | assistant
                    .col(ColumnDef::new(Messages::Role).string().not_null())
                    .col(ColumnDef::new(Messages::Content).text().not_null())
                    .col(
                        ColumnDef::new(Messages::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_messages_conversation")
                            .from(Messages::Table, Messages::ConversationId)
                            .to(Conversations::Table, Conversations::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_messages_conversation")
                    .table(Messages::Table)
                    .col(Messages::ConversationId)
                    .col(Messages::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Sources citées par un message de l'assistant (traçabilité).
        manager
            .create_table(
                Table::create()
                    .table(MessageSources::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MessageSources::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MessageSources::MessageId).uuid().not_null())
                    .col(ColumnDef::new(MessageSources::ChunkId).uuid().not_null())
                    .col(ColumnDef::new(MessageSources::Score).float().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_sources_message")
                            .from(MessageSources::Table, MessageSources::MessageId)
                            .to(Messages::Table, Messages::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_sources_chunk")
                            .from(MessageSources::Table, MessageSources::ChunkId)
                            .to(DocumentChunks::Table, DocumentChunks::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_sources_message")
                    .table(MessageSources::Table)
                    .col(MessageSources::MessageId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MessageSources::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Messages::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Conversations::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Conversations {
    Table,
    Id,
    OrgId,
    UserId,
    Title,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Messages {
    Table,
    Id,
    ConversationId,
    Role,
    Content,
    CreatedAt,
}

#[derive(DeriveIden)]
enum MessageSources {
    Table,
    Id,
    MessageId,
    ChunkId,
    Score,
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

#[derive(DeriveIden)]
enum DocumentChunks {
    Table,
    Id,
}
