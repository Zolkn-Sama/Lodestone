pub use sea_orm_migration::prelude::*;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20260706_153103_create_users_and_orgs::Migration),
            Box::new(m20260809_131736_create_refresh_tokens::Migration)
        ]
    }
}

mod m20260706_153103_create_users_and_orgs;
mod m20260809_131736_create_refresh_tokens;