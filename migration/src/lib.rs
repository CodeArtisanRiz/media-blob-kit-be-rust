pub use sea_orm_migration::prelude::*;

mod m20240101_000001_create_user_table;
mod m20241201_000002_create_refresh_tokens_table;
mod m20241202_000003_create_projects_table;
mod m20241202_000004_create_api_keys_table;
mod m20241204_000005_create_files_table;
mod m20241204_000006_create_jobs_table;
mod m20260912_000007_add_quota_to_projects;
mod m20260912_000008_add_content_hash_to_files;
mod m20260912_000009_add_retry_fields_to_jobs;

pub struct Migrator;

#[async_trait::async_trait]
impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(m20240101_000001_create_user_table::Migration),
            Box::new(m20241201_000002_create_refresh_tokens_table::Migration),
            Box::new(m20241202_000003_create_projects_table::Migration),
            Box::new(m20241202_000004_create_api_keys_table::Migration),
            Box::new(m20241204_000005_create_files_table::Migration),
            Box::new(m20241204_000006_create_jobs_table::Migration),
            Box::new(m20260912_000007_add_quota_to_projects::Migration),
            Box::new(m20260912_000008_add_content_hash_to_files::Migration),
            Box::new(m20260912_000009_add_retry_fields_to_jobs::Migration),
        ]
    }
}
