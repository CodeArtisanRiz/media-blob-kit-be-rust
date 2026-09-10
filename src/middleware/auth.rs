use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use crate::config::get_config;
use uuid::Uuid;
use crate::entities::user;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: Uuid,
    pub username: String,
    pub role: user::Role,
}

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: usize,
    role: user::Role,
    user_id: Uuid,
}

pub async fn auth_middleware(
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract Authorization header
    let auth_header = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    // Check Bearer prefix
    if !auth_header.starts_with("Bearer ") {
        return Err(StatusCode::UNAUTHORIZED);
    }

    let token = &auth_header[7..]; // Remove "Bearer " prefix

    // Decode and validate JWT
    let token_data = decode::<Claims>(
        token,
        &DecodingKey::from_secret(get_config().jwt_secret.as_ref()),
        &Validation::default(),
    )
    .map_err(|e| {
        eprintln!("JWT decode error: {}", e);
        StatusCode::UNAUTHORIZED
    })?;

    // Create AuthUser from claims
    let auth_user = AuthUser {
        id: token_data.claims.user_id,
        username: token_data.claims.sub,
        role: token_data.claims.role,
    };

    // Insert auth user into request extensions
    req.extensions_mut().insert(auth_user);

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    #[test]
    fn test_jwt_claims_encode_decode() {
        let user_id = Uuid::new_v4();
        let claims = Claims {
            sub: "testuser".to_string(),
            exp: (chrono::Utc::now() + chrono::Duration::hours(1)).timestamp() as usize,
            role: user::Role::User,
            user_id,
        };

        let secret = "testsecret1234567890";
        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(secret.as_bytes()),
        ).expect("Failed to encode JWT");

        let decoded = decode::<Claims>(
            &token,
            &DecodingKey::from_secret(secret.as_bytes()),
            &Validation::default(),
        ).expect("Failed to decode JWT");

        assert_eq!(decoded.claims.sub, "testuser");
        assert_eq!(decoded.claims.user_id, user_id);
    }
}
