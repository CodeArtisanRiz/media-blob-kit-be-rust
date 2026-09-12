use axum::{
    extract::{Path, Query, State, Extension},
    response::{Redirect},
    Json,
};
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, PaginatorTrait,
    Condition,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

use crate::entities::{file, project};
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::pagination::PaginatedResponse;
use crate::services::s3::S3Service;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ListFilesQuery {
    pub page: Option<u64>,
    pub limit: Option<u64>,
    pub project_id: Option<Uuid>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct FileResponse {
    pub id: Uuid,
    pub project_id: Uuid,
    pub filename: String,
    pub mime_type: String,
    pub size: i64,
    pub url: String, // Public URL (if public) or Presigned
    #[schema(value_type = Object)]
    pub variants: Value,
    pub created_at: String,
}

impl From<file::Model> for FileResponse {
    fn from(model: file::Model) -> Self {
        let url = crate::utils::build_s3_url(&model.s3_key);
        let variants = crate::utils::format_variants_json(&model.variants_json);

        Self {
            id: model.id,
            project_id: model.project_id,
            filename: model.filename,
            mime_type: model.mime_type,
            size: model.size,
            url,
            variants,
            created_at: model.created_at.to_string(),
        }
    }
}

#[utoipa::path(
    get,
    path = "/files",
    params(
        ("page" = Option<u64>, Query, description = "Page number"),
        ("limit" = Option<u64>, Query, description = "Items per page"),
        ("project_id" = Option<Uuid>, Query, description = "Filter by Project ID")
    ),
    responses(
        (status = 200, description = "List of files", body = PaginatedResponse<FileResponse>),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "File Management"
)]
pub async fn list_files(
    Extension(user): Extension<AuthUser>,
    State(db): State<sea_orm::DatabaseConnection>,
    Query(query): Query<ListFilesQuery>,
) -> Result<Json<PaginatedResponse<FileResponse>>, AppError> {
    let page = query.page.unwrap_or(1).max(1);
    let limit = query.limit.unwrap_or(10).max(1);

    // 2. Build Filter
    let mut condition = Condition::all();

    // Role-based Access Control
    match user.role {
        crate::entities::user::Role::Su => {
            // SU can see all files, or filter by specific project
            if let Some(pid) = query.project_id {
                condition = condition.add(file::Column::ProjectId.eq(pid));
            }
        },
        _ => {
            // Admin/User can only see files from projects they own
            // First, find all project IDs owned by this user
            let user_projects: Vec<Uuid> = project::Entity::find()
                .filter(project::Column::OwnerId.eq(user.id))
                .select_only()
                .column(project::Column::Id)
                .into_tuple()
                .all(&db)
                .await
                .map_err(|e| AppError::InternalServerError(e.to_string()))?;

            if user_projects.is_empty() {
                // If user has no projects, return empty list immediately
                return Ok(Json(PaginatedResponse {
                    data: vec![],
                    total_items: 0,
                    total_pages: 0,
                    current_page: page,
                    page_size: limit,
                }));
            }

            if let Some(pid) = query.project_id {
                // If requesting specific project, verify ownership
                if !user_projects.contains(&pid) {
                    return Err(AppError::Forbidden("Access denied to this project".into()));
                }
                condition = condition.add(file::Column::ProjectId.eq(pid));
            } else {
                // Filter where project_id IN (user_projects)
                condition = condition.add(file::Column::ProjectId.is_in(user_projects));
            }
        }
    }

    // 3. Execute Query
    let paginator = file::Entity::find()
        .filter(condition)
        .order_by_desc(file::Column::CreatedAt)
        .paginate(&db, limit);

    let total_items = paginator.num_items().await.map_err(|e| AppError::InternalServerError(e.to_string()))?;
    let total_pages = paginator.num_pages().await.map_err(|e| AppError::InternalServerError(e.to_string()))?;
    let items = paginator.fetch_page(page - 1).await.map_err(|e| AppError::InternalServerError(e.to_string()))?;

    let data: Vec<FileResponse> = items.into_iter().map(FileResponse::from).collect();

    Ok(Json(PaginatedResponse {
        data,
        total_items,
        total_pages,
        current_page: page,
        page_size: limit,
    }))
}

