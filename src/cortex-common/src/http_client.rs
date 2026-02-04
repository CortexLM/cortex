//! Centralized HTTP client factory for all Cortex services.
//!
//! Provides factory functions to create HTTP clients with consistent configuration:
//! - `create_default_client()` - Standard 30s timeout
//! - `create_streaming_client()` - 5min timeout for LLM streaming
//! - `create_client_with_timeout(duration)` - Custom timeout
//!
//! All clients include: User-Agent, tcp_nodelay, and proper error handling.
//!
//! DNS caching is configured with reasonable TTL to allow failover and load
//! balancer updates (#2177).
//!
//! # Timeout Configuration Guide
//!
//! This section documents the timeout hierarchy across the Cortex codebase. Use this
//! as a reference when configuring timeouts for new features or debugging timeout issues.
//!
//! ## Timeout Hierarchy
//!
//! | Use Case                    | Timeout | Constant/Location                          | Rationale                               |
//! |-----------------------------|---------|--------------------------------------------|-----------------------------------------|
//! | Health checks               | 5s      | `HEALTH_CHECK_TIMEOUT` (this module)       | Quick validation of service status      |
//! | Standard HTTP requests      | 30s     | `DEFAULT_TIMEOUT` (this module)            | Normal API calls with reasonable margin |
//! | Per-chunk read (streaming)  | 30s     | `read_timeout` (cortex-app-server/config)  | Individual chunk timeout during stream  |
//! | Pool idle timeout           | 60s     | `POOL_IDLE_TIMEOUT` (this module)          | DNS re-resolution for failover          |
//! | LLM Request (non-streaming) | 120s    | `DEFAULT_REQUEST_TIMEOUT_SECS` (cortex-exec/runner) | Model inference takes time |
//! | LLM Streaming total         | 300s    | `STREAMING_TIMEOUT` (this module)          | Long-running streaming responses        |
//! | Server request lifecycle    | 300s    | `request_timeout` (cortex-app-server/config) | Full HTTP request/response cycle      |
//! | Entire exec session         | 600s    | `DEFAULT_TIMEOUT_SECS` (cortex-exec/runner) | Multi-turn conversation limit          |
//! | Graceful shutdown           | 30s     | `shutdown_timeout` (cortex-app-server/config) | Time for cleanup on shutdown        |
//!
//! ## Module-Specific Timeouts
//!
//! ### cortex-common (this module)
//! - `DEFAULT_TIMEOUT` (30s): Use for standard API calls.
//! - `STREAMING_TIMEOUT` (300s): Use for LLM streaming endpoints.
//! - `HEALTH_CHECK_TIMEOUT` (5s): Use for health/readiness checks.
//! - `POOL_IDLE_TIMEOUT` (60s): Connection pool cleanup for DNS freshness.
//!
//! ### cortex-exec (runner.rs)
//! - `DEFAULT_TIMEOUT_SECS` (600s): Maximum duration for entire exec session.
//! - `DEFAULT_REQUEST_TIMEOUT_SECS` (120s): Single LLM request timeout.
//!
//! ### cortex-app-server (config.rs)
//! - `request_timeout` (300s): Full request lifecycle timeout.
//! - `read_timeout` (30s): Per-chunk timeout for streaming reads.
//! - `shutdown_timeout` (30s): Graceful shutdown duration.
//!
//! ### cortex-engine (api_client.rs)
//! - Re-exports constants from this module for consistency.
//!
//! ## Recommendations
//!
//! When adding new timeout configurations:
//! 1. Use constants from this module when possible for consistency.
//! 2. Document any new timeout constants with their rationale.
//! 3. Consider the timeout hierarchy - inner timeouts should be shorter than outer ones.
//! 4. For LLM operations, use longer timeouts (120s-300s) to accommodate model inference.
//! 5. For health checks and quick validations, use short timeouts (5s-10s).

use reqwest::Client;
use std::time::Duration;

/// User-Agent string for all HTTP requests
pub const USER_AGENT: &str = concat!("cortex-cli/", env!("CARGO_PKG_VERSION"));

/// Default timeout for standard API requests (30 seconds)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Extended timeout for LLM streaming requests (5 minutes)
pub const STREAMING_TIMEOUT: Duration = Duration::from_secs(300);

/// Short timeout for health checks (5 seconds)
pub const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Connection pool idle timeout to ensure DNS is re-resolved periodically.
/// This helps with failover scenarios where DNS records change (#2177).
/// Set to 60 seconds to balance between performance and DNS freshness.
pub const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Creates an HTTP client with default configuration (30s timeout).
///
/// Includes: User-Agent, tcp_nodelay, 30s timeout.
///
/// # Example
/// ```ignore
/// let client = create_default_client().expect("HTTP client");
/// let resp = client.get("https://api.example.com/data").send().await.ok();
/// ```
pub fn create_default_client() -> Result<Client, String> {
    create_client_with_timeout(DEFAULT_TIMEOUT)
}

