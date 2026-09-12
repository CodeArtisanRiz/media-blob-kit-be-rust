use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Add storage and transform quota columns to projects table
        manager
            .alter_table(
                Table::alter()
                    .table(Project::Projects)
                    .add_column(
                        ColumnDef::new(Project::StorageUsedBytes)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .add_column(
                        ColumnDef::new(Project::StorageLimitBytes)
                            .big_integer()
                            .not_null()
                            .default(5368709120i64), // 5 GB
                    )
                    .add_column(
                        ColumnDef::new(Project::TransformsUsed)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .add_column(
                        ColumnDef::new(Project::TransformsLimit)
                            .big_integer()
                            .not_null()
                            .default(10000i64), // 10,000 transforms/month
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Project::Projects)
                    .drop_column(Project::StorageUsedBytes)
                    .drop_column(Project::StorageLimitBytes)
                    .drop_column(Project::TransformsUsed)
                    .drop_column(Project::TransformsLimit)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Project {
    Projects,
    StorageUsedBytes,
    StorageLimitBytes,
    TransformsUsed,
    TransformsLimit,
}
