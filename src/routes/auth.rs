use axum::{
    extract::State,
    response::Json,
};
use sea_orm::{DatabaseConnection, EntityTrait, QueryFilter, ColumnTrait, ActiveModelTrait, Set, IntoActiveModel};
use serde::{Deserialize, Serialize};
use argon2::{
    password_hash::{PasswordHash, PasswordVerifier},
    Argon2,
};
use jsonwebtoken::{encode, EncodingKey, Header};
use crate::entities::{user::{self, Entity as User}, refresh_token::{self, Entity as RefreshToken}};
use sha2::{Sha256, Digest};
use base64::{Engine as _, engine::general_purpose};
use rand::Rng;
use uuid::Uuid;
use crate::error::AppError;

#[derive(Deserialize, utoipa::ToSchema)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct LoginResponse {
    access_token: String,
    refresh_token: String,
    expires_in: usize,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct RefreshRequest {
    refresh_token: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct RefreshResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(Deserialize, utoipa::ToSchema)]
pub struct LogoutRequest {
    refresh_token: String,
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct LogoutResponse {
    message: String,
}

use crate::config::get_config;

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    role: user::Role,
    user_id: Uuid,
}


fn generate_refresh_token() -> String {
    let mut random_bytes = [0u8; 32];
    rand::thread_rng().fill(&mut random_bytes);
    general_purpose::STANDARD.encode(random_bytes)
}

fn hash_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[utoipa::path(
    post,
    path = "/auth/login",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = LoginResponse),
        (status = 401, description = "Invalid credentials")
    ),
    tag = "Authentication"
)]
pub async fn login(
    State(db): State<DatabaseConnection>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {

    let user = User::find()
        .filter(user::Column::Username.eq(&payload.username))
        .one(&db)
        .await
        .map_err(|e| {
            println!("DB Error: {}", e);
            AppError::DatabaseError(e)
        })?;

    if let Some(user) = user {

        let parsed_hash = PasswordHash::new(&user.password).map_err(|e| {
            eprintln!("Password hash parse error: {}", e);
            AppError::InternalServerError("Password validation failed".to_string())
        })?;
        
        if Argon2::default()
            .verify_password(payload.password.as_bytes(), &parsed_hash)
            .is_ok()
        {

            let expiration = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
            
            let claims = Claims {
                sub: user.username.clone(),
                exp: expiration,
                role: user.role.clone(),
                user_id: user.id,
            };

            let config = get_config();
            let secret = config.jwt_secret.as_str();
            let access_token = encode(
                &Header::default(),
                &claims,
                &EncodingKey::from_secret(secret.as_bytes()),
            )
            .map_err(|e| {
                eprintln!("Token creation error: {}", e);
                AppError::InternalServerError("Token creation failed".to_string())
            })?;

            // Generate refresh token
            let refresh_token_str = generate_refresh_token();
            let refresh_token_hash = hash_token(&refresh_token_str);
            let expires_at = chrono::Utc::now() + chrono::Duration::days(1);

            let refresh_token = refresh_token::ActiveModel {
                id: Set(Uuid::new_v4()),
                user_id: Set(user.id),
                token_hash: Set(refresh_token_hash),
                expires_at: Set(expires_at.naive_utc()),
                created_at: Set(chrono::Utc::now().naive_utc()),
                revoked: Set(false),
            };

            refresh_token.insert(&db).await.map_err(|e| {
                eprintln!("Refresh token DB error: {}", e);
                AppError::DatabaseError(e)
            })?;

            println!("Auth | POST /auth/login | user={} | res=200", user.username);
            return Ok(Json(LoginResponse {
                access_token: access_token,
                refresh_token: refresh_token_str,
                expires_in: 3600,
            }));
        } else {
            println!("Auth | POST /auth/login | user={} | res=401 (invalid password)", user.username);
        }
    } else {
        println!("Auth | POST /auth/login | user={} | res=401 (not found)", payload.username);
    }

    Err(AppError::Unauthorized("Invalid credentials".to_string()))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorResponse {
    error: String,
}

#[utoipa::path(
    post,
    path = "/auth/refresh",
    request_body = RefreshRequest,
    responses(
        (status = 200, description = "Token refreshed successfully", body = RefreshResponse),
        (status = 401, description = "Invalid or expired refresh token", body = ErrorResponse)
    ),
    tag = "Authentication"
)]
pub async fn refresh(
    State(db): State<DatabaseConnection>,
    Json(payload): Json<RefreshRequest>,
) -> Result<Json<RefreshResponse>, AppError> {

    
    let token_hash = hash_token(&payload.refresh_token);
    
    // Find refresh token in database
    let refresh_token = RefreshToken::find()
        .filter(refresh_token::Column::TokenHash.eq(&token_hash))
        .one(&db)
        .await
        .map_err(|e| {
            println!("DB Error: {}", e);
            AppError::DatabaseError(e)
        })?
        .ok_or(AppError::Unauthorized("Invalid refresh token".to_string()))?;

    // Check if token is revoked
    if refresh_token.revoked {
        println!("Token is revoked");
        return Err(AppError::Unauthorized("User logged out. Please re-login.".to_string()));
    }

    // Check if token is expired
    let now = chrono::Utc::now().naive_utc();
    if refresh_token.expires_at < now {
        println!("Token is expired");
        return Err(AppError::Unauthorized("Refresh token expired. Please re-login.".to_string()));
    }

    // Get user details
    let user = User::find_by_id(refresh_token.user_id)
        .one(&db)
        .await
        .map_err(|e| {
            println!("User lookup error: {}", e);
            AppError::DatabaseError(e)
        })?
        .ok_or(AppError::Unauthorized("User not found. Please re-login.".to_string()))?;

    let user_id_val = user.id;

    // Mark token as used
    let mut active_token = refresh_token.into_active_model();
    active_token.revoked = Set(true);
    active_token.update(&db).await.map_err(|e| {
        eprintln!("DB Error: {}", e);
        AppError::DatabaseError(e)
    })?;

    // Generate new access token
    let expiration = (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize;
    let username = user.username.clone();

    let claims = Claims {
        sub: user.username,
        exp: expiration,
        role: user.role,
        user_id: user.id,
    };

    let config = get_config();
    let secret = config.jwt_secret.as_str();
    let token = encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes()))
        .map_err(|e| {
            println!("Token Encode Error: {}", e);
            AppError::InternalServerError("Failed to generate token".to_string())
        })?;

    // Generate new refresh token (rotation)
    let new_refresh_token_str = generate_refresh_token();
    let new_refresh_token_hash = hash_token(&new_refresh_token_str);
    let new_expires_at = chrono::Utc::now() + chrono::Duration::days(1);

    let new_refresh_token = refresh_token::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id_val),
        token_hash: Set(new_refresh_token_hash),
        expires_at: Set(new_expires_at.naive_utc()),
        created_at: Set(chrono::Utc::now().naive_utc()),
        revoked: Set(false),
    };

    new_refresh_token.insert(&db).await.map_err(|e| {
        eprintln!("Refresh token rotation DB error: {}", e);
        AppError::DatabaseError(e)
    })?;

    println!("Auth | POST /auth/refresh | user={} | res=200", username);
    Ok(Json(RefreshResponse { access_token: token, refresh_token: new_refresh_token_str }))
}

