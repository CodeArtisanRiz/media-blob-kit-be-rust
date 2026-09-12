use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, PaginatorTrait, ModelTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::entities::project::{self, Entity as Project};
use crate::entities::{file, job};
use crate::error::AppError;
use crate::middleware::auth::AuthUser;
use crate::pagination::{Pagination, PaginatedResponse};
use crate::services::s3::S3Service;
use axum::extract::Query;

#[derive(Deserialize, utoipa::IntoParams)]
pub struct DeleteProjectQuery {
    pub permanent: Option<bool>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateProjectRequest {
    name: String,
    description: Option<String>,
    #[schema(value_type = Object)]
    settings: Option<Value>,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateProjectRequest {
    name: Option<String>,
    description: Option<String>,
    #[schema(value_type = Object)]
    settings: Option<Value>,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ProjectResponse {
    #[schema(value_type = String)]
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    #[schema(value_type = Object)]
    pub settings: Value,
    pub storage_used_bytes: i64,
    pub storage_limit_bytes: i64,
    pub transforms_used: i64,
    pub transforms_limit: i64,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

impl From<project::Model> for ProjectResponse {
    fn from(project: project::Model) -> Self {
        ProjectResponse {
            id: project.id,
            name: project.name,
            description: project.description,
            settings: project.settings,
            storage_used_bytes: project.storage_used_bytes,
            storage_limit_bytes: project.storage_limit_bytes,
            transforms_used: project.transforms_used,
            transforms_limit: project.transforms_limit,
            created_at: project.created_at,
            updated_at: project.updated_at,
        }
    }
}

#[utoipa::path(
    post,
    path = "/projects",
    request_body = CreateProjectRequest,
    responses(
        (status = 201, description = "Project created successfully", body = ProjectResponse),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn create_project(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Json(payload): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<ProjectResponse>), AppError> {
    let project = project::ActiveModel {
        id: Set(Uuid::new_v4()),
        owner_id: Set(auth_user.id),
        name: Set(payload.name),
        description: Set(payload.description),
        settings: Set(payload.settings.unwrap_or(serde_json::json!({}))),
        storage_used_bytes: Set(0),
        storage_limit_bytes: Set(5 * 1024 * 1024 * 1024), // 5 GB default quota
        transforms_used: Set(0),
        transforms_limit: Set(10_000), // 10,000 monthly transforms quota
        created_at: Set(chrono::Utc::now().naive_utc()),
        updated_at: Set(chrono::Utc::now().naive_utc()),
        ..Default::default()
    };

    let created_project = project.insert(&db).await?;

    println!("Project | POST /projects | user={} | name={} | res=201", auth_user.username, created_project.name);
    Ok((StatusCode::CREATED, Json(ProjectResponse::from(created_project))))
}

#[utoipa::path(
    get,
    path = "/projects",
    params(
        ("page" = Option<u64>, Query, description = "Page number"),
        ("limit" = Option<u64>, Query, description = "Items per page")
    ),
    responses(
        (status = 200, description = "List of user's projects", body = PaginatedResponse<ProjectResponse>),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn list_projects(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Query(pagination): Query<Pagination>,
) -> Result<Json<PaginatedResponse<ProjectResponse>>, AppError> {
    let page = pagination.page.unwrap_or(1);
    let limit = pagination.limit.unwrap_or(10);

    let paginator = Project::find()
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_null())
        .order_by_desc(project::Column::CreatedAt)
        .paginate(&db, limit);

    let total_items = paginator.num_items().await.map_err(AppError::DatabaseError)?;
    let projects = paginator.fetch_page(page - 1).await.map_err(AppError::DatabaseError)?;

    let responses: Vec<ProjectResponse> = projects.into_iter().map(ProjectResponse::from).collect();
    
    println!("Project | GET /projects | user={} | count={} | res=200", auth_user.username, total_items);
    Ok(Json(PaginatedResponse::new(responses, total_items, page, limit)))
}

#[utoipa::path(
    get,
    path = "/projects/{id}",
    params(
        ("id" = Uuid, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Project details", body = ProjectResponse),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn get_project(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectResponse>, AppError> {
    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_null())
        .one(&db)
        .await?;

    match project {
        Some(p) => {
            println!("Project | GET /projects/{} | user={} | res=200", project_id, auth_user.username);
            Ok(Json(ProjectResponse::from(p)))
        }
        None => {
            println!("Project | GET /projects/{} | user={} | res=404 | Project not found", project_id, auth_user.username);
            Err(AppError::NotFound("Project not found".to_string()))
        }
    }
}

#[utoipa::path(
    put,
    path = "/projects/{id}",
    params(
        ("id" = Uuid, Path, description = "Project ID")
    ),
    request_body = UpdateProjectRequest,
    responses(
        (status = 200, description = "Project updated successfully", body = ProjectResponse),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn update_project(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Path(project_id): Path<Uuid>,
    Json(payload): Json<UpdateProjectRequest>,
) -> Result<Json<ProjectResponse>, AppError> {
    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_null())
        .one(&db)
        .await?;

    match project {
        Some(p) => {
            let mut active_project = p.into_active_model();
            
            if let Some(name) = payload.name {
                active_project.name = Set(name);
            }
            if let Some(description) = payload.description {
                active_project.description = Set(Some(description));
            }
            if let Some(settings) = payload.settings {
                active_project.settings = Set(settings);
            }
            active_project.updated_at = Set(chrono::Utc::now().naive_utc());

            let updated_project = active_project.update(&db).await?;
            println!("Project | PUT /projects/{} | user={} | res=200", project_id, auth_user.username);
            Ok(Json(ProjectResponse::from(updated_project)))
        }
        None => {
            println!("Project | PUT /projects/{} | user={} | res=404 | Project not found", project_id, auth_user.username);
            Err(AppError::NotFound("Project not found".to_string()))
        }
    }
}

#[utoipa::path(
    delete,
    path = "/projects/{id}",
    params(
        ("id" = Uuid, Path, description = "Project ID"),
        ("permanent" = Option<bool>, Query, description = "Permanent hard deletion flag")
    ),
    responses(
        (status = 200, description = "Project deleted successfully"),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn delete_project(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Path(project_id): Path<Uuid>,
    Query(query): Query<DeleteProjectQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .one(&db)
        .await?;

    match project {
        Some(p) => {
            let permanent = query.permanent.unwrap_or(false);

            if permanent {
                let s3_service = S3Service::new().await;
                let files = file::Entity::find()
                    .filter(file::Column::ProjectId.eq(project_id))
                    .all(&db)
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

                p.delete(&db).await?;
                println!("Project | DELETE /projects/{} | user={} | mode=hard | res=200", project_id, auth_user.username);
                Ok(Json(serde_json::json!({
                    "message": "Project and all associated assets permanently deleted"
                })))
            } else {
                let mut active_project = p.into_active_model();
                active_project.deleted_at = Set(Some(chrono::Utc::now().naive_utc()));
                active_project.update(&db).await?;
                println!("Project | DELETE /projects/{} | user={} | mode=soft | res=200", project_id, auth_user.username);
                Ok(Json(serde_json::json!({
                    "message": "Project soft-deleted successfully (30-day retention)"
                })))
            }
        }
        None => {
            println!("Project | DELETE /projects/{} | user={} | res=404 | Project not found", project_id, auth_user.username);
            Err(AppError::NotFound("Project not found".to_string()))
        }
    }
}

#[utoipa::path(
    post,
    path = "/projects/{id}/restore",
    params(
        ("id" = Uuid, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Project restored successfully", body = ProjectResponse),
        (status = 404, description = "Project not found or not in soft-deleted state"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn restore_project(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<ProjectResponse>, AppError> {
    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_not_null())
        .one(&db)
        .await?;

    match project {
        Some(p) => {
            let mut active_project = p.into_active_model();
            active_project.deleted_at = Set(None);
            active_project.updated_at = Set(chrono::Utc::now().naive_utc());
            let restored = active_project.update(&db).await?;
            println!("Project | POST /projects/{}/restore | user={} | res=200", project_id, auth_user.username);
            Ok(Json(ProjectResponse::from(restored)))
        }
        None => {
            println!("Project | POST /projects/{}/restore | user={} | res=404 | Project not found", project_id, auth_user.username);
            Err(AppError::NotFound("Project not found or not soft-deleted".to_string()))
        }
    }
}

#[utoipa::path(
    post,
    path = "/projects/{id}/sync-variants",
    params(
        ("id" = Uuid, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Variant synchronization jobs queued successfully"),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn sync_variants(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_null())
        .one(&db)
        .await?;

    let project = match project {
        Some(p) => p,
        None => return Err(AppError::NotFound("Project not found".to_string())),
    };

    let files = file::Entity::find()
        .filter(file::Column::ProjectId.eq(project_id))
        .filter(file::Column::MimeType.starts_with("image/"))
        .all(&db)
        .await?;

    let variants_config = project.settings.get("variants").cloned().unwrap_or(serde_json::json!({}));

    let mut count = 0;
    for f in files {
        let job = job::ActiveModel {
            id: Set(Uuid::new_v4()),
            file_id: Set(f.id),
            status: Set("pending".to_string()),
            payload: Set(serde_json::json!({
                "variants": variants_config
            })),
            created_at: Set(chrono::Utc::now().naive_utc()),
            updated_at: Set(chrono::Utc::now().naive_utc()),
        };
        job.insert(&db).await?;
        count += 1;
    }

    println!("Project | POST /projects/{}/sync-variants | user={} | jobs_queued={} | res=200", project_id, auth_user.username, count);
    Ok(Json(serde_json::json!({
        "message": format!("Triggered regeneration for {} image(s)", count),
        "jobs_queued": count
    })))
}

/// Delete all original images from S3 for a project (only when keep_original is false).
/// Variants remain intact. This is a destructive operation.
#[utoipa::path(
    post,
    path = "/projects/{id}/delete-originals",
    params(
        ("id" = Uuid, Path, description = "Project ID")
    ),
    responses(
        (status = 200, description = "Originals deleted successfully"),
        (status = 400, description = "keep_original is enabled — disable it first"),
        (status = 404, description = "Project not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Project Management"
)]
pub async fn delete_originals(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<AuthUser>,
    Path(project_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_null())
        .one(&db)
        .await?;

    let project = match project {
        Some(p) => p,
        None => return Err(AppError::NotFound("Project not found".to_string())),
    };

    // Verify keep_original is false
    let settings: crate::models::settings::ProjectSettings = serde_json::from_value(project.settings.clone())
        .unwrap_or_default();

    if settings.keep_original {
        return Err(AppError::BadRequest(
            "Cannot delete originals while keep_original is enabled. Disable it in project settings first.".to_string()
        ));
    }

    let s3_service = S3Service::new().await;

    // Find all image files with 'ready' status (variants have been processed)
    let files = file::Entity::find()
        .filter(file::Column::ProjectId.eq(project_id))
        .filter(file::Column::MimeType.starts_with("image/"))
        .filter(file::Column::Status.eq("ready"))
        .all(&db)
        .await?;

    let mut deleted_count = 0u64;
    let mut freed_bytes: i64 = 0;

    for f in &files {
        // Only delete if the file has variants (originals without variants should be kept)
        let has_variants = f.variants_json.as_object()
            .map(|obj| !obj.is_empty())
            .unwrap_or(false);

        if !has_variants {
            continue;
        }

        // Attempt to delete original from S3
        match s3_service.delete_object(&f.s3_key).await {
            Ok(_) => {
                deleted_count += 1;
                freed_bytes += f.size;
            }
            Err(e) => {
                eprintln!("Failed to delete original {}: {}", f.s3_key, e);
            }
        }
    }

    // Decrement project storage
    if freed_bytes > 0 {
        let update_stmt = sea_orm::Statement::from_string(
            db.get_database_backend(),
            format!(
                "UPDATE projects SET storage_used_bytes = GREATEST(0, storage_used_bytes - {}) WHERE id = '{}'",
                freed_bytes, project_id
            ),
        );
        let _ = db.execute(update_stmt).await;
    }

    println!(
        "Project | POST /projects/{}/delete-originals | user={} | deleted={} | freed={} | res=200",
        project_id, auth_user.username, deleted_count, freed_bytes
    );

    Ok(Json(serde_json::json!({
        "message": format!("Deleted {} original images, freed {} bytes", deleted_count, freed_bytes),
        "deleted_count": deleted_count,
        "freed_bytes": freed_bytes
    })))
}
