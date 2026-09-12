use axum::{
    extract::{Multipart, State, Query},
    response::Json,
    Extension,
};
use sea_orm::{ActiveModelTrait, DatabaseConnection, Set, ConnectionTrait};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::entities::{file, job};
use crate::error::AppError;
use crate::middleware::api_key::ProjectContext;
use crate::services::s3::S3Service;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct UploadImageQuery {
    /// Comma-separated variant names to generate (e.g. 'thumbnail,card'). Omit to generate all project variants.
    #[param(inline)]
    pub variants: Option<String>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct FileUploadResponse {
    id: Uuid,
    url: String,
    filename: String,
    mime_type: String,
    size: i64,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ImageUploadResponse {
    id: Uuid,
    original_url: String,
    variants: serde_json::Value,
}

// Helper to get file extension
fn get_extension(filename: &str) -> String {
    std::path::Path::new(filename)
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("bin")
        .to_lowercase()
}

// Helper to sanitize bucket name
fn sanitize_bucket_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
}

#[utoipa::path(
    post,
    path = "/upload/file",
    tag = "File Upload",
    request_body(content = Vec<u8>, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "File uploaded successfully", body = FileUploadResponse),
        (status = 400, description = "Bad Request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal Server Error")
    ),
    security(
        ("api_key" = [])
    )
)]
pub async fn upload_file(
    State(db): State<DatabaseConnection>,
    Extension(project): Extension<ProjectContext>,
    mut multipart: Multipart,
) -> Result<Json<FileUploadResponse>, AppError> {
    let s3_service = S3Service::new().await;
    
    while let Some(field) = multipart.next_field().await.map_err(|_| AppError::BadRequest("Invalid multipart data".to_string()))? {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("unknown").to_string();
            let content_type = field.content_type().unwrap_or("application/octet-stream").to_string();
            let data = field.bytes().await.map_err(|_| AppError::InternalServerError("Failed to read file bytes".to_string()))?;
            let size = data.len() as i64;
            let ext = get_extension(&filename);
            
            let file_id = Uuid::new_v4();
            // Format: {project_name}-{project_id}/files/{file_id}.{ext}
            let s3_key = format!("{}-{}/files/{}.{}", sanitize_bucket_name(&project.name), project.id, file_id, ext);
            
            // Ensure bucket exists
            s3_service.ensure_bucket_exists().await?;

            // Upload to S3
            s3_service.put_object(&s3_key, data.to_vec(), &content_type).await?;
            
            // Save to DB
            let file = file::ActiveModel {
                id: Set(file_id),
                project_id: Set(project.id),
                s3_key: Set(s3_key.clone()),
                filename: Set(filename.clone()),
                mime_type: Set(content_type.clone()),
                size: Set(size),
                status: Set("ready".to_string()),
                variants_json: Set(serde_json::json!({})),
                content_hash: Set(None),
                created_at: Set(chrono::Utc::now().naive_utc()),
                updated_at: Set(chrono::Utc::now().naive_utc()),
            };
            
            let saved_file = file.insert(&db).await.map_err(AppError::DatabaseError)?;
            
            // Update storage quota tracking for project
            let update_stmt = sea_orm::Statement::from_string(
                db.get_database_backend(),
                format!("UPDATE projects SET storage_used_bytes = storage_used_bytes + {} WHERE id = '{}'", size, project.id),
            );
            let _ = db.execute(update_stmt).await;

            // Construct URL
            let url = crate::utils::build_s3_url(&s3_key);

            println!("Upload | POST /upload/file | project={} | file={} | res=200", project.name, saved_file.filename);
            return Ok(Json(FileUploadResponse {
                id: saved_file.id,
                url,
                filename: saved_file.filename,
                mime_type: saved_file.mime_type,
                size: saved_file.size,
            }));
        }
    }
    
    println!("Upload | POST /upload/file | project={} | res=400 | No file field found", project.name);
    Err(AppError::BadRequest("No file field found".to_string()))
}