#[utoipa::path(
    post,
    path = "/auth/logout",
    request_body = LogoutRequest,
    responses(
        (status = 200, description = "Logged out successfully", body = LogoutResponse),
        (status = 404, description = "Refresh token not found")
    ),
    tag = "Authentication"
)]
pub async fn logout(
    State(db): State<DatabaseConnection>,
    Json(payload): Json<LogoutRequest>,
) -> Result<Json<LogoutResponse>, AppError> {
    let refresh_token_hash = hash_token(&payload.refresh_token);

    let refresh_token = RefreshToken::find()
        .filter(refresh_token::Column::TokenHash.eq(&refresh_token_hash))
        .one(&db)
        .await
        .map_err(|e| {
            eprintln!("DB Error: {}", e);
            AppError::DatabaseError(e)
        })?
        .ok_or(AppError::NotFound("Token not found".to_string()))?;

    let mut active_token = refresh_token.into_active_model();
    active_token.revoked = Set(true);
    active_token.update(&db).await.map_err(|e| {
        eprintln!("DB Error: {}", e);
        AppError::DatabaseError(e)
    })?;

    println!("Auth | POST /auth/logout | res=200");
    Ok(Json(LogoutResponse {
        message: "Logged out successfully".to_string(),
    }))
}

#[derive(Serialize, utoipa::ToSchema)]
pub struct UserProfile {
    #[schema(value_type = String)]
    id: Uuid,
    username: String,
    role: user::Role,
    created_at: chrono::NaiveDateTime,
}

#[utoipa::path(
    get,
    path = "/auth/me",
    responses(
        (status = 200, description = "User profile retrieved successfully", body = UserProfile),
        (status = 401, description = "Unauthorized - Invalid or missing token")
    ),
    security(
        ("bearer_auth" = [])
    ),
    tag = "Authentication"
)]
pub async fn me(
    State(db): State<DatabaseConnection>,
    auth_user: axum::Extension<crate::middleware::auth::AuthUser>,
) -> Result<Json<crate::routes::users::UserResponse>, AppError> {
    let user = User::find()
        .filter(user::Column::Username.eq(&auth_user.username))
        .one(&db)
        .await
        .map_err(|e| {
            eprintln!("DB Error: {}", e);
            AppError::DatabaseError(e)
        })?
        .ok_or(AppError::Unauthorized("User not found".to_string()))?;

    println!("Auth | GET /auth/me | user={} | res=200", user.username);
    Ok(Json(crate::routes::users::UserResponse::from(user)))
}
