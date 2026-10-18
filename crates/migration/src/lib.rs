pub use sea_orm_migration::prelude::*;

mod m20260706_153103_create_users_and_orgs;
mod m20260809_131736_create_refresh_tokens;
mod m20260810_130502_create_memberships; // <-- adapte au nom réel du fichier généré
mod m20261016_120000_create_invitations;
mod m20261016_140000_create_documents_and_chunks;
mod m20261017_090000_create_conversations;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260706_153103_create_users_and_orgs::Migration),
            Box::new(m20260809_131736_create_refresh_tokens::Migration),
            Box::new(m20260810_130502_create_memberships::Migration),
            Box::new(m20261016_120000_create_invitations::Migration),
            Box::new(m20261016_140000_create_documents_and_chunks::Migration),
            Box::new(m20261017_090000_create_conversations::Migration),
        ]
    }
}