#[utoipa::path(
    post,
    path = "/upload/image",
    tag = "File Upload",
    params(
        ("variants" = Option<String>, Query, description = "Comma-separated list of variant names to generate (e.g. 'thumbnail,card')")
    ),
    request_body(content = Vec<u8>, content_type = "multipart/form-data"),
    responses(
        (status = 200, description = "Image uploaded successfully", body = ImageUploadResponse),
        (status = 400, description = "Bad Request"),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal Server Error")
    ),
    security(
        ("api_key" = [])
    )
)]
pub async fn upload_image(
    State(db): State<DatabaseConnection>,
    Extension(project): Extension<ProjectContext>,
    Query(query): Query<UploadImageQuery>,
    mut multipart: Multipart,
) -> Result<Json<ImageUploadResponse>, AppError> {
    let s3_service = S3Service::new().await;

    // Parse requested variant filter (if specified)
    let requested_set: Option<std::collections::HashSet<String>> = query.variants.as_deref().map(|s| {
        s.split(',')
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
            .collect()
    });

    while let Some(field) = multipart.next_field().await.map_err(|_| AppError::BadRequest("Invalid multipart data".to_string()))? {
        if field.name() == Some("file") {
            let filename = field.file_name().unwrap_or("unknown").to_string();
            let mut content_type = field.content_type().unwrap_or("application/octet-stream").to_string();
            let ext = get_extension(&filename);
            
            // Normalize SVG and modern image content types
            if ext == "svg" {
                content_type = "image/svg+xml".to_string();
            } else if ext == "webp" && !content_type.starts_with("image/") {
                content_type = "image/webp".to_string();
            } else if ext == "avif" && !content_type.starts_with("image/") {
                content_type = "image/avif".to_string();
            }

            // Basic validation for image type
            if !content_type.starts_with("image/") && ext != "svg" && ext != "webp" && ext != "avif" && ext != "png" && ext != "jpg" && ext != "jpeg" {
                println!("Upload | POST /upload/image | project={} | res=400 | File is not an image", project.name);
                return Err(AppError::BadRequest("File is not an image".to_string()));
            }

            let data = field.bytes().await.map_err(|_| AppError::InternalServerError("Failed to read file bytes".to_string()))?;
            let size = data.len() as i64;

            let file_id = Uuid::new_v4();
            // Format: {project_name}-{project_id}/images/original/{file_id}.{ext}
            let s3_key = format!("{}-{}/images/original/{}.{}", sanitize_bucket_name(&project.name), project.id, file_id, ext);

            // Ensure bucket exists
            s3_service.ensure_bucket_exists().await?;

            // Upload Original to S3
            s3_service.put_object(&s3_key, data.to_vec(), &content_type).await?;

            let is_svg = content_type == "image/svg+xml" || ext == "svg";

            // Calculate future variant URLs
            let mut variants_map = serde_json::Map::new();
            let mut filtered_variants_config = std::collections::HashMap::new();

            if !is_svg {
                if let Some(variants_config) = &project.settings.variants {
                    for (variant_name, config) in variants_config {
                        if let Some(ref set) = requested_set {
                            if !set.contains(variant_name) {
                                continue;
                            }
                        }

                        filtered_variants_config.insert(variant_name.clone(), config.clone());

                        let variant_ext = config.format.as_deref().unwrap_or(&ext);
                        let variant_ext = if variant_ext == "original" { &ext } else { variant_ext };
                        
                        let variant_key = format!("{}-{}/images/{}/{}.{}", 
                            sanitize_bucket_name(&project.name), 
                            project.id, 
                            variant_name, 
                            file_id, 
                            variant_ext
                        );

                        let variant_url = crate::utils::build_s3_url(&variant_key);
                        variants_map.insert(variant_name.clone(), serde_json::Value::String(variant_url));
                    }
                }
            }
            
            let variants = serde_json::Value::Object(variants_map);

            // Save to DB (SVGs are immediately marked ready, raster images queued for processing)
            let file_status = if is_svg || filtered_variants_config.is_empty() {
                "ready"
            } else {
                "processing"
            };

            let file = file::ActiveModel {
                id: Set(file_id),
                project_id: Set(project.id),
                s3_key: Set(s3_key.clone()),
                filename: Set(filename),
                mime_type: Set(content_type),
                size: Set(size),
                status: Set(file_status.to_string()),
                variants_json: Set(variants.clone()),
                content_hash: Set(None),
                created_at: Set(chrono::Utc::now().naive_utc()),
                updated_at: Set(chrono::Utc::now().naive_utc()),
            };

            let saved_file = file.insert(&db).await.map_err(AppError::DatabaseError)?;

            // Update storage quota tracking for project
            let update_stmt = sea_orm::Statement::from_string(
                db.get_database_backend(),
                format!("UPDATE projects SET storage_used_bytes = storage_used_bytes + {} WHERE id = '{}'", size, project.id),
            );
            let _ = db.execute(update_stmt).await;

            // Only enqueue worker job if raster transformations are configured
            if !is_svg && !filtered_variants_config.is_empty() {
                let job = job::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    file_id: Set(saved_file.id),
                    status: Set("pending".to_string()),
                    payload: Set(serde_json::json!({
                        "variants": filtered_variants_config
                    })),
                    attempt_count: Set(0),
                    max_retries: Set(3),
                    created_at: Set(chrono::Utc::now().naive_utc()),
                    updated_at: Set(chrono::Utc::now().naive_utc()),
                };

                job.insert(&db).await.map_err(AppError::DatabaseError)?;
            }

            // Construct URL
            let url = crate::utils::build_s3_url(&s3_key);

            println!("Upload | POST /upload/image | project={} | file={} | res=200", project.name, file_id);
            return Ok(Json(ImageUploadResponse {
                id: file_id,
                original_url: url,
                variants,
            }));
        }
    }

    println!("Upload | POST /upload/image | project={} | res=400 | No file field found", project.name);
    Err(AppError::BadRequest("No file field found".to_string()))
}