/// Creates an HTTP client for LLM streaming (5min timeout).
///
/// Use this for endpoints that stream responses (SSE, chunked transfer).
///
/// # Example
/// ```ignore
/// let client = create_streaming_client().expect("HTTP client");
/// let resp = client.post("https://api.example.com/chat").send().await.ok();
/// ```
pub fn create_streaming_client() -> Result<Client, String> {
    create_client_with_timeout(STREAMING_TIMEOUT)
}

/// Creates an HTTP client for health checks (5s timeout).
pub fn create_health_check_client() -> Result<Client, String> {
    create_client_with_timeout(HEALTH_CHECK_TIMEOUT)
}

/// Creates an HTTP client with a custom timeout.
///
/// All clients include:
/// - User-Agent: `cortex-cli/{version}`
/// - tcp_nodelay: true (for lower latency)
/// - pool_idle_timeout: 60s (for DNS TTL respect, #2177)
/// - Specified timeout
/// - Read timeout to prevent hangs on Content-Length mismatches
pub fn create_client_with_timeout(timeout: Duration) -> Result<Client, String> {
    // Set read timeout to prevent hangs when Content-Length doesn't match actual body size
    // This handles cases where connection closes early or proxy truncates responses
    let read_timeout = timeout.min(Duration::from_secs(60));

    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .read_timeout(read_timeout)
        .tcp_nodelay(true)
        // Ensure connections are closed periodically to allow DNS re-resolution
        // This prevents stale IP addresses from being used after DNS changes (#2177)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_max_idle_per_host(4)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))
}

/// Creates an HTTP client builder with standard configuration.
///
/// Use this when you need to customize the client further before building.
/// Includes read timeout to prevent hangs on truncated responses.
///
/// # Example
/// ```ignore
/// let client = create_client_builder()
///     .redirect(reqwest::redirect::Policy::none())
///     .build()
///     .expect("HTTP client");
/// ```
pub fn create_client_builder() -> reqwest::ClientBuilder {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DEFAULT_TIMEOUT)
        .read_timeout(DEFAULT_TIMEOUT)
        .tcp_nodelay(true)
        // Ensure connections are closed periodically to allow DNS re-resolution (#2177)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .pool_max_idle_per_host(4)
}

/// Creates a blocking HTTP client with default configuration.
pub fn create_blocking_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(DEFAULT_TIMEOUT)
        .build()
        .map_err(|e| format!("Failed to build blocking HTTP client: {e}"))
}

/// Creates a blocking HTTP client with custom timeout.
pub fn create_blocking_client_with_timeout(
    timeout: Duration,
) -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .build()
        .map_err(|e| format!("Failed to build blocking HTTP client: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_default_client_succeeds() {
        let result = create_default_client();
        assert!(result.is_ok(), "create_default_client should succeed");
    }

    #[test]
    fn test_create_streaming_client_succeeds() {
        let result = create_streaming_client();
        assert!(result.is_ok(), "create_streaming_client should succeed");
    }

    #[test]
    fn test_create_health_check_client_succeeds() {
        let result = create_health_check_client();
        assert!(result.is_ok(), "create_health_check_client should succeed");
    }

    #[test]
    fn test_create_client_with_timeout_succeeds() {
        let result = create_client_with_timeout(Duration::from_secs(60));
        assert!(result.is_ok(), "create_client_with_timeout should succeed");
    }

    #[test]
    fn test_create_blocking_client_succeeds() {
        let result = create_blocking_client();
        assert!(result.is_ok(), "create_blocking_client should succeed");
    }

    #[test]
    fn test_create_blocking_client_with_timeout_succeeds() {
        let result = create_blocking_client_with_timeout(Duration::from_secs(60));
        assert!(
            result.is_ok(),
            "create_blocking_client_with_timeout should succeed"
        );
    }

    #[test]
    fn test_create_client_builder_returns_builder() {
        let builder = create_client_builder();
        let result = builder.build();
        assert!(
            result.is_ok(),
            "create_client_builder should return valid builder"
        );
    }

    #[test]
    fn test_user_agent_constant_is_set() {
        assert!(!USER_AGENT.is_empty(), "USER_AGENT should not be empty");
        assert!(
            USER_AGENT.contains("cortex-cli"),
            "USER_AGENT should contain cortex-cli"
        );
    }

    #[test]
    fn test_timeout_constants_are_correct() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
        assert_eq!(STREAMING_TIMEOUT, Duration::from_secs(300));
        assert_eq!(HEALTH_CHECK_TIMEOUT, Duration::from_secs(5));
    }
}
