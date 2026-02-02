//! API client utilities.
//!
//! Provides utilities for making HTTP API requests
//! with retry, rate limiting, and error handling.
//!
//! # Global HTTP Client Factory
//!
//! Use the factory functions to create HTTP clients with consistent configuration:
//! - `create_default_client()` - Standard 30s timeout
//! - `create_streaming_client()` - 5min timeout for LLM streaming
//! - `create_client_with_timeout(duration)` - Custom timeout
//!
//! All clients include: User-Agent, tcp_nodelay, and proper error handling.
//!
//! Note: This module re-exports and wraps functions from `cortex_common::http_client`
//! with appropriate error types for cortex-engine.

use std::collections::HashMap;
use std::time::Duration;

use reqwest::{Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::error::{CortexError, Result};

// Re-export constants from cortex-common
pub use cortex_common::http_client::{
    DEFAULT_TIMEOUT, HEALTH_CHECK_TIMEOUT, STREAMING_TIMEOUT, USER_AGENT,
};

/// Default connection timeout (10 seconds)
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

// ============================================================================
// Global HTTP Client Factory Functions (wrapping cortex-common)
// ============================================================================

/// Creates an HTTP client with default configuration (30s timeout).
///
/// Includes: User-Agent, tcp_nodelay, 30s timeout.
pub fn create_default_client() -> Result<Client> {
    cortex_common::http_client::create_default_client().map_err(|e| CortexError::Internal(e))
}

/// Creates an HTTP client for LLM streaming (5min timeout).
///
/// Use this for endpoints that stream responses (SSE, chunked transfer).
pub fn create_streaming_client() -> Result<Client> {
    cortex_common::http_client::create_streaming_client().map_err(|e| CortexError::Internal(e))
}

/// Creates an HTTP client for health checks (5s timeout).
pub fn create_health_check_client() -> Result<Client> {
    cortex_common::http_client::create_health_check_client().map_err(|e| CortexError::Internal(e))
}

/// Creates an HTTP client with a custom timeout.
///
/// All clients include:
/// - User-Agent: `cortex-cli/{version}`
/// - tcp_nodelay: true (for lower latency)
/// - Specified timeout (applies to both connect and overall request)
pub fn create_client_with_timeout(timeout: Duration) -> Result<Client> {
    cortex_common::http_client::create_client_with_timeout(timeout)
        .map_err(|e| CortexError::Internal(e))
}

/// Creates an HTTP client with separate connect and response timeouts.
///
/// This allows distinguishing between connection establishment and response waiting.
/// Use this when you need different timeouts for initial connection vs response.
///
/// # Arguments
/// * `connect_timeout` - Timeout for establishing TCP connection
/// * `response_timeout` - Total timeout for the entire request/response cycle
pub fn create_client_with_timeouts(
    connect_timeout: Duration,
    response_timeout: Duration,
) -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(connect_timeout)
        .timeout(response_timeout)
        .tcp_nodelay(true)
        .build()
        .map_err(|e| CortexError::Internal(format!("Failed to build HTTP client: {e}")))
}

/// Creates an HTTP client builder with standard configuration.
///
/// Use this when you need to customize the client further before building.
/// Note: The timeout is applied to both the TCP connection phase and the overall request.
pub fn create_client_builder() -> reqwest::ClientBuilder {
    cortex_common::http_client::create_client_builder()
}

// ============================================================================
// Blocking Client Factory (wrapping cortex-common)
// ============================================================================

/// Creates a blocking HTTP client with default configuration.
pub fn create_blocking_client() -> Result<reqwest::blocking::Client> {
    cortex_common::http_client::create_blocking_client().map_err(|e| CortexError::Internal(e))
}

/// Creates a blocking HTTP client with custom timeout.
pub fn create_blocking_client_with_timeout(timeout: Duration) -> Result<reqwest::blocking::Client> {
    cortex_common::http_client::create_blocking_client_with_timeout(timeout)
        .map_err(|e| CortexError::Internal(e))
}

// ============================================================================
// Legacy ApiClientConfig (kept for backward compatibility)
// ============================================================================

/// API client configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiClientConfig {
    /// Base URL.
    pub base_url: String,
    /// Request timeout.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Max retries.
    #[serde(default = "default_retries")]
    pub max_retries: u32,
    /// Retry delay.
    #[serde(default = "default_retry_delay")]
    pub retry_delay_ms: u64,
    /// Default headers.
    #[serde(default)]
    pub default_headers: HashMap<String, String>,
    /// User agent.
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
}

fn default_timeout() -> u64 {
    30
}
fn default_retries() -> u32 {
    3
}
fn default_retry_delay() -> u64 {
    1000
}
fn default_user_agent() -> String {
    USER_AGENT.to_string()
}

