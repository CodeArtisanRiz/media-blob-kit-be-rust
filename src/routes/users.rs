use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
};
use sea_orm::{
    DatabaseConnection, EntityTrait, ActiveModelTrait, Set, ModelTrait, PaginatorTrait,
    QueryOrder, IntoActiveModel,
};
use serde::{Deserialize, Serialize};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHasher, SaltString},
    Argon2,
};
use crate::entities::user::{self, Entity as User};
use crate::middleware::auth::AuthUser;
use uuid::Uuid;
use crate::pagination::{Pagination, PaginatedResponse};
use axum::extract::Query;
use crate::error::AppError;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub role: UserRole,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct UpdateUserRequest {
    pub password: Option<String>,
    pub role: Option<UserRole>,
}

#[derive(Deserialize, utoipa::ToSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum UserRole {
    Admin,
    User,
}

impl From<UserRole> for user::Role {
    fn from(role: UserRole) -> Self {
        match role {
            UserRole::Admin => user::Role::Admin,
            UserRole::User => user::Role::User,
        }
    }
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct UserResponse {
    #[schema(value_type = String)]
    pub id: Uuid,
    pub username: String,
    pub role: user::Role,
    pub created_at: chrono::NaiveDateTime,
}

impl From<user::Model> for UserResponse {
    fn from(user: user::Model) -> Self {
        UserResponse {
            id: user.id,
            username: user.username,
            role: user.role,
            created_at: user.created_at,
        }
    }
}

#[utoipa::path(
    post,
    path = "/users",
    request_body = CreateUserRequest,
    responses(
        (status = 201, description = "User created successfully", body = UserResponse),
        (status = 409, description = "Username already exists"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "User Management"
)]
pub async fn create_user(
    State(db): State<DatabaseConnection>,
    axum::Extension(auth_user): axum::Extension<AuthUser>,
    Json(payload): Json<CreateUserRequest>,
) -> Result<(StatusCode, Json<UserResponse>), AppError> {
    // Hash password
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let password_hash = argon2
        .hash_password(payload.password.as_bytes(), &salt)
        .map_err(|e| {
            eprintln!("Password hash error: {}", e);
            AppError::InternalServerError("Password hashing failed".to_string())
        })?
        .to_string();

    // Create user
    let user = user::ActiveModel {
        id: Set(Uuid::new_v4()),
        username: Set(payload.username),
        password: Set(password_hash),
        role: Set(payload.role.into()),
        created_at: Set(chrono::Utc::now().naive_utc()),
        ..Default::default()
    };

    match user.insert(&db).await {
        Ok(created_user) => {
            println!("User | POST /users | user={} | created={} | res=201", auth_user.username, created_user.username);
            Ok((StatusCode::CREATED, Json(UserResponse::from(created_user))))
        }
        Err(e) => {
            eprintln!("Failed to create user: {}", e);
            if e.to_string().contains("duplicate key value violates unique constraint") {
                return Err(AppError::Conflict("Username already exists".to_string()));
            }
            Err(AppError::DatabaseError(e))
        }
    }
}

#[utoipa::path(
    get,
    path = "/users",
    params(
        ("page" = Option<u64>, Query, description = "Page number"),
        ("limit" = Option<u64>, Query, description = "Items per page")
    ),
    responses(
        (status = 200, description = "List of all users", body = PaginatedResponse<UserResponse>),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "User Management"
)]
pub async fn list_users(
    State(db): State<DatabaseConnection>,
    _auth_user: axum::Extension<AuthUser>,
    Query(pagination): Query<Pagination>,
) -> Result<Json<PaginatedResponse<UserResponse>>, AppError> {
    let page = pagination.page.unwrap_or(1);
    let limit = pagination.limit.unwrap_or(10);

    let paginator = User::find()
        .order_by_desc(user::Column::CreatedAt)
        .paginate(&db, limit);

    let total_items = paginator.num_items().await.map_err(AppError::DatabaseError)?;
    let users = paginator.fetch_page(page - 1).await.map_err(AppError::DatabaseError)?;

    let user_responses: Vec<UserResponse> = users.into_iter().map(UserResponse::from).collect();
    
    println!("User | GET /users | user={} | count={} | res=200", _auth_user.username, total_items);
    Ok(Json(PaginatedResponse::new(user_responses, total_items, page, limit)))
}

#[utoipa::path(
    patch,
    path = "/users/{id}",
    params(
        ("id" = String, Path, description = "User ID to update")
    ),
    request_body = UpdateUserRequest,
    responses(
        (status = 200, description = "User updated successfully", body = UserResponse),
        (status = 404, description = "User not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "User Management"
)]
pub async fn update_user(
    State(db): State<DatabaseConnection>,
    axum::Extension(auth_user): axum::Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
    Json(payload): Json<UpdateUserRequest>,
) -> Result<Json<UserResponse>, AppError> {
    let user = User::find_by_id(user_id)
        .one(&db)
        .await
        .map_err(AppError::DatabaseError)?;

    let user = match user {
        Some(u) => u,
        None => return Err(AppError::NotFound("User not found".to_string())),
    };

    let mut active_user = user.into_active_model();

    if let Some(role) = payload.role {
        active_user.role = Set(role.into());
    }

    if let Some(pwd) = payload.password {
        if !pwd.trim().is_empty() {
            let salt = SaltString::generate(&mut OsRng);
            let argon2 = Argon2::default();
            let password_hash = argon2
                .hash_password(pwd.as_bytes(), &salt)
                .map_err(|e| {
                    eprintln!("Password hash error: {}", e);
                    AppError::InternalServerError("Password hashing failed".to_string())
                })?
                .to_string();
            active_user.password = Set(password_hash);
        }
    }

    let updated = active_user.update(&db).await.map_err(AppError::DatabaseError)?;
    println!("User | PATCH /users/{} | user={} | res=200", user_id, auth_user.username);
    Ok(Json(UserResponse::from(updated)))
}

#[utoipa::path(
    delete,
    path = "/users/{id}",
    params(
        ("id" = String, Path, description = "User ID to delete")
    ),
    responses(
        (status = 200, description = "User deleted successfully"),
        (status = 400, description = "Cannot delete yourself"),
        (status = 404, description = "User not found"),
        (status = 500, description = "Internal server error")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "User Management"
)]
pub async fn delete_user(
    State(db): State<DatabaseConnection>,
    axum::Extension(auth_user): axum::Extension<AuthUser>,
    Path(user_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Prevent deleting self
    if auth_user.id == user_id {
        println!("User | DELETE /users/{} | user={} | res=400 | Cannot delete yourself", user_id, auth_user.username);
        return Err(AppError::BadRequest("Cannot delete yourself".to_string()));
    }

    let user = User::find_by_id(user_id)
        .one(&db)
        .await?;
    
    match user {
        Some(user) => {
            user.delete(&db).await?;
            println!("User | DELETE /users/{} | user={} | res=200", user_id, auth_user.username);
            Ok(Json(serde_json::json!({
                "message": "User deleted successfully"
            })))
        }
        None => {
            println!("User | DELETE /users/{} | user={} | res=404 | User not found", user_id, auth_user.username);
            Err(AppError::NotFound("User not found".to_string()))
        }
    }
}
