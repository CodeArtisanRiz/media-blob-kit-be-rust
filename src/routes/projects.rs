use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, Set, PaginatorTrait,
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
    id: Uuid,
    name: String,
    description: Option<String>,
    #[schema(value_type = Object)]
    settings: Value,
    created_at: chrono::NaiveDateTime,
    updated_at: chrono::NaiveDateTime,
}

impl From<project::Model> for ProjectResponse {
    fn from(project: project::Model) -> Self {
        ProjectResponse {
            id: project.id,
            name: project.name,
            description: project.description,
            settings: project.settings,
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
        created_at: Set(chrono::Utc::now().naive_utc()),
        updated_at: Set(chrono::Utc::now().naive_utc()),
        ..Default::default()
    };

    let created_project = project.insert(&db).await?;

    println!("Project | POST /projects | user={} | name={} | res=201", auth_user.username, created_project.name);
    Ok((StatusCode::CREATED, Json(ProjectResponse::from(created_project))))
}

// GET /projects
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

// DELETE /projects/:id
#[utoipa::path(
    delete,
    path = "/projects/{id}",
    params(
        ("id" = Uuid, Path, description = "Project ID"),
        ("permanent" = Option<bool>, Query, description = "Permanently delete project and files")
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
    
    // Check if hard delete requested
    let hard_delete = query.permanent.unwrap_or(false);

    let project = Project::find_by_id(project_id)
        .filter(project::Column::OwnerId.eq(auth_user.id))
        .filter(project::Column::DeletedAt.is_null()) // Always check soft delete first
        .one(&db)
        .await?;

    match project {
        Some(p) => {
            if hard_delete {
                // HARD DELETE LOGIC
                
                // 1. Find all files for this project
                let files = file::Entity::find()
                    .filter(file::Column::ProjectId.eq(p.id))
                    .all(&db)
                    .await
                    .map_err(|e| AppError::InternalServerError(e.to_string()))?;

                let s3_service = S3Service::new().await;

                // 2. Iterate and delete from S3
                for f in files {
                    // Delete Original
                    let _ = s3_service.delete_object(&f.s3_key).await;

                    // Delete Variants
                    if let Some(variants) = f.variants_json.as_object() {
                        for (_v_name, v_path) in variants {
                            if let Some(v_str) = v_path.as_str() {
                                // Extract key logic (simplified for now, ideally shared helper)
                                let config = crate::config::get_config();
                                let bucket = &config.s3_bucket_name;
                                
                                let key_to_delete = if let Some(idx) = v_str.find(&format!("/{}/", bucket)) {
                                     Some(v_str[idx + bucket.len() + 2..].to_string())
                                } else if let Ok(url) = url::Url::parse(v_str) {
                                     Some(url.path().trim_start_matches('/').to_string())
                                } else {
                                    None
                                };
                                
                                if let Some(k) = key_to_delete {
                                    let _ = s3_service.delete_object(&k).await;
                                }
                            }
                        }
                    }
                    
                    // Delete File Row (Optional if cascade is set on DB, but SeaORM needs explicit handling if not relying on DB cascade entirely for logic)
                    // DB `on_delete=Cascade` handles this automatically if configured in Postgres.
                    // But we will be safe and delete manually or rely on cascade. 
                    // Since schema has `on_delete="Cascade"`, deleting project *should* delete files.
                    // But good to clean up S3 first.
                }

                // 3. Delete Project from DB
                let res = Project::delete_by_id(p.id).exec(&db).await.map_err(|e| AppError::InternalServerError(e.to_string()))?;
                 
                 if res.rows_affected == 0 {
                    return Err(AppError::InternalServerError("Failed to delete project".into()));
                 }

                println!("Project | DELETE /projects/{}?permanent=true | user={} | res=200", project_id, auth_user.username);
                 Ok(Json(serde_json::json!({
                    "message": "Project permanently deleted"
                })))

            } else {
                // SOFT DELETE LOGIC (Existing)
                let mut active_project = p.into_active_model();
                active_project.deleted_at = Set(Some(chrono::Utc::now().naive_utc()));
                active_project.update(&db).await?;
    
                println!("Project | DELETE /projects/{} | user={} | res=200", project_id, auth_user.username);
                Ok(Json(serde_json::json!({
                    "message": "Project deleted successfully"
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
    path = "/projects/{id}/sync-variants",
    params(
        ("id" = Uuid, Path, description = "Project ID")
    ),
    responses(
        (status = 202, description = "Variant synchronization started"),
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

    match project {
        Some(p) => {
             // Create Sync Job Payload (Optional, if we want to log it or use it for the wrapper job logic in future)
             // But we are spawning individual file jobs directly here.
             
             // 1. Find all image files
            let files = file::Entity::find()
                .filter(file::Column::ProjectId.eq(p.id))
                .filter(file::Column::MimeType.like("image/%")) // SeaORM like? or contains?
                // SeaORM uses LIKE for strings. 
                // MimeType is String.
                // .filter(file::Column::MimeType.contains("image")) Is safer if SeaORM supports it.
                // Let's use `starts_with` or `contains`.
                .filter(file::Column::MimeType.contains("image"))
                .all(&db)
                .await
                .map_err(|e| AppError::InternalServerError(e.to_string()))?;

            let variants_json = p.settings.get("variants").cloned().unwrap_or(serde_json::json!({}));
            
            let mut job_count = 0;
            for f in files {
                let job_payload = serde_json::json!({
                    "type": "sync_file_variants",
                    "variants_config": variants_json 
                });

                let job = job::ActiveModel {
                    id: Set(Uuid::new_v4()),
                    file_id: Set(f.id),
                    status: Set("pending".to_string()),
                    payload: Set(job_payload),
                    created_at: Set(chrono::Utc::now().naive_utc()),
                    updated_at: Set(chrono::Utc::now().naive_utc()),
                    ..Default::default()
                };

                job.insert(&db).await.map_err(|e| AppError::InternalServerError(e.to_string()))?;
                job_count += 1;
            }

            println!("Project | POST /projects/{}/sync-variants | user={} | jobs_spawned={} | res=202", project_id, auth_user.username, job_count);
            Ok(Json(serde_json::json!({
                "message": "Variant synchronization started",
                "jobs_queued": job_count
            })))
        }
        None => {
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
        (status = 404, description = "Project not found or not soft-deleted"),
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
            let updated = active_project.update(&db).await?;

            println!("Project | POST /projects/{}/restore | user={} | res=200", project_id, auth_user.username);
            Ok(Json(ProjectResponse::from(updated)))
        }
        None => {
            Err(AppError::NotFound("Project not found or not in trash".to_string()))
        }
    }
}
