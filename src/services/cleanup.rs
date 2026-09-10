use sea_orm::{DatabaseConnection, EntityTrait, QueryFilter, ColumnTrait};
use crate::entities::{project, file};
use crate::services::s3::S3Service;
use std::time::Duration;
use chrono::Utc;

pub struct CleanupService {
    db: DatabaseConnection,
}

impl CleanupService {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    pub async fn run_scheduler(self) {
        println!("Cleanup Scheduler | Started");
        let mut interval = tokio::time::interval(Duration::from_secs(86400)); // Run once a day (start immediately first)
        
        // Skip first tick if we want to delay, but typically immediate start is okay or ticks immediately.
        // interval.tick().await; 

        loop {
            interval.tick().await;
            println!("Cleanup Scheduler | Running cleanups...");
            
            if let Err(e) = self.clean_soft_deleted_projects().await {
                eprintln!("Cleanup Scheduler | Error cleaning projects: {}", e);
            }
        }
    }

    async fn clean_soft_deleted_projects(&self) -> Result<(), Box<dyn std::error::Error>> {
        // Logic: Find projects deleted > 30 days ago
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
            
            // 1. Find Files
            let files = file::Entity::find()
                .filter(file::Column::ProjectId.eq(p.id))
                .all(&self.db)
                .await?;

            // 2. Delete S3 Objects
            for f in files {
                // Delete Original
                let _ = s3_service.delete_object(&f.s3_key).await;

                // Delete Variants
                if let Some(variants) = f.variants_json.as_object() {
                    for (_v_name, v_path) in variants {
                        if let Some(v_str) = v_path.as_str() {
                            let k = crate::utils::extract_s3_key(v_str);
                            let _ = s3_service.delete_object(&k).await;
                        }
                    }
                }
            }

            // 3. Delete Project from DB
            project::Entity::delete_by_id(p.id).exec(&self.db).await?;
        }

        Ok(())
    }
}
