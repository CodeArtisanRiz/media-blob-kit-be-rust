use axum::{
    extract::Request,
    http::{header, HeaderMap},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::get_config;
use crate::entities::api_key::{self, Entity as ApiKey};
use crate::entities::project::{self, Entity as Project};
use crate::entities::user;
use crate::error::AppError;
use crate::models::settings::ProjectSettings;

#[derive(Clone, Debug)]
pub struct ProjectContext {
    pub id: Uuid,
    pub name: String,
    pub settings: ProjectSettings,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    role: user::Role,
    user_id: Uuid,
}

pub async fn api_key_auth(
    axum::extract::State(db): axum::extract::State<DatabaseConnection>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();

    // 1. Check if raw API Key is provided via x-api-key, X-API-KEY, Authorization: Bearer mbk_..., or ?api_key= query
    let auth_header = headers.get(header::AUTHORIZATION).and_then(|h| h.to_str().ok());

    let raw_api_key = if let Some(header_val) = headers.get("x-api-key").and_then(|h| h.to_str().ok()) {
        Some(header_val.to_string())
    } else if let Some(auth) = auth_header {
        if auth.starts_with("Bearer mbk_") {
            Some(auth[7..].to_string())
        } else if auth.starts_with("mbk_") {
            Some(auth.to_string())
        } else {
            None
        }
    } else if let Some(query_str) = request.uri().query() {
        query_str.split('&').find_map(|pair| {
            let mut parts = pair.split('=');
            if parts.next() == Some("api_key") || parts.next() == Some("key") {
                parts.next().map(|v| v.to_string())
            } else {
                None
            }
        })
    } else {
        None
    };

    if let Some(key_str) = raw_api_key {
        let mut hasher = Sha256::new();
        hasher.update(key_str.as_bytes());
        let key_hash = format!("{:x}", hasher.finalize());

        let result = ApiKey::find()
            .filter(api_key::Column::KeyHash.eq(&key_hash))
            .find_also_related(Project)
            .one(&db)
            .await
            .map_err(AppError::DatabaseError)?;

        if let Some((api_key, project)) = result {
            let project = match project {
                Some(p) => p,
                None => {
                    println!("Auth | {} {} | res=500 | Orphaned API Key", method, uri);
                    return Err(AppError::InternalServerError("Orphaned API Key".to_string()));
                }
            };

            if !api_key.is_active {
                println!("Auth | {} {} | project={} | res=401 | API Key is inactive", method, uri, project.name);
                return Err(AppError::Unauthorized("API Key is inactive".to_string()));
            }

            if let Some(expires_at) = api_key.expires_at {
                if expires_at < chrono::Utc::now().naive_utc() {
                    println!("Auth | {} {} | project={} | res=401 | API Key has expired", method, uri, project.name);
                    return Err(AppError::Unauthorized("API Key has expired".to_string()));
                }
            }

            let settings: ProjectSettings = serde_json::from_value(project.settings.clone())
                .map_err(|e| {
                    eprintln!("Failed to parse project settings: {}", e);
                    e
                })
                .unwrap_or_default();

            request.extensions_mut().insert(ProjectContext {
                id: project.id,
                name: project.name,
                settings,
            });

            return Ok(next.run(request).await);
        }
    }

    // 2. Check if request is authenticated via Bearer JWT (Dashboard Admin / User Upload)
    let jwt_token = if let Some(auth) = auth_header {
        if auth.starts_with("Bearer ") && !auth.starts_with("Bearer mbk_") {
            Some(auth[7..].to_string())
        } else {
            None
        }
    } else if let Some(query_str) = request.uri().query() {
        query_str.split('&').find_map(|pair| {
            let mut parts = pair.split('=');
            if parts.next() == Some("token") {
                parts.next().map(|v| v.to_string())
            } else {
                None
            }
        })
    } else {
        None
    };

    if let Some(token) = jwt_token {
        let token_data = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(get_config().jwt_secret.as_ref()),
            &Validation::default(),
        )
        .map_err(|e| {
            eprintln!("JWT decode error: {}", e);
            AppError::Unauthorized("Invalid or expired session token".to_string())
        })?;

        let user_id = token_data.claims.user_id;
        let user_role = token_data.claims.role;

        // Check for project_id or api_key_id in headers or query
        let query_str = request.uri().query().unwrap_or("");
        
        let target_project_id = headers.get("x-project-id")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| Uuid::parse_str(s).ok())
            .or_else(|| {
                query_str.split('&').find_map(|pair| {
                    let mut parts = pair.split('=');
                    if parts.next() == Some("project_id") {
                        parts.next().and_then(|v| Uuid::parse_str(v).ok())
                    } else {
                        None
                    }
                })
            });

        let target_key_id = headers.get("x-api-key-id")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| Uuid::parse_str(s).ok())
            .or_else(|| {
                query_str.split('&').find_map(|pair| {
                    let mut parts = pair.split('=');
                    if parts.next() == Some("api_key_id") || parts.next() == Some("key_id") {
                        parts.next().and_then(|v| Uuid::parse_str(v).ok())
                    } else {
                        None
                    }
                })
            });

        let found_project = if let Some(key_id) = target_key_id {
            let key_match = ApiKey::find()
                .filter(api_key::Column::Id.eq(key_id))
                .find_also_related(Project)
                .one(&db)
                .await
                .map_err(AppError::DatabaseError)?;

            if let Some((_, Some(proj))) = key_match {
                if user_role == user::Role::Su || proj.owner_id == user_id {
                    Some(proj)
                } else {
                    return Err(AppError::Unauthorized("Access denied to project key".to_string()));
                }
            } else {
                None
            }
        } else if let Some(proj_id) = target_project_id {
            let mut query = Project::find_by_id(proj_id)
                .filter(project::Column::DeletedAt.is_null());

            if user_role != user::Role::Su {
                query = query.filter(project::Column::OwnerId.eq(user_id));
            }

            query.one(&db).await.map_err(AppError::DatabaseError)?
        } else {
            // Default to user's first active project if not explicitly specified
            let mut query = Project::find()
                .filter(project::Column::DeletedAt.is_null());

            if user_role != user::Role::Su {
                query = query.filter(project::Column::OwnerId.eq(user_id));
            }

            query.one(&db).await.map_err(AppError::DatabaseError)?
        };

        if let Some(project) = found_project {
            let settings: ProjectSettings = serde_json::from_value(project.settings.clone())
                .map_err(|e| {
                    eprintln!("Failed to parse project settings: {}", e);
                    e
                })
                .unwrap_or_default();

            request.extensions_mut().insert(ProjectContext {
                id: project.id,
                name: project.name,
                settings,
            });

            return Ok(next.run(request).await);
        } else {
            println!("Auth | {} {} | user={} | res=404 | No matching project found for user", method, uri, user_id);
            return Err(AppError::NotFound("Project not found or access denied".to_string()));
        }
    }

    println!("Auth | {} {} | res=401 | Missing API Key or Bearer token", method, uri);
    Err(AppError::Unauthorized("Missing API Key or Bearer authorization header".to_string()))
}