impl Default for ApiClientConfig {
    fn default() -> Self {
        Self {
            base_url: String::new(),
            timeout_secs: default_timeout(),
            max_retries: default_retries(),
            retry_delay_ms: default_retry_delay(),
            default_headers: HashMap::new(),
            user_agent: default_user_agent(),
        }
    }
}

/// API client for making HTTP requests.
pub struct ApiClient {
    client: Client,
    config: ApiClientConfig,
}

impl ApiClient {
    /// Create a new client.
    pub fn new(config: ApiClientConfig) -> Result<Self> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent(&config.user_agent);

        // Add default headers
        let mut headers = reqwest::header::HeaderMap::new();
        for (key, value) in &config.default_headers {
            headers.insert(
                reqwest::header::HeaderName::try_from(key.as_str())
                    .map_err(|e| CortexError::InvalidInput(e.to_string()))?,
                value
                    .parse()
                    .map_err(|e: reqwest::header::InvalidHeaderValue| {
                        CortexError::InvalidInput(e.to_string())
                    })?,
            );
        }
        builder = builder.default_headers(headers);

        let client = builder
            .build()
            .map_err(|e| CortexError::Internal(format!("Failed to build client: {e}")))?;

        Ok(Self { client, config })
    }

    /// Make a GET request.
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::GET, path, None::<()>).await
    }

    /// Make a POST request.
    pub async fn post<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: B) -> Result<T> {
        self.request(Method::POST, path, Some(body)).await
    }

    /// Make a PUT request.
    pub async fn put<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: B) -> Result<T> {
        self.request(Method::PUT, path, Some(body)).await
    }

    /// Make a DELETE request.
    pub async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::DELETE, path, None::<()>).await
    }

    /// Make a PATCH request.
    pub async fn patch<T: DeserializeOwned, B: Serialize>(&self, path: &str, body: B) -> Result<T> {
        self.request(Method::PATCH, path, Some(body)).await
    }

    /// Make a request with retry.
    async fn request<T: DeserializeOwned, B: Serialize>(
        &self,
        method: Method,
        path: &str,
        body: Option<B>,
    ) -> Result<T> {
        let url = format!("{}{}", self.config.base_url, path);
        let mut last_error = None;

        for attempt in 0..=self.config.max_retries {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(
                    self.config.retry_delay_ms * (1 << (attempt - 1)),
                ))
                .await;
            }

            match self.execute_request(&method, &url, &body).await {
                Ok(response) => {
                    let status = response.status();

                    if status.is_success() {
                        let result = response
                            .json::<T>()
                            .await
                            .map_err(|e| CortexError::Internal(e.to_string()))?;
                        return Ok(result);
                    }

                    // Check if retryable (including HTTP 429 with Retry-After)
                    if Self::is_retryable_status(status) && attempt < self.config.max_retries {
                        // Parse Retry-After header for HTTP 429 responses (#2745)
                        let retry_after_secs = if status == StatusCode::TOO_MANY_REQUESTS {
                            Self::parse_retry_after_header(&response)
                        } else {
                            None
                        };

                        last_error = if let Some(secs) = retry_after_secs {
                            Some(CortexError::RateLimitWithRetryAfter {
                                message: format!("HTTP {status}"),
                                retry_after_secs: secs,
                            })
                        } else {
                            Some(CortexError::ConnectionFailed {
                                endpoint: url.clone(),
                                message: format!("HTTP {status}"),
                            })
                        };
                        continue;
                    }

                    // For non-retryable responses, still check for rate limit info
                    if status == StatusCode::TOO_MANY_REQUESTS {
                        let retry_after = Self::parse_retry_after_header(&response);
                        let error_body = response.text().await.unwrap_or_default();
                        return Err(if let Some(secs) = retry_after {
                            CortexError::RateLimitWithRetryAfter {
                                message: format!("HTTP {status}: {error_body}"),
                                retry_after_secs: secs,
                            }
                        } else {
                            CortexError::RateLimit(format!("HTTP {status}: {error_body}"))
                        });
                    }

                    let error_body = response.text().await.unwrap_or_default();
                    return Err(CortexError::ConnectionFailed {
                        endpoint: url.clone(),
                        message: format!("HTTP {status}: {error_body}"),
                    });
                }
                Err(e) if attempt < self.config.max_retries && Self::is_retryable_error(&e) => {
                    tracing::debug!(
                        attempt = attempt,
                        error = %e,
                        "Retrying request after transient error (possibly HTTP/2 GOAWAY)"
                    );
                    last_error = Some(CortexError::ConnectionFailed {
                        endpoint: url.clone(),
                        message: e.to_string(),
                    });
                }
                Err(e) => return Err(CortexError::from_reqwest_with_proxy_check(e, &url)),
            }
        }

        Err(last_error.unwrap_or_else(|| CortexError::ConnectionFailed {
            endpoint: url.clone(),
            message: "Request failed".to_string(),
        }))
    }

    /// Execute a single request.
    /// Returns the raw reqwest error for better error handling at the call site.
    async fn execute_request<B: Serialize>(
        &self,
        method: &Method,
        url: &str,
        body: &Option<B>,
    ) -> std::result::Result<Response, reqwest::Error> {
        let mut request = self.client.request(method.clone(), url);

        if let Some(body) = body {
            request = request.json(body);
        }

        request.send().await
    }

    /// Check if status is retryable.
    fn is_retryable_status(status: StatusCode) -> bool {
        matches!(
            status,
            StatusCode::TOO_MANY_REQUESTS
                | StatusCode::SERVICE_UNAVAILABLE
                | StatusCode::GATEWAY_TIMEOUT
                | StatusCode::BAD_GATEWAY
        )
    }

    /// Check if an error is retryable (including HTTP/2 GOAWAY).
    fn is_retryable_error(err: &reqwest::Error) -> bool {
        // Check for HTTP/2 GOAWAY or connection reset errors
        let err_str = err.to_string().to_lowercase();
        err_str.contains("goaway")
            || err_str.contains("stream was reset")
            || err_str.contains("connection reset")
            || err_str.contains("connection was reset")
            || err_str.contains("h2 protocol error")
            || err.is_connect()
            || err.is_timeout()
    }

    /// Parse Retry-After header from HTTP response (#2745).
    ///
    /// Supports both formats:
    /// - Seconds: "Retry-After: 30" (most common)
    /// - HTTP date: "Retry-After: Wed, 21 Oct 2015 07:28:00 GMT"
    fn parse_retry_after_header(response: &Response) -> Option<u64> {
        let retry_after = response.headers().get("retry-after")?;
        let value = retry_after.to_str().ok()?.trim();

        // Try parsing as seconds (most common format)
        if let Ok(secs) = value.parse::<u64>() {
            return Some(secs);
        }

        // For HTTP date format, return a default fallback since parsing
        // full HTTP dates would require additional dependencies.
        // Most rate limit responses use seconds format.
        if value.contains(',') || value.contains("GMT") {
            // Looks like an HTTP date, use a reasonable default
            tracing::debug!(
                retry_after = value,
                "Retry-After header is HTTP date format, using default 60s"
            );
            return Some(60);
        }

        None
    }

    /// Get raw response.
    pub async fn get_raw(&self, path: &str) -> Result<String> {
        let url = format!("{}{}", self.config.base_url, path);
        let response = self.client.get(&url).send().await?;

        response.text().await.map_err(std::convert::Into::into)
    }

    /// Download bytes.
    pub async fn download(&self, path: &str) -> Result<Vec<u8>> {
        let url = format!("{}{}", self.config.base_url, path);
        let response = self.client.get(&url).send().await?;

        response
            .bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(std::convert::Into::into)
    }
}

