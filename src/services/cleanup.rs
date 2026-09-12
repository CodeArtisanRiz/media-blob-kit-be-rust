use sea_orm::{DatabaseConnection, EntityTrait, QueryFilter, ColumnTrait};
use sea_orm::sea_query::Expr;
use crate::entities::{project, file, refresh_token};
use crate::services::s3::S3Service;
use std::time::Duration;
use chrono::{Utc, Datelike};

pub struct CleanupService {
    db: DatabaseConnection,
}

impl CleanupService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn run_scheduler(self) {
        println!("Cleanup Scheduler | Started");
        let mut interval = tokio::time::interval(Duration::from_secs(86400)); // Run once a day

        loop {
            interval.tick().await;
            println!("Cleanup Scheduler | Running cleanups...");
            
            if let Err(e) = self.clean_soft_deleted_projects().await {
                eprintln!("Cleanup Scheduler | Error cleaning projects: {}", e);
            }

            if let Err(e) = self.clean_expired_refresh_tokens().await {
                eprintln!("Cleanup Scheduler | Error cleaning refresh tokens: {}", e);
            }

            // On the 1st of the month, reset monthly transforms used
            if Utc::now().day() == 1 {
                if let Err(e) = self.reset_monthly_transforms().await {
                    eprintln!("Cleanup Scheduler | Error resetting monthly transforms: {}", e);
                }
            }
        }
    }

    async fn clean_soft_deleted_projects(&self) -> Result<(), Box<dyn std::error::Error>> {
        let threshold = Utc::now().naive_utc() - chrono::Duration::days(30);

        let projects_to_delete = project::Entity::find()
            .filter(project::Column::DeletedAt.is_not_null())
            .filter(project::Column::DeletedAt.lt(threshold))
            .all(&self.db)
            .await?;

        if projects_to_delete.is_empty() {
             return Ok(());
        }

        println!("Cleanup Scheduler | Found {} projects to hard delete", projects_to_delete.len());

        let s3_service = S3Service::new().await;

        for p in projects_to_delete {
            println!("Cleanup Scheduler | Hard deleting project: {} ({})", p.name, p.id);
            
            let files = file::Entity::find()
                .filter(file::Column::ProjectId.eq(p.id))
                .all(&self.db)
                .await?;

            for f in files {
                let _ = s3_service.delete_object(&f.s3_key).await;

                if let Some(variants) = f.variants_json.as_object() {
                    for (_v_name, v_path) in variants {
                        if let Some(v_str) = v_path.as_str() {
                            let k = crate::utils::extract_s3_key(v_str);
                            let _ = s3_service.delete_object(&k).await;
                        }
                    }
                }
            }

            project::Entity::delete_by_id(p.id).exec(&self.db).await?;
        }

        Ok(())
    }

    async fn clean_expired_refresh_tokens(&self) -> Result<(), Box<dyn std::error::Error>> {
        let threshold = Utc::now().naive_utc() - chrono::Duration::days(7);

        let res = refresh_token::Entity::delete_many()
            .filter(
                refresh_token::Column::Revoked.eq(true)
                    .or(refresh_token::Column::ExpiresAt.lt(threshold))
            )
            .exec(&self.db)
            .await?;

        if res.rows_affected > 0 {
            println!("Cleanup Scheduler | Pruned {} expired/revoked refresh tokens", res.rows_affected);
        }

        Ok(())
    }

    async fn reset_monthly_transforms(&self) -> Result<(), Box<dyn std::error::Error>> {
        let res = project::Entity::update_many()
            .col_expr(project::Column::TransformsUsed, Expr::value(0i64))
            .exec(&self.db)
            .await?;

        println!("Cleanup Scheduler | Monthly reset of transforms_used applied to {} projects", res.rows_affected);
        Ok(())
    }
}
