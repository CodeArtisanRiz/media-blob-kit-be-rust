use axum::{
    extract::Request,
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::entities::api_key::{self, Entity as ApiKey};
use crate::entities::project::Entity as Project;
use crate::error::AppError;
use crate::models::settings::ProjectSettings;

#[derive(Clone, Debug)]
pub struct ProjectContext {
    pub id: Uuid,
    pub name: String,
    pub settings: ProjectSettings,
}

pub async fn api_key_auth(
    axum::extract::State(db): axum::extract::State<DatabaseConnection>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Result<Response, AppError> {
    let method = request.method().to_string();
    let uri = request.uri().to_string();

    let api_key_header = match headers.get("x-api-key") {
        Some(header) => header.to_str().map_err(|_| {
            println!("Auth | {} {} | res=401 | Invalid API Key format", method, uri);
            AppError::Unauthorized("Invalid API Key format".to_string())
        })?,
        None => {
            println!("Auth | {} {} | res=401 | Missing x-api-key header", method, uri);
            return Err(AppError::Unauthorized("Missing x-api-key header".to_string()));
        }
    };

    let mut hasher = Sha256::new();
    hasher.update(api_key_header.as_bytes());
    let key_hash = format!("{:x}", hasher.finalize());

    // Find API Key and related Project by SHA-256 hash
    let result = ApiKey::find()
        .filter(api_key::Column::KeyHash.eq(&key_hash))
        .find_also_related(Project)
        .one(&db)
        .await
        .map_err(AppError::DatabaseError)?;

    let (api_key, project) = match result {
        Some(r) => r,
        None => {
            println!("Auth | {} {} | res=401 | Invalid API Key", method, uri);
            return Err(AppError::Unauthorized("Invalid API Key".to_string()));
        }
    };

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

    Ok(next.run(request).await)
}
