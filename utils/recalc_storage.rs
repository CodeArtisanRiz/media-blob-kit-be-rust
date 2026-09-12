use sea_orm::{Database, Statement, ConnectionTrait};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let db_url = std::env::var("DATABASE_URL").expect("DATABASE_URL not set");
    let db = Database::connect(&db_url).await?;

    let sql = r#"
        WITH file_totals AS (
            SELECT project_id, SUM(size) as total_size
            FROM files
            GROUP BY project_id
        )
        UPDATE projects p
        SET storage_used_bytes = COALESCE(ft.total_size, 0)
        FROM file_totals ft
        WHERE p.id = ft.project_id;
    "#;

    let stmt = Statement::from_string(db.get_database_backend(), sql.to_string());
    let res = db.execute(stmt).await?;
    println!("Updated storage quotas for {} projects based on existing originals.", res.rows_affected());
    
    Ok(())
}