/// Request builder.
pub struct RequestBuilder {
    method: Method,
    url: String,
    headers: HashMap<String, String>,
    query: HashMap<String, String>,
    body: Option<serde_json::Value>,
    timeout: Option<Duration>,
}

impl RequestBuilder {
    /// Create a GET request.
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::GET, url)
    }

    /// Create a POST request.
    pub fn post(url: impl Into<String>) -> Self {
        Self::new(Method::POST, url)
    }

    /// Create a PUT request.
    pub fn put(url: impl Into<String>) -> Self {
        Self::new(Method::PUT, url)
    }

    /// Create a DELETE request.
    pub fn delete(url: impl Into<String>) -> Self {
        Self::new(Method::DELETE, url)
    }

    /// Create a new request.
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: HashMap::new(),
            query: HashMap::new(),
            body: None,
            timeout: None,
        }
    }

    /// Add header.
    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    /// Add authorization header.
    pub fn bearer_auth(self, token: impl Into<String>) -> Self {
        self.header("Authorization", format!("Bearer {}", token.into()))
    }

    /// Add query parameter.
    pub fn query(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.insert(key.into(), value.into());
        self
    }

    /// Set JSON body.
    pub fn json<T: Serialize>(mut self, body: T) -> Self {
        self.body = serde_json::to_value(body).ok();
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Execute request.
    pub async fn send(self) -> Result<ApiResponse> {
        let client = Client::builder()
            .timeout(self.timeout.unwrap_or(Duration::from_secs(30)))
            .build()
            .map_err(|e| CortexError::Internal(format!("Failed to build client: {e}")))?;

        let mut url = self.url;
        if !self.query.is_empty() {
            let query_string: Vec<_> = self.query.iter().map(|(k, v)| format!("{k}={v}")).collect();
            url = format!("{}?{}", url, query_string.join("&"));
        }

        let mut request = client.request(self.method, &url);

        for (key, value) in self.headers {
            request = request.header(&key, &value);
        }

        if let Some(body) = self.body {
            request = request.json(&body);
        }

        let response = request.send().await?;

        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .filter_map(|(k, v)| v.to_str().ok().map(|v| (k.to_string(), v.to_string())))
            .collect();

        let body = response.text().await.map_err(|e: reqwest::Error| e)?;

        Ok(ApiResponse {
            status,
            headers,
            body,
        })
    }
}