// GET /files/:id
#[utoipa::path(
    get,
    path = "/files/{id}",
    params(
        ("id" = Uuid, Path, description = "File ID")
    ),
    responses(
        (status = 200, description = "File details", body = FileResponse),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "File Management"
)]
pub async fn get_file(
    Path(id): Path<Uuid>,
    Extension(user): Extension<AuthUser>,
    State(db): State<sea_orm::DatabaseConnection>,
) -> Result<Json<FileResponse>, AppError> {
    // 1. Get File
    let file = file::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?
        .ok_or(AppError::NotFound("File not found".into()))?;

    // 3. Verify Access
    if user.role != crate::entities::user::Role::Su {
        // Check if user owns the project this file belongs to
        let project = project::Entity::find_by_id(file.project_id)
            .one(&db)
            .await
            .map_err(|e| AppError::InternalServerError(e.to_string()))?
            .ok_or(AppError::NotFound("Project not found".into()))?; // Should not happen for valid file

        if project.owner_id != user.id {
            return Err(AppError::Forbidden("Access denied to this file".into()));
        }
    }

    Ok(Json(FileResponse::from(file)))
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct ContentQuery {
    pub variant: Option<String>,
}

// GET /files/:id/content
#[utoipa::path(
    get,
    path = "/files/{id}/content",
    params(
        ("id" = Uuid, Path, description = "File ID"),
        ("variant" = Option<String>, Query, description = "Image variant name (e.g. 'thumbnail')")
    ),
    responses(
        (status = 307, description = "Temporary redirect to S3 URL"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "File Management"
)]
pub async fn get_file_content(
    Path(id): Path<Uuid>,
    Query(query): Query<ContentQuery>,
    Extension(user): Extension<AuthUser>,
    State(db): State<sea_orm::DatabaseConnection>,
) -> Result<Redirect, AppError> {
    // 1. Get File
    let file = file::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?
        .ok_or(AppError::NotFound("File not found".into()))?;

    // 3. Verify Access
    if user.role != crate::entities::user::Role::Su {
        let project = project::Entity::find_by_id(file.project_id)
            .one(&db)
            .await
            .map_err(|e| AppError::InternalServerError(e.to_string()))?
            .ok_or(AppError::NotFound("Project not found".into()))?;

        if project.owner_id != user.id {
            return Err(AppError::Forbidden("Access denied to this file".into()));
        }
    }

    // 4. Resolve Key (Original vs Variant)
    let key = if let Some(variant_name) = query.variant {
        let variants = file.variants_json.as_object().ok_or(AppError::InternalServerError("Invalid variants data".into()))?;
        
        if let Some(variant_path) = variants.get(&variant_name) {
            let variant_value = variant_path.as_str().ok_or(AppError::NotFound("Invalid variant path".into()))?;
            crate::utils::extract_s3_key(variant_value)
        } else {
            return Err(AppError::NotFound(format!("Variant '{}' not found", variant_name)));
        }
    } else {
        file.s3_key
    };

    // 5. Generate Presigned URL
    let s3_service = S3Service::new().await;
    let url = s3_service.get_presigned_url(&key, Duration::from_secs(3600)).await?;


    // 6. Redirect
    Ok(Redirect::temporary(&url))
}

// DELETE /files/:id
#[utoipa::path(
    delete,
    path = "/files/{id}",
    params(
        ("id" = Uuid, Path, description = "File ID")
    ),
    responses(
        (status = 200, description = "File deleted successfully"),
        (status = 404, description = "File not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "File Management"
)]
pub async fn delete_file(
    Path(id): Path<Uuid>,
    Extension(user): Extension<AuthUser>,
    State(db): State<sea_orm::DatabaseConnection>,
) -> Result<Json<serde_json::Value>, AppError> {
    // 1. Get File
    let file = file::Entity::find_by_id(id)
        .one(&db)
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?
        .ok_or(AppError::NotFound("File not found".into()))?;

    // 2. Verify Access
    if user.role != crate::entities::user::Role::Su {
        let project = project::Entity::find_by_id(file.project_id)
            .one(&db)
            .await
            .map_err(|e| AppError::InternalServerError(e.to_string()))?
            .ok_or(AppError::NotFound("Project not found".into()))?;

        if project.owner_id != user.id {
            return Err(AppError::Forbidden("Access denied to this file".into()));
        }
    }

    // 3. Delete from S3 (Original + Variants)
    let s3_service = S3Service::new().await;

    // Delete Original
    if let Err(e) = s3_service.delete_object(&file.s3_key).await {
        eprintln!("Failed to delete original file from S3: {}", e);
        // Continue to try deleting variants and DB record? 
        // Or fail? Best effort is usually preferred for cleanup.
    }

    // Delete Variants
    if let Some(variants) = file.variants_json.as_object() {
        for (_variant_name, variant_path) in variants {
            if let Some(variant_str) = variant_path.as_str() {
                // Extract Key logic (similar to get_file_content but simplified or extract common logic)
                // For now, let's copy the extraction logic or assume logic.
                // Wait, if we stored full URLs, we need to extract key.
                
                let config = crate::config::get_config();
                let bucket = &config.s3_bucket_name;
                
                let key_to_delete = if let Some(idx) = variant_str.find(&format!("/{}/", bucket)) {
                     Some(variant_str[idx + bucket.len() + 2..].to_string())
                } else if let Ok(url) = url::Url::parse(variant_str) {
                     Some(url.path().trim_start_matches('/').to_string())
                } else {
                    None
                };

                if let Some(key) = key_to_delete {
                    if let Err(e) = s3_service.delete_object(&key).await {
                        eprintln!("Failed to delete variant from S3: {}", e);
                    }
                }
            }
        }
    }

    // 4. Delete from DB
    // Use ActiveModel to delete
    let res = file::Entity::delete_by_id(id)
        .exec(&db)
        .await
        .map_err(|e| AppError::InternalServerError(e.to_string()))?;

    if res.rows_affected == 0 {
         return Err(AppError::NotFound("File not found in DB".into()));
    }

    // 5. Decrement project storage quota tracking
    use sea_orm::sea_query::Expr;
    let _ = project::Entity::update_many()
        .col_expr(
            project::Column::StorageUsedBytes,
            Expr::cust(format!("GREATEST(0, storage_used_bytes - {})", file.size)),
        )
        .filter(project::Column::Id.eq(file.project_id))
        .exec(&db)
        .await;

    Ok(Json(serde_json::json!({
        "message": "File deleted successfully",
        "id": id
    })))
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct BatchDeleteRequest {
    pub ids: Vec<Uuid>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct BatchDeleteResponse {
    pub deleted_count: usize,
    pub failed_count: usize,
    pub freed_bytes: i64,
}

// POST /files/batch-delete
#[utoipa::path(
    post,
    path = "/files/batch-delete",
    request_body = BatchDeleteRequest,
    responses(
        (status = 200, description = "Batch files deleted", body = BatchDeleteResponse),
        (status = 400, description = "Too many files requested or empty"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "File Management"
)]
pub async fn batch_delete_files(
    Extension(user): Extension<AuthUser>,
    State(db): State<sea_orm::DatabaseConnection>,
    Json(payload): Json<BatchDeleteRequest>,
) -> Result<Json<BatchDeleteResponse>, AppError> {
    if payload.ids.is_empty() {
        return Err(AppError::BadRequest("No file IDs provided".into()));
    }
    if payload.ids.len() > 100 {
        return Err(AppError::BadRequest("Cannot delete more than 100 files in one request".into()));
    }

    let files = file::Entity::find()
        .filter(file::Column::Id.is_in(payload.ids.clone()))
        .all(&db)
        .await
        .map_err(AppError::DatabaseError)?;

    let s3_service = S3Service::new().await;
    let config = crate::config::get_config();
    let bucket = &config.s3_bucket_name;

    let mut deleted_count = 0;
    let mut failed_count = 0;
    let mut freed_bytes: i64 = 0;

    for f in files {
        // Ownership check
        if user.role != crate::entities::user::Role::Su {
            let project = project::Entity::find_by_id(f.project_id)
                .one(&db)
                .await
                .map_err(|e| AppError::InternalServerError(e.to_string()))?;
            if let Some(p) = project {
                if p.owner_id != user.id {
                    failed_count += 1;
                    continue;
                }
            } else {
                failed_count += 1;
                continue;
            }
        }

        // Delete S3 original
        let _ = s3_service.delete_object(&f.s3_key).await;

        // Delete variants
        if let Some(variants) = f.variants_json.as_object() {
            for (_v_name, v_path) in variants {
                if let Some(variant_str) = v_path.as_str() {
                    let key_to_delete = if let Some(idx) = variant_str.find(&format!("/{}/", bucket)) {
                        Some(variant_str[idx + bucket.len() + 2..].to_string())
                    } else if let Ok(url) = url::Url::parse(variant_str) {
                        Some(url.path().trim_start_matches('/').to_string())
                    } else {
                        None
                    };

                    if let Some(key) = key_to_delete {
                        let _ = s3_service.delete_object(&key).await;
                    }
                }
            }
        }

        // Delete DB record
        if let Ok(_) = file::Entity::delete_by_id(f.id).exec(&db).await {
            deleted_count += 1;
            freed_bytes += f.size;
            
            use sea_orm::sea_query::Expr;
            let _ = project::Entity::update_many()
                .col_expr(
                    project::Column::StorageUsedBytes,
                    Expr::cust(format!("GREATEST(0, storage_used_bytes - {})", f.size)),
                )
                .filter(project::Column::Id.eq(f.project_id))
                .exec(&db)
                .await;
        } else {
            failed_count += 1;
        }
    }

    Ok(Json(BatchDeleteResponse {
        deleted_count,
        failed_count,
        freed_bytes,
    }))
}
