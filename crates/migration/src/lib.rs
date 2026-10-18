pub use sea_orm_migration::prelude::*;

mod m20260706_153103_create_users_and_orgs;
mod m20260809_131736_create_refresh_tokens;
mod m20260810_130502_create_memberships; // <-- adapte au nom réel du fichier généré
mod m20261016_120000_create_invitations;
mod m20261016_140000_create_documents_and_chunks;
mod m20261017_090000_create_conversations;
mod m20261018_100000_add_plans_and_usage;
mod m20261018_120000_create_subscriptions;
mod m20261018_140000_add_subscription_item;
mod m20261018_160000_add_billing_interval;
mod m20261018_180000_add_sovereign_billing;
mod m20261018_200000_add_document_embedding_model;
mod m20261019_090000_create_password_resets;
mod m20261019_120000_keep_org_data_on_user_deletion;

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
            Box::new(m20261018_100000_add_plans_and_usage::Migration),
            Box::new(m20261018_120000_create_subscriptions::Migration),
            Box::new(m20261018_140000_add_subscription_item::Migration),
            Box::new(m20261018_160000_add_billing_interval::Migration),
            Box::new(m20261018_180000_add_sovereign_billing::Migration),
            Box::new(m20261018_200000_add_document_embedding_model::Migration),
            Box::new(m20261019_090000_create_password_resets::Migration),
            Box::new(m20261019_120000_keep_org_data_on_user_deletion::Migration),
        ]
    }
}
