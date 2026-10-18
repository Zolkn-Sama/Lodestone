use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Supprimer un compte (RGPD) ne doit pas effacer les documents qu'il a
        // apportés à une org, ni les invitations qu'il a envoyées : ils
        // appartiennent à l'org. Seul l'auteur est effacé (NULL).
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE documents
                DROP CONSTRAINT IF EXISTS fk_documents_uploaded_by,
                ALTER COLUMN uploaded_by DROP NOT NULL,
                ADD CONSTRAINT fk_documents_uploaded_by FOREIGN KEY (uploaded_by)
                    REFERENCES users(id) ON DELETE SET NULL",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE invitations
                DROP CONSTRAINT IF EXISTS fk_invitations_invited_by,
                ALTER COLUMN invited_by DROP NOT NULL,
                ADD CONSTRAINT fk_invitations_invited_by FOREIGN KEY (invited_by)
                    REFERENCES users(id) ON DELETE SET NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Les lignes dont l'auteur a été supprimé ne peuvent pas retrouver un
        // NOT NULL : elles sont retirées avant de rétablir l'ancienne contrainte.
        let db = manager.get_connection();
        db.execute_unprepared(
            "DELETE FROM documents WHERE uploaded_by IS NULL;
             ALTER TABLE documents
                DROP CONSTRAINT IF EXISTS fk_documents_uploaded_by,
                ALTER COLUMN uploaded_by SET NOT NULL,
                ADD CONSTRAINT fk_documents_uploaded_by FOREIGN KEY (uploaded_by)
                    REFERENCES users(id) ON DELETE CASCADE",
        )
        .await?;
        db.execute_unprepared(
            "DELETE FROM invitations WHERE invited_by IS NULL;
             ALTER TABLE invitations
                DROP CONSTRAINT IF EXISTS fk_invitations_invited_by,
                ALTER COLUMN invited_by SET NOT NULL,
                ADD CONSTRAINT fk_invitations_invited_by FOREIGN KEY (invited_by)
                    REFERENCES users(id) ON DELETE CASCADE",
        )
        .await?;
        Ok(())
    }
}
