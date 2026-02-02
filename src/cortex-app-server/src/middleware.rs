//! HTTP middleware components.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::time::timeout;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::state::AppState;

/// Request ID header name.
pub const REQUEST_ID_HEADER: &str = "X-Request-Id";

/// Request timing header name.
pub const REQUEST_TIMING_HEADER: &str = "X-Response-Time";

/// Request ID middleware - adds unique ID to each request.
pub async fn request_id_middleware(mut request: Request, next: Next) -> Response {
    // Get or generate request ID
    let request_id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(String::from)
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    // Add request ID to extensions
    request
        .extensions_mut()
        .insert(RequestId(request_id.clone()));

    // Add to response headers
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        REQUEST_ID_HEADER,
        HeaderValue::from_str(&request_id).unwrap(),
    );

    response
}

/// Request ID type.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Timing middleware - tracks request duration.
pub async fn timing_middleware(request: Request, next: Next) -> Response {
    let start = Instant::now();
    let method = request.method().clone();
    let uri = request.uri().clone();

    let mut response = next.run(request).await;

    let duration = start.elapsed();
    let duration_ms = duration.as_secs_f64() * 1000.0;

    // Add timing header
    if let Ok(value) = HeaderValue::from_str(&format!("{duration_ms:.2}ms")) {
        response.headers_mut().insert(REQUEST_TIMING_HEADER, value);
    }

    // Log request
    let status = response.status();
    if status.is_success() {
        info!(
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = %format!("{:.2}", duration_ms),
            "Request completed"
        );
    } else if status.is_client_error() {
        warn!(
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = %format!("{:.2}", duration_ms),
            "Client error"
        );
    } else {
        error!(
            method = %method,
            uri = %uri,
            status = %status,
            duration_ms = %format!("{:.2}", duration_ms),
            "Server error"
        );
    }

    response
}

/// Rate limiting middleware.
///
/// Issue #2321: This middleware consistently returns HTTP 429 Too Many Requests
/// for all rate limiting scenarios. Previous inconsistency with 503 has been fixed.
///
/// Response behavior:
/// - Returns 429 Too Many Requests when rate limit is exceeded
/// - Includes Retry-After header (60 seconds) to help clients implement backoff
/// - Never returns 503 for rate limiting (503 is reserved for service unavailability)
pub async fn rate_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Skip rate limiting if disabled
    if !state.config.rate_limit.enabled {
        return Ok(next.run(request).await);
    }

    // Get rate limit key
    let key = get_rate_limit_key(&request, &state);

    // Check if exempt path
    let path = request.uri().path();
    if state
        .config
        .rate_limit
        .exempt_paths
        .iter()
        .any(|p| path.starts_with(p))
    {
        return Ok(next.run(request).await);
    }

    // Check rate limit - Issue #2321: Always return 429, never 503
    match state.check_rate_limit(&key).await {
        Ok(()) => Ok(next.run(request).await),
        Err(_) => {
            // Issue #2321: Consistently return 429 Too Many Requests
            // with Retry-After header for proper client retry logic
            let mut response = StatusCode::TOO_MANY_REQUESTS.into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("60"));
            Ok(response)
        }
    }
}

/// Get rate limit key from request.
fn get_rate_limit_key(request: &Request, state: &AppState) -> String {
    // Try API key first
    if state.config.rate_limit.by_api_key
        && let Some(api_key) = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("ApiKey "))
    {
        return format!("apikey:{api_key}");
    }

    // Fall back to IP address
    // When trust_proxy is enabled, check proxy headers for real client IP
    if state.config.rate_limit.trust_proxy {
        // Try X-Real-IP first (single IP from proxy)
        if let Some(real_ip) = request.headers().get("X-Real-IP")
            && let Ok(ip) = real_ip.to_str()
        {
            return format!("ip:{}", ip.trim());
        }

        // Then try X-Forwarded-For (may contain multiple IPs, take the first)
        if let Some(forwarded) = request.headers().get("X-Forwarded-For")
            && let Ok(s) = forwarded.to_str()
            && let Some(ip) = s.split(',').next()
        {
            return format!("ip:{}", ip.trim());
        }
    }

    // Default to unknown when not behind proxy or headers not present
    "ip:unknown".to_string()
}

/// Timeout middleware.
pub async fn timeout_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let timeout_duration = state.config.request_timeout_duration();

    match timeout(timeout_duration, next.run(request)).await {
        Ok(response) => Ok(response),
        Err(_) => {
            error!("Request timed out after {:?}", timeout_duration);
            Err(StatusCode::GATEWAY_TIMEOUT)
        }
    }
}

/// Security headers middleware.
pub async fn security_headers_middleware(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();

    // Add security headers
    headers.insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("X-Frame-Options", HeaderValue::from_static("DENY"));
    headers.insert(
        "X-XSS-Protection",
        HeaderValue::from_static("1; mode=block"),
    );
    headers.insert(
        "Referrer-Policy",
        HeaderValue::from_static("strict-origin-when-cross-origin"),
    );

    response
}

/// Content type validation middleware.
pub async fn content_type_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    // Only validate POST/PUT/PATCH requests
    if matches!(
        request.method(),
        &Method::POST | &Method::PUT | &Method::PATCH
    ) {
        let content_type = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Allow JSON and multipart
        if !content_type.starts_with("application/json")
            && !content_type.starts_with("multipart/form-data")
            && !content_type.is_empty()
        {
            warn!("Unsupported content type: {}", content_type);
            return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
    }

    Ok(next.run(request).await)
}

