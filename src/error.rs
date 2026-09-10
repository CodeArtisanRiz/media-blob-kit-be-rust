use axum::{
    http::StatusCode,
    response::{IntoResponse, Response, Json},
};
use serde_json::json;

#[derive(Debug)]
pub enum AppError {
    DatabaseError(sea_orm::DbErr),
    NotFound(String),
    Unauthorized(String),
    BadRequest(String),
    InternalServerError(String),
    Conflict(String),
    Forbidden(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::DatabaseError(e) => {
                tracing::error!("Database error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Unauthorized(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::InternalServerError(msg) => {
                tracing::error!("Internal server error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error".to_string())
            }
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg.clone()),
        };

        tracing::warn!(status = status.as_u16(), error = %error_message, "Application error");

        let body = Json(json!({
            "error": error_message,
        }));

        (status, body).into_response()
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::DatabaseError(e) => write!(f, "Database error: {}", e),
            AppError::NotFound(msg) => write!(f, "Not found: {}", msg),
            AppError::Unauthorized(msg) => write!(f, "Unauthorized: {}", msg),
            AppError::BadRequest(msg) => write!(f, "Bad request: {}", msg),
            AppError::InternalServerError(msg) => write!(f, "Internal server error: {}", msg),
            AppError::Conflict(msg) => write!(f, "Conflict: {}", msg),
            AppError::Forbidden(msg) => write!(f, "Forbidden: {}", msg),
        }
    }
}

impl From<sea_orm::DbErr> for AppError {
    fn from(err: sea_orm::DbErr) -> Self {
        AppError::DatabaseError(err)
    }
}
