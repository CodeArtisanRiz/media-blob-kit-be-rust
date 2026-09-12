use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Jobs::Table)
                    .add_column(
                        ColumnDef::new(Jobs::AttemptCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .add_column(
                        ColumnDef::new(Jobs::MaxRetries)
                            .integer()
                            .not_null()
                            .default(3),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Jobs::Table)
                    .drop_column(Jobs::AttemptCount)
                    .drop_column(Jobs::MaxRetries)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Jobs {
    Table,
    AttemptCount,
    MaxRetries,
}
