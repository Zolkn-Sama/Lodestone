use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Documents::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(Documents::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(Documents::OrgId).uuid().not_null())
                    .col(ColumnDef::new(Documents::Title).string().not_null())
                    .col(ColumnDef::new(Documents::SourceType).string().not_null())
                    .col(
                        ColumnDef::new(Documents::Status)
                            .string()
                            .not_null()
                            .default("pending"),
                    )
                    .col(ColumnDef::new(Documents::UploadedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(Documents::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_documents_org")
                            .from(Documents::Table, Documents::OrgId)
                            .to(Organizations::Table, Organizations::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_documents_uploaded_by")
                            .from(Documents::Table, Documents::UploadedBy)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_documents_org")
                    .table(Documents::Table)
                    .col(Documents::OrgId)
                    .to_owned(),
            )
            .await?;

        // Le type vector(768) et l'index HNSW passent en SQL brut : pgvector
        // n'est pas couvert par le builder sans feature supplémentaire.
        // 768 = dimension de nomic-embed-text.
        let db = manager.get_connection();
        db.execute_unprepared("CREATE EXTENSION IF NOT EXISTS vector")
            .await?;
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS document_chunks (
                id          UUID PRIMARY KEY,
                document_id UUID NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
                org_id      UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
                chunk_index INTEGER NOT NULL,
                content     TEXT NOT NULL,
                embedding   vector(768) NOT NULL,
                UNIQUE (document_id, chunk_index)
            )",
        )
        .await?;
        // Recherche par similarité cosinus (RAG-01).
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_document_chunks_embedding
                ON document_chunks USING hnsw (embedding vector_cosine_ops)",
        )
        .await?;
        // Isolation tenant : toute recherche est filtrée par org_id.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_document_chunks_org ON document_chunks (org_id)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // L'extension `vector` est laissée en place : d'autres objets peuvent en dépendre.
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS document_chunks")
            .await?;
        manager
            .drop_table(Table::drop().table(Documents::Table).to_owned())
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Documents {
    Table,
    Id,
    OrgId,
    Title,
    SourceType,
    Status,
    UploadedBy,
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
