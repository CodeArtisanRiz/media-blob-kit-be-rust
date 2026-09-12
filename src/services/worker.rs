use std::time::Duration;
use std::sync::Arc;
use tokio::sync::{Semaphore, OwnedSemaphorePermit};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, 
    QueryOrder, QuerySelect, Set, TransactionTrait
};
use sea_orm::sea_query::{LockType, LockBehavior, Expr};
use tokio::time::sleep;
use crate::entities::{job, file, project};
use crate::services::s3::S3Service;
use crate::utils::{image_processor, sanitize_bucket_name};
use crate::models::settings::VariantConfig;
use std::collections::HashMap;
use uuid::Uuid;

use crate::services::broadcaster::Broadcaster;
use crate::routes::jobs::JobResponse;

#[derive(Clone)]
pub struct Worker {
    db: DatabaseConnection,
    s3: S3Service,
    semaphore: Arc<Semaphore>,
    broadcaster: Broadcaster,
}

impl Worker {
    pub async fn new(db: DatabaseConnection, broadcaster: Broadcaster) -> Self {
        let s3 = S3Service::new().await;
        let config = crate::config::get_config();
        let semaphore = Arc::new(Semaphore::new(config.worker_concurrency));
        Self { db, s3, semaphore, broadcaster }
    }