/// API response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiResponse {
    /// Status code.
    pub status: u16,
    /// Headers.
    pub headers: HashMap<String, String>,
    /// Body.
    pub body: String,
}

impl ApiResponse {
    /// Check if successful.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Parse JSON body.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_str(&self.body).map_err(|e| CortexError::Internal(e.to_string()))
    }

    /// Get header value.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(std::string::String::as_str)
    }
}

/// Webhook handler.
pub struct WebhookHandler {
    secret: Option<String>,
}

impl WebhookHandler {
    /// Create a new handler.
    pub fn new() -> Self {
        Self { secret: None }
    }

    /// Create with secret.
    pub fn with_secret(secret: impl Into<String>) -> Self {
        Self {
            secret: Some(secret.into()),
        }
    }

    /// Verify signature (simplified - in production use proper HMAC).
    pub fn verify(&self, _payload: &[u8], _signature: &str) -> bool {
        // Simplified verification - always pass if no secret
        self.secret.is_none()
    }

    /// Parse webhook payload.
    pub fn parse<T: DeserializeOwned>(&self, payload: &str, signature: Option<&str>) -> Result<T> {
        if let Some(sig) = signature
            && !self.verify(payload.as_bytes(), sig)
        {
            return Err(CortexError::Auth("Invalid webhook signature".to_string()));
        }

        serde_json::from_str(payload).map_err(|e| CortexError::Internal(e.to_string()))
    }
}

impl Default for WebhookHandler {
    fn default() -> Self {
        Self::new()
    }
}

/// REST resource helper.
pub struct RestResource {
    client: ApiClient,
    path: String,
}

impl RestResource {
    /// Create a new resource.
    pub fn new(client: ApiClient, path: impl Into<String>) -> Self {
        Self {
            client,
            path: path.into(),
        }
    }

    /// List resources.
    pub async fn list<T: DeserializeOwned>(&self) -> Result<Vec<T>> {
        self.client.get(&self.path).await
    }

    /// Get resource by ID.
    pub async fn get<T: DeserializeOwned>(&self, id: &str) -> Result<T> {
        self.client.get(&format!("{}/{}", self.path, id)).await
    }

    /// Create resource.
    pub async fn create<T: DeserializeOwned, B: Serialize>(&self, data: B) -> Result<T> {
        self.client.post(&self.path, data).await
    }

    /// Update resource.
    pub async fn update<T: DeserializeOwned, B: Serialize>(&self, id: &str, data: B) -> Result<T> {
        self.client
            .put(&format!("{}/{}", self.path, id), data)
            .await
    }

    /// Delete resource.
    pub async fn delete<T: DeserializeOwned>(&self, id: &str) -> Result<T> {
        self.client.delete(&format!("{}/{}", self.path, id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let config = ApiClientConfig::default();
        assert_eq!(config.timeout_secs, 30);
        assert_eq!(config.max_retries, 3);
    }

    #[test]
    fn test_request_builder() {
        let builder = RequestBuilder::get("https://api.example.com")
            .header("X-Custom", "value")
            .bearer_auth("token123")
            .query("page", "1");

        assert_eq!(builder.headers.get("X-Custom"), Some(&"value".to_string()));
        assert!(
            builder
                .headers
                .get("Authorization")
                .unwrap()
                .contains("Bearer")
        );
    }

    #[test]
    fn test_api_response() {
        let response = ApiResponse {
            status: 200,
            headers: HashMap::new(),
            body: r#"{"message": "ok"}"#.to_string(),
        };

        assert!(response.is_success());

        let json: serde_json::Value = response.json().unwrap();
        assert_eq!(json["message"], "ok");
    }

    #[test]
    fn test_webhook_handler() {
        let handler = WebhookHandler::new();
        assert!(handler.verify(b"test", "any"));

        let _handler = WebhookHandler::with_secret("secret");
        // Signature verification would fail here without proper HMAC
    }
}
