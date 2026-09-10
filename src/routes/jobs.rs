use axum::{
    extract::{Query, State},
    Json,
};
use sea_orm::{
    ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect,
    RelationTrait,
};
use serde::{Deserialize, Serialize};
use crate::entities::job::{self, Entity as Job};
use crate::entities::file;
use crate::error::AppError;
use crate::middleware::api_key::ProjectContext;
use crate::pagination::Pagination;

#[derive(Deserialize)]
pub struct JobFilter {
    pub status: Option<String>,
    #[serde(flatten)]
    pub pagination: Pagination,
}

use utoipa::ToSchema;

#[derive(Serialize, ToSchema, Clone)]
pub struct JobResponse {
    pub id: uuid::Uuid,
    pub file_id: uuid::Uuid,
    pub status: String,
    pub payload: serde_json::Value,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

impl From<job::Model> for JobResponse {
    fn from(model: job::Model) -> Self {
        Self {
            id: model.id,
            file_id: model.file_id,
            status: model.status,
            payload: model.payload,
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

#[utoipa::path(
    get,
    path = "/jobs",
    tag = "Jobs",
    params(
        ("status" = Option<String>, Query, description = "Filter by job status (pending, processing, completed, failed)"),
        ("page" = Option<u64>, Query, description = "Page number (default: 1)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 10)")
    ),
    responses(
        (status = 200, description = "List of jobs grouped by project", body = std::collections::HashMap<String, PaginatedProjectJobsResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal Server Error")
    ),
    security(
        ("api_key" = [])
    )
)]
pub async fn list_jobs(
    State(db): State<DatabaseConnection>,
    axum::Extension(project): axum::Extension<ProjectContext>,
    Query(filter): Query<JobFilter>,
) -> Result<Json<std::collections::HashMap<String, PaginatedProjectJobsResponse>>, AppError> {
    let page = filter.pagination.page.unwrap_or(1);
    let limit = filter.pagination.limit.unwrap_or(10);

    let mut query = Job::find()
        .join(sea_orm::JoinType::InnerJoin, job::Relation::File.def())
        .filter(file::Column::ProjectId.eq(project.id))
        .order_by_desc(job::Column::CreatedAt);

    if let Some(status) = filter.status {
        query = query.filter(job::Column::Status.eq(status));
    }

    let paginator = query.paginate(&db, limit);
    let total_items = paginator.num_items().await.map_err(AppError::DatabaseError)?;
    let total_pages = paginator.num_pages().await.map_err(AppError::DatabaseError)?;
    let jobs = paginator.fetch_page(page - 1).await.map_err(AppError::DatabaseError)?;

    let data: Vec<JobResponse> = jobs.into_iter().map(JobResponse::from).collect();

    let response = PaginatedProjectJobsResponse {
        project_id: project.id,
        jobs: data,
        total_items,
        total_pages,
        current_page: page,
        page_size: limit,
    };

    let mut result = std::collections::HashMap::new();
    result.insert(project.name.clone(), response);

    println!("Jobs | GET /jobs | project={} | count={} | res=200", project.name, total_items);

    Ok(Json(result))
}

#[derive(Serialize, ToSchema)]
pub struct PaginatedProjectJobsResponse {
    pub project_id: uuid::Uuid,
    pub jobs: Vec<JobResponse>,
    pub total_items: u64,
    pub total_pages: u64,
    pub current_page: u64,
    pub page_size: u64,
}



#[utoipa::path(
    get,
    path = "/admin/jobs",
    tag = "Jobs",
    params(
        ("status" = Option<String>, Query, description = "Filter by job status (pending, processing, completed, failed)"),
        ("page" = Option<u64>, Query, description = "Page number (default: 1)"),
        ("limit" = Option<u64>, Query, description = "Items per page (default: 10)")
    ),
    responses(
        (status = 200, description = "List of jobs grouped by project", body = std::collections::HashMap<String, PaginatedProjectJobsResponse>),
        (status = 401, description = "Unauthorized"),
        (status = 500, description = "Internal Server Error")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn list_admin_jobs(
    State(db): State<DatabaseConnection>,
    axum::Extension(user): axum::Extension<crate::middleware::auth::AuthUser>,
    Query(filter): Query<JobFilter>,
) -> Result<Json<std::collections::HashMap<String, PaginatedProjectJobsResponse>>, AppError> {
    use crate::entities::{project, user::Role};
    use sea_orm::QuerySelect;

    // 1. Fetch projects based on role
    let projects = match user.role {
        Role::Su => project::Entity::find().all(&db).await.map_err(AppError::DatabaseError)?,
        Role::Admin => project::Entity::find()
            .filter(project::Column::OwnerId.eq(user.id))
            .all(&db)
            .await
            .map_err(AppError::DatabaseError)?,
        Role::User => return Err(AppError::Unauthorized("Insufficient permissions".to_string())),
    };

    if projects.is_empty() {
        return Ok(Json(std::collections::HashMap::new()));
    }

    // 2. Fetch jobs for these projects
    let project_ids: Vec<uuid::Uuid> = projects.iter().map(|p| p.id).collect();
    
    let mut query = Job::find()
        .join(sea_orm::JoinType::InnerJoin, job::Relation::File.def())
        .join(sea_orm::JoinType::InnerJoin, file::Relation::Project.def())
        .filter(file::Column::ProjectId.is_in(project_ids))
        .order_by_desc(job::Column::CreatedAt)
        .select_also(file::Entity);

    if let Some(status) = &filter.status {
        query = query.filter(job::Column::Status.eq(status));
    }

    let jobs = query.all(&db).await.map_err(AppError::DatabaseError)?;

    // 3. Group and Paginate in memory
    let mut result: std::collections::HashMap<String, PaginatedProjectJobsResponse> = std::collections::HashMap::new();
    let mut project_jobs: std::collections::HashMap<uuid::Uuid, Vec<JobResponse>> = std::collections::HashMap::new();

    // Group jobs by project_id
    for (job_model, file_opt) in jobs {
        if let Some(file_model) = file_opt {
            project_jobs.entry(file_model.project_id).or_default().push(JobResponse::from(job_model));
        }
    }

    let page = filter.pagination.page.unwrap_or(1);
    let limit = filter.pagination.limit.unwrap_or(10);

    for p in projects {
        let all_jobs = project_jobs.remove(&p.id).unwrap_or_default();
        let total_items = all_jobs.len() as u64;
        let total_pages = (total_items as f64 / limit as f64).ceil() as u64;
        
        // Slice for pagination
        let start = ((page - 1) * limit) as usize;
        let end = std::cmp::min(start + limit as usize, all_jobs.len());
        
        let paginated_jobs = if start < all_jobs.len() {
            all_jobs[start..end].to_vec()
        } else {
            vec![]
        };

        result.insert(p.name, PaginatedProjectJobsResponse {
            project_id: p.id,
            jobs: paginated_jobs,
            total_items,
            total_pages,
            current_page: page,
            page_size: limit,
        });
    }

    println!("Jobs | GET /admin/jobs | user={} | projects={} | res=200", user.username, result.len());

    Ok(Json(result))
}

use axum::response::sse::{Event, Sse};
use tokio_stream::{wrappers::BroadcastStream, StreamExt};
use std::convert::Infallible;
use std::time::Duration;
use crate::services::broadcaster::Broadcaster;

#[utoipa::path(
    get,
    path = "/admin/jobs/events",
    tag = "Jobs",
    responses(
        (status = 200, description = "SSE event stream of job status updates", content_type = "text/event-stream")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
pub async fn job_events(
    axum::Extension(broadcaster): axum::Extension<Broadcaster>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, Infallible>>> {
    let rx = broadcaster.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|msg| {
        match msg {
            Ok(job) => {
                let data = serde_json::to_string(&job).ok()?;
                Some(Ok(Event::default().event("job_update").data(data)))
            }
            Err(_) => None,
        }
    });

    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
