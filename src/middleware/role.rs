use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use crate::entities::user::Role;
use crate::middleware::auth::AuthUser;

pub async fn require_su(
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_user = req
        .extensions()
        .get::<AuthUser>()
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if auth_user.role != Role::Su && auth_user.role != Role::Admin {
        eprintln!("Access denied: user '{}' is not superuser or admin", auth_user.username);
        return Err(StatusCode::FORBIDDEN);
    }

    Ok(next.run(req).await)
}
