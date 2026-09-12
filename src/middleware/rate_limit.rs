use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct RateLimiter {
    clients: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    max_requests: u32,
    window_duration: Duration,
}

impl RateLimiter {
    pub fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            max_requests,
            window_duration: Duration::from_secs(window_secs),
        }
    }

    pub async fn check(&self, client_key: &str) -> (bool, u32, u64) {
        let mut clients = self.clients.lock().await;
        let now = Instant::now();

        // Cleanup stale records periodically
        if clients.len() > 10_000 {
            clients.retain(|_, (_, timestamp)| now.duration_since(*timestamp) < self.window_duration);
        }

        let entry = clients.entry(client_key.to_string()).or_insert((0, now));

        if now.duration_since(entry.1) > self.window_duration {
            *entry = (1, now);
            let remaining = self.max_requests.saturating_sub(1);
            return (true, remaining, self.window_duration.as_secs());
        }

        if entry.0 < self.max_requests {
            entry.0 += 1;
            let remaining = self.max_requests.saturating_sub(entry.0);
            return (true, remaining, self.window_duration.as_secs());
        }

        let retry_after = self.window_duration.saturating_sub(now.duration_since(entry.1)).as_secs().max(1);
        (false, 0, retry_after)
    }
}

pub async fn rate_limit_middleware(
    axum::extract::State(limiter): axum::extract::State<RateLimiter>,
    headers: HeaderMap,
    request: Request,
    next: Next,
) -> Response {
    // Determine client identifier: x-forwarded-for > x-real-ip > x-api-key > authorization header > default
    let client_key = headers
        .get("x-forwarded-for")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_string())
        })
        .or_else(|| {
            headers
                .get("x-api-key")
                .and_then(|h| h.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "default_client".to_string());

    let (allowed, remaining, retry_after) = limiter.check(&client_key).await;

    if !allowed {
        let body = Json(serde_json::json!({
            "error": "Too Many Requests",
            "message": format!("Rate limit exceeded. Please retry after {} seconds.", retry_after)
        }));

        let mut response = (StatusCode::TOO_MANY_REQUESTS, body).into_response();
        response.headers_mut().insert("Retry-After", retry_after.to_string().parse().unwrap());
        response.headers_mut().insert("X-RateLimit-Limit", limiter.max_requests.to_string().parse().unwrap());
        response.headers_mut().insert("X-RateLimit-Remaining", "0".parse().unwrap());
        return response;
    }

    let mut response = next.run(request).await;
    response.headers_mut().insert("X-RateLimit-Limit", limiter.max_requests.to_string().parse().unwrap());
    response.headers_mut().insert("X-RateLimit-Remaining", remaining.to_string().parse().unwrap());
    response
}
