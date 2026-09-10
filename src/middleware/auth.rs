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
    // Extract Authorization header or query parameter token (for SSE/EventSource)
    let token = if let Some(auth_header) = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        if !auth_header.starts_with("Bearer ") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        auth_header[7..].to_string()
    } else if let Some(query_str) = req.uri().query() {
        // Fallback to ?token= query parameter (for browser EventSource connections)
        let token_param = query_str.split('&').find_map(|pair| {
            let mut parts = pair.split('=');
            if parts.next() == Some("token") {
                parts.next().map(|v| v.to_string())
            } else {
                None
            }
        });
        token_param.ok_or(StatusCode::UNAUTHORIZED)?
    } else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    // Decode and validate JWT
    let token_data = decode::<Claims>(
        &token,
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
