mod home;
mod auth;
mod users;
mod projects;
mod api_keys;
pub mod upload;
mod jobs;
mod files;

use axum::{
    routing::{get, post, delete},
    Router,
    middleware,
};
use sea_orm::DatabaseConnection;
use crate::middleware::auth::auth_middleware;
use crate::middleware::role::require_su;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

// Define the OpenAPI documentation
#[derive(OpenApi)]
#[openapi(
    paths(
        // General endpoints
        home::root,
        // Authentication endpoints
        auth::login,
        auth::refresh,
        auth::logout,
        auth::me,
        // User management endpoints
        users::create_user,
        users::list_users,
        users::delete_user,
        // Project management endpoints
        projects::create_project,
        projects::list_projects,
        projects::get_project,
        projects::update_project,
        projects::delete_project,
        projects::sync_variants,
        // API Key endpoints
        api_keys::create_api_key,
        api_keys::list_api_keys,
        api_keys::update_api_key,
        api_keys::delete_api_key,
        // Upload endpoints
        upload::upload_file,
        upload::upload_image,
        // Jobs endpoints
        jobs::list_jobs,
        jobs::list_admin_jobs,
        // File endpoints
        files::list_files,
        files::get_file,
        files::get_file_content,
        files::delete_file,
    ),
    components(
        schemas(
            // Home schemas
            home::RootResponse,
            // Auth schemas
            auth::LoginRequest,
            auth::LoginResponse,
            auth::RefreshRequest,
            auth::RefreshResponse,
            auth::LogoutRequest,
            auth::LogoutResponse,
            auth::ErrorResponse,
            auth::UserProfile,
            // User schemas
            users::CreateUserRequest,
            users::UserResponse,
            users::UserRole,
            crate::entities::user::Role,
            // Project schemas
            projects::CreateProjectRequest,
            projects::UpdateProjectRequest,
            projects::ProjectResponse,
            // API Key schemas
            api_keys::CreateApiKeyRequest,
            api_keys::UpdateApiKeyRequest,
            api_keys::ApiKeyResponse,
            // Upload schemas
            upload::FileUploadResponse,
            upload::ImageUploadResponse,
            // Job schemas
            jobs::JobResponse,
            jobs::JobResponse,
        jobs::PaginatedProjectJobsResponse,
        // File schemas
        files::FileResponse,
        )
    ),
    tags(
        (name = "General", description = "General API information"),
        (name = "Authentication", description = "Authentication endpoints for login, token refresh, and logout"),
        (name = "User Management", description = "User management endpoints (superuser access required)"),
        (name = "Project Management", description = "Project management endpoints"),
        (name = "Project API Keys", description = "API Key management endpoints"),
        (name = "File Upload", description = "File and Image upload endpoints"),
        (name = "File Management", description = "File retrieval and serving endpoints"),
        (name = "Jobs", description = "Background job management endpoints")
    ),
    info(
        title = "MediaBlobKit API",
        version = "0.1.0",
        description = "A Rust/Axum application for media blob management with user authentication and role-based access control",
    ),
    modifiers(&SecurityAddon)
)]
struct ApiDoc;

// Add security scheme for JWT Bearer tokens
struct SecurityAddon;

impl utoipa::Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.as_mut().unwrap();
        components.add_security_scheme(
            "bearer_auth",
            utoipa::openapi::security::SecurityScheme::Http(
                utoipa::openapi::security::Http::new(
                    utoipa::openapi::security::HttpAuthScheme::Bearer
                )
            ),
        );
        components.add_security_scheme(
            "api_key",
            utoipa::openapi::security::SecurityScheme::ApiKey(
                utoipa::openapi::security::ApiKey::Header(
                    utoipa::openapi::security::ApiKeyValue::new("x-api-key")
                )
            ),
        );
    }
}

pub fn create_routes(db: DatabaseConnection) -> Router {
    // Swagger UI (stateless)  
    let swagger_router: Router = SwaggerUi::new("/swagger-ui")
        .url("/api-docs/openapi.json", ApiDoc::openapi())
        .into();

    // Protected routes that require auth
    let protected_routes = Router::new()
        .route("/auth/me", get(auth::me))
        .route("/projects", post(projects::create_project))
        .route("/projects", get(projects::list_projects))
        .route("/projects/{id}", get(projects::get_project))
        .route("/projects/{id}", axum::routing::put(projects::update_project))
        .route("/projects/{id}", delete(projects::delete_project))
        .route("/projects/{id}/sync-variants", post(projects::sync_variants))
        .route("/projects/{id}/keys", post(api_keys::create_api_key))
        .route("/projects/{id}/keys", get(api_keys::list_api_keys))
        .route("/projects/{id}/keys/{key_id}", axum::routing::patch(api_keys::update_api_key))
        .route("/projects/{id}/keys/{key_id}", delete(api_keys::delete_api_key))
        .route("/admin/jobs", get(jobs::list_admin_jobs))
        .route("/files", get(files::list_files))
        .route("/files/{id}", get(files::get_file).delete(files::delete_file))
        .route("/files/{id}/content", get(files::get_file_content))
        .layer(middleware::from_fn(auth_middleware));

    // Su-only routes
    let su_routes = Router::new()
        .route("/users", post(users::create_user))
        .route("/users", get(users::list_users))
        .route("/users/{id}", delete(users::delete_user))
        .layer(middleware::from_fn(require_su))
        .layer(middleware::from_fn(auth_middleware));

    // Public routes (no auth required) and merge all together
    let app_routes = Router::new()
        .route("/", get(home::root))
        .route("/favicon.ico", get(|| async { axum::http::StatusCode::NO_CONTENT }))
        .route("/auth/login", post(auth::login))
        .route("/auth/refresh", post(auth::refresh))
        .route("/auth/logout", post(auth::logout))
        .merge(protected_routes)
        .merge(su_routes)
        .merge(
            Router::new()
                .route("/upload/file", post(upload::upload_file))
                .route("/upload/image", post(upload::upload_image))
                .route("/jobs", get(jobs::list_jobs))
                .layer(axum::middleware::from_fn_with_state(db.clone(), crate::middleware::api_key::api_key_auth))
        )
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(db);
    
    // Merge Swagger UI (which has no state) with the rest
    Router::new()
        .merge(swagger_router)
        .merge(app_routes)
}