    pub async fn run(&self) {
        println!("Worker started with concurrency: {}", crate::config::get_config().worker_concurrency);
        
        // Recover any jobs stuck in 'processing' state from previous runs
        if let Err(e) = self.recover_stuck_jobs().await {
            eprintln!("Failed to recover stuck jobs: {}", e);
        }

        loop {
            // Acquire permit before looking for work
            let permit = match self.semaphore.clone().acquire_owned().await {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("Semaphore error: {}", e);
                    break;
                }
            };

            match self.claim_next_job().await {
                Ok(Some(job_model)) => {
                    let worker = self.clone();
                    tokio::spawn(async move {
                        worker.perform_job(job_model, permit).await;
                    });
                }
                Ok(None) => {
                    // No jobs found, drop permit and sleep
                    drop(permit);
                    sleep(Duration::from_secs(5)).await;
                }
                Err(e) => {
                    eprintln!("Worker error: {}", e);
                    drop(permit);
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn recover_stuck_jobs(&self) -> Result<(), String> {
        job::Entity::update_many()
            .col_expr(job::Column::Status, Expr::value("pending"))
            .filter(job::Column::Status.eq("processing"))
            .exec(&self.db)
            .await
            .map_err(|e| e.to_string())?;

        Ok(())
    }

    async fn claim_next_job(&self) -> Result<Option<job::Model>, String> {
        // Start transaction
        let txn = self.db.begin().await.map_err(|e| e.to_string())?;

        // 1. Find pending job with lock and respect backoff (updated_at <= now)
        let now = chrono::Utc::now().naive_utc();
        let job_opt = job::Entity::find()
            .filter(job::Column::Status.eq("pending"))
            .filter(job::Column::UpdatedAt.lte(now))
            .order_by_asc(job::Column::CreatedAt)
            .limit(1)
            .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
            .one(&txn)
            .await
            .map_err(|e| e.to_string())?;

        let job_model = match job_opt {
            Some(j) => j,
            None => return Ok(None), // No jobs
        };

        println!("Worker picked up job {}", job_model.id);

        // Update job status to processing
        let mut job_active: job::ActiveModel = job_model.clone().into();
        job_active.status = Set("processing".to_string());
        job_active.updated_at = Set(chrono::Utc::now().naive_utc());
        let job_model = job_active.update(&txn).await.map_err(|e| e.to_string())?;

        // Commit transaction to release lock and save 'processing' state
        txn.commit().await.map_err(|e| e.to_string())?;

        // Broadcast processing status
        self.broadcaster.send(JobResponse::from(job_model.clone()));

        Ok(Some(job_model))
    }

    async fn perform_job(&self, job_model: job::Model, _permit: OwnedSemaphorePermit) {
        let job_start_time = std::time::Instant::now();
        
        match self.handle_job(&job_model).await {
            Ok(_) => {
                let duration = job_start_time.elapsed();
                println!("Job {} completed successfully took {:.2?}", job_model.id, duration);
                let mut job_active: job::ActiveModel = job_model.into();
                job_active.status = Set("completed".to_string());
                job_active.updated_at = Set(chrono::Utc::now().naive_utc());
                if let Ok(updated) = job_active.update(&self.db).await {
                    self.broadcaster.send(JobResponse::from(updated));
                }
            },
            Err(e) => {
                eprintln!("Job {} failed: {}", job_model.id, e);
                let payload = job_model.payload.clone();
                let current_attempts = job_model.attempt_count + 1;
                
                let mut job_active: job::ActiveModel = job_model.into();
                job_active.attempt_count = Set(current_attempts);

                if current_attempts < job_active.max_retries.as_ref().clone() {
                    // Retry with exponential backoff: 30s * attempt_count
                    let backoff_secs = 30 * current_attempts as i64;
                    let next_try = chrono::Utc::now().naive_utc() + chrono::Duration::seconds(backoff_secs);
                    println!("Job {} will retry (attempt {}/{}) after {}s", job_active.id.as_ref(), current_attempts, job_active.max_retries.as_ref(), backoff_secs);
                    
                    job_active.status = Set("pending".to_string());
                    job_active.updated_at = Set(next_try);
                } else {
                    println!("Job {} permanently failed (reached max retries)", job_active.id.as_ref());
                    job_active.status = Set("failed".to_string());
                    job_active.payload = Set(serde_json::json!({
                        "error": e,
                        "original_payload": payload
                    }));
                    job_active.updated_at = Set(chrono::Utc::now().naive_utc());
                }

                if let Ok(updated) = job_active.update(&self.db).await {
                    self.broadcaster.send(JobResponse::from(updated));
                }
            }
        }
    }

    async fn handle_job(&self, job: &job::Model) -> Result<(), String> {
        let payload = job.payload.as_object().ok_or("Invalid payload")?;

        if let Some(job_type) = payload.get("type").and_then(|v| v.as_str()) {
            match job_type {
                "sync_project_variants" => self.handle_sync_project_variants(job).await,
                "sync_file_variants" => self.handle_sync_file_variants(job).await,
                _ => Err(format!("Unknown job type: {}", job_type)),
            }
        } else if payload.contains_key("variants") {
             self.handle_process_image(job).await
        } else {
             Err("Unknown job payload structure".to_string())
        }
    }

    async fn handle_sync_project_variants(&self, job: &job::Model) -> Result<(), String> {
        let payload = job.payload.as_object().unwrap();
        let project_id_str = payload.get("project_id").and_then(|v| v.as_str()).ok_or("Missing project_id")?;
        let project_id = Uuid::parse_str(project_id_str).map_err(|e| e.to_string())?;

        let project = project::Entity::find_by_id(project_id)
            .one(&self.db)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Project not found")?;

        let settings = project.settings.as_object().ok_or("Invalid project settings")?;
        let variants_json = settings.get("variants").cloned().unwrap_or(serde_json::json!({}));
        
        let files = file::Entity::find()
            .filter(file::Column::ProjectId.eq(project_id))
            .filter(file::Column::MimeType.contains("image"))
            .all(&self.db)
            .await
            .map_err(|e| e.to_string())?;

        println!("SyncProjectVariants: Found {} images for project {}", files.len(), project.name);

        for f in files {
            let job_payload = serde_json::json!({
                "type": "sync_file_variants",
                "file_id": f.id.to_string(),
                "variants_config": variants_json
            });

            let job = job::ActiveModel {
                id: Set(Uuid::new_v4()),
                file_id: Set(f.id),
                status: Set("pending".to_string()),
                payload: Set(job_payload),
                attempt_count: Set(0),
                max_retries: Set(3),
                created_at: Set(chrono::Utc::now().naive_utc()),
                updated_at: Set(chrono::Utc::now().naive_utc()),
            };

            job.insert(&self.db).await.map_err(|e| e.to_string())?;
        }

        Ok(())
    }

    async fn handle_sync_file_variants(&self, job: &job::Model) -> Result<(), String> {
        let payload = job.payload.as_object().unwrap();
        let variants_config_json = payload.get("variants_config").ok_or("Missing variants_config")?;
        let target_variants: HashMap<String, VariantConfig> = serde_json::from_value(variants_config_json.clone())
            .map_err(|e| e.to_string())?;

        let file = file::Entity::find_by_id(job.file_id)
            .one(&self.db)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("File not found")?;
        
        self.process_image_logic(&file, target_variants).await
    }

    async fn handle_process_image(&self, job: &job::Model) -> Result<(), String> {
         let payload = job.payload.as_object().ok_or("Invalid payload")?;
         let variants_json = payload.get("variants").ok_or("No variants in payload")?;
         let variants: HashMap<String, VariantConfig> = serde_json::from_value(variants_json.clone())
             .map_err(|e| e.to_string())?;
         
         let file = file::Entity::find_by_id(job.file_id)
            .one(&self.db)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("File not found")?;

         self.process_image_logic(&file, variants).await
    }

    async fn process_image_logic(&self, file: &file::Model, variants: HashMap<String, VariantConfig>) -> Result<(), String> {
        let project = project::Entity::find_by_id(file.project_id)
            .one(&self.db)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("Project not found")?;

        let settings: crate::models::settings::ProjectSettings = serde_json::from_value(project.settings.clone())
            .unwrap_or_default();

        // Download original file
        let original_data = self.s3.get_object(&file.s3_key).await.map_err(|e| e.to_string())?;

        // Keep track of old variant URLs to clean up stale ones
        let old_variants = file.variants_json.as_object().cloned().unwrap_or_default();

        let mut successful_variants = serde_json::Map::new();
        let mut total_variant_bytes: i64 = 0;

        // Process each target variant
        for (variant_name, config) in &variants {
            println!("Processing variant: {}", variant_name);
            
            let original_data_clone = original_data.clone();
            let config_clone = config.clone();

            let (processed_data, mime_type) = tokio::task::spawn_blocking(move || {
                image_processor::process_image(&original_data_clone, &config_clone)
            }).await
              .map_err(|e| format!("Task join error: {}", e))?
              .map_err(|e| e.to_string())?;

            let ext = match mime_type.as_str() {
                "image/avif" => "avif",
                "image/webp" => "webp",
                "image/png" => "png",
                "image/jpeg" => "jpg",
                _ => "bin",
            };

            let s3_key = format!("{}-{}/images/{}/{}.{}", 
                sanitize_bucket_name(&project.name), 
                project.id, 
                variant_name, 
                file.id, 
                ext
            );

            total_variant_bytes += processed_data.len() as i64;
            self.s3.put_object(&s3_key, processed_data, &mime_type).await.map_err(|e| e.to_string())?;
            
            let variant_url = crate::utils::build_s3_url(&s3_key);
            successful_variants.insert(variant_name.clone(), serde_json::Value::String(variant_url));
        }

        // Clean up stale variants that were in old variants_json but are no longer in target config
        for (old_key, old_val) in &old_variants {
            if !variants.contains_key(old_key) {
                if let Some(old_url) = old_val.as_str() {
                    let key = crate::utils::extract_s3_key(old_url);
                    println!("Cleaning up stale variant S3 object: {}", key);
                    let _ = self.s3.delete_object(&key).await;
                }
            }
        }

        // Update File status AND variants_json
        let transform_count = successful_variants.len() as i64;
        let mut file_active: file::ActiveModel = file.clone().into();
        file_active.status = Set("ready".to_string());
        file_active.variants_json = Set(serde_json::Value::Object(successful_variants));
        file_active.updated_at = Set(chrono::Utc::now().naive_utc());
        file_active.update(&self.db).await.map_err(|e| e.to_string())?;

        // Update Project transforms_used counter AND add variant storage using SeaORM expressions
        if transform_count > 0 || total_variant_bytes > 0 {
            let _ = project::Entity::update_many()
                .col_expr(
                    project::Column::TransformsUsed,
                    Expr::col(project::Column::TransformsUsed).add(transform_count),
                )
                .col_expr(
                    project::Column::TransformsTotal,
                    Expr::col(project::Column::TransformsTotal).add(transform_count),
                )
                .col_expr(
                    project::Column::StorageUsedBytes,
                    Expr::col(project::Column::StorageUsedBytes).add(total_variant_bytes),
                )
                .filter(project::Column::Id.eq(project.id))
                .exec(&self.db)
                .await;
        }

        // If keep_original is false and all variants processed successfully, delete original from S3
        if !settings.keep_original && transform_count > 0 {
            println!("keep_original=false for project '{}', deleting original: {}", project.name, file.s3_key);
            if let Err(e) = self.s3.delete_object(&file.s3_key).await {
                eprintln!("Failed to delete original after variant processing: {}", e);
            } else {
                let _ = project::Entity::update_many()
                    .col_expr(
                        project::Column::StorageUsedBytes,
                        Expr::cust(format!("GREATEST(0, storage_used_bytes - {})", file.size)),
                    )
                    .filter(project::Column::Id.eq(project.id))
                    .exec(&self.db)
                    .await;
            }
        }

        Ok(())
    }
}