/// Request body size limit middleware.
pub async fn body_limit_middleware(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let max_size = state.config.max_body_size;

    // Check content-length header
    if let Some(content_length) = request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        && content_length > max_size
    {
        warn!(
            "Request body too large: {} bytes (max: {})",
            content_length, max_size
        );
        return Err(StatusCode::PAYLOAD_TOO_LARGE);
    }

    Ok(next.run(request).await)
}

/// CORS configuration.
/// Includes Access-Control-Max-Age header to allow browsers to cache
/// preflight responses, reducing the number of OPTIONS requests.
pub fn cors_layer(origins: &[String]) -> tower_http::cors::CorsLayer {
    use tower_http::cors::{Any, CorsLayer};

    // Default max age for preflight cache: 24 hours (86400 seconds)
    // This reduces the number of OPTIONS preflight requests from browsers
    let max_age = std::time::Duration::from_secs(86400);

    if origins.is_empty() {
        CorsLayer::permissive().max_age(max_age)
    } else {
        let origins: Vec<HeaderValue> = origins
            .iter()
            .filter_map(|o| HeaderValue::from_str(o).ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
            .max_age(max_age)
    }
}

/// Compression configuration.
pub fn compression_layer() -> tower_http::compression::CompressionLayer {
    tower_http::compression::CompressionLayer::new()
}

/// Request logging configuration.
#[derive(Debug, Clone)]
pub struct RequestLogging {
    /// Log request headers.
    pub headers: bool,
    /// Log request body.
    pub body: bool,
    /// Log response body.
    pub response_body: bool,
    /// Maximum body size to log.
    pub max_body_size: usize,
    /// Headers to redact.
    pub redact_headers: Vec<String>,
}

impl Default for RequestLogging {
    fn default() -> Self {
        Self {
            headers: true,
            body: false,
            response_body: false,
            max_body_size: 4096,
            redact_headers: vec![
                "Authorization".to_string(),
                "Cookie".to_string(),
                "X-Api-Key".to_string(),
            ],
        }
    }
}

/// Error handling middleware.
pub async fn error_handling_middleware(request: Request, next: Next) -> Response {
    let response = next.run(request).await;

    // Add error details for error responses
    if response.status().is_server_error() {
        error!("Server error: {}", response.status());
    }

    response
}

/// Content negotiation middleware.
///
/// This middleware validates the Accept header and returns 406 Not Acceptable
/// if the client requests an unsupported content type. The API only supports
/// JSON responses.
///
/// Supported content types:
/// - `application/json`
/// - `*/*` (wildcard)
/// - No Accept header (defaults to JSON)
pub async fn content_negotiation_middleware(
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Get Accept header
    if let Some(accept) = request.headers().get(header::ACCEPT)
        && let Ok(accept_str) = accept.to_str()
    {
        // Parse Accept header and check for supported types
        let supported = accept_str.split(',').any(|media_type| {
            let media_type = media_type.split(';').next().unwrap_or("").trim();
            media_type == "application/json"
                || media_type == "application/*"
                || media_type == "*/*"
                || media_type.is_empty()
        });

        if !supported {
            warn!(
                "Unsupported Accept header: {}. Only application/json is supported.",
                accept_str
            );
            return Err(StatusCode::NOT_ACCEPTABLE);
        }
    }

    Ok(next.run(request).await)
}

/// Request context extracted from middleware.
#[derive(Debug, Clone)]
pub struct RequestContext {
    /// Request ID.
    pub request_id: String,
    /// Client IP address.
    pub client_ip: Option<String>,
    /// User agent.
    pub user_agent: Option<String>,
    /// Request start time.
    pub start_time: Instant,
}

impl RequestContext {
    /// Create from request.
    pub fn from_request(request: &Request) -> Self {
        let request_id = request
            .extensions()
            .get::<RequestId>()
            .map(|r| r.0.clone())
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Try to get client IP from proxy headers (X-Real-IP first, then X-Forwarded-For)
        let client_ip = request
            .headers()
            .get("X-Real-IP")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .or_else(|| {
                request
                    .headers()
                    .get("X-Forwarded-For")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.split(',').next())
                    .map(|s| s.trim().to_string())
            });

        let user_agent = request
            .headers()
            .get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        Self {
            request_id,
            client_ip,
            user_agent,
            start_time: Instant::now(),
        }
    }

    /// Get request duration.
    pub fn duration(&self) -> Duration {
        self.start_time.elapsed()
    }
}

/// Health check bypass middleware - skip middleware for health endpoints.
pub async fn health_check_bypass_middleware(request: Request, next: Next) -> Response {
    let path = request.uri().path();

    // Fast path for health checks
    if path == "/health" || path == "/api/v1/health" {
        return next.run(request).await;
    }

    // Normal processing
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id() {
        let id = RequestId(Uuid::new_v4().to_string());
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_request_logging_default() {
        let logging = RequestLogging::default();
        assert!(logging.headers);
        assert!(!logging.body);
        assert!(
            logging
                .redact_headers
                .contains(&"Authorization".to_string())
        );
    }
}
