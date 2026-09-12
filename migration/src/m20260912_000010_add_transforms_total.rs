use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Project::Projects)
                    .add_column(
                        ColumnDef::new(Project::TransformsTotal)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;

        // Initialize total with current monthly uses
        let update_stmt = sea_orm::Statement::from_string(
            manager.get_database_backend(),
            "UPDATE projects SET transforms_total = transforms_used".to_string(),
        );
        manager.get_connection().execute(update_stmt).await?;
        
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Project::Projects)
                    .drop_column(Project::TransformsTotal)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Project {
    Projects,
    TransformsTotal,
}
