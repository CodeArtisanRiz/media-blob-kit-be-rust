use axum::{
    extract::State,
    response::Json,
};
use sea_orm::{ConnectionTrait, DatabaseConnection};
use serde::Serialize;

#[derive(Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    pub status: String,
}

#[utoipa::path(
    get,
    path = "/health",
    tag = "General",
    responses(
        (status = 200, description = "Health check", body = HealthResponse),
    )
)]
pub async fn health_check(
    State(db): State<DatabaseConnection>,
) -> Json<HealthResponse> {
    let status = match db
        .execute(sea_orm::Statement::from_string(
            db.get_database_backend(),
            "SELECT 1".to_owned(),
        ))
        .await
    {
        Ok(_) => "ok".to_string(),
        Err(_) => "degraded".to_string(),
    };

    Json(HealthResponse { status })
}
