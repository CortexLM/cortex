//! Centralized timeout constants for the Cortex CLI.
//!
//! This module provides consistent timeout values used throughout the codebase.
//! Centralizing these values ensures uniformity and makes it easier to adjust
//! timeouts across the application.

/// Default timeout for the entire execution in seconds (10 minutes).
///
/// This is the maximum time allowed for a complete headless execution,
/// including all LLM requests and tool executions.
pub const DEFAULT_EXEC_TIMEOUT_SECS: u64 = 600;

/// Default timeout for a single LLM request in seconds (2 minutes).
///
/// This is the maximum time to wait for a single completion request
/// to the LLM provider.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;

/// Default timeout for streaming responses in seconds (5 minutes).
///
/// Extended timeout for LLM streaming requests where responses are
/// delivered incrementally over time.
pub const DEFAULT_STREAMING_TIMEOUT_SECS: u64 = 300;

/// Default timeout for health check requests in seconds (5 seconds).
///
/// Short timeout used for quick health check endpoints.
pub const DEFAULT_HEALTH_CHECK_TIMEOUT_SECS: u64 = 5;

/// Default timeout for graceful shutdown in seconds (30 seconds).
///
/// Maximum time to wait for in-flight operations to complete during
/// shutdown before forcing termination.
pub const DEFAULT_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

/// Default timeout for batch execution in seconds (5 minutes).
///
/// Maximum time allowed for executing a batch of parallel tool calls.
pub const DEFAULT_BATCH_TIMEOUT_SECS: u64 = 300;

/// Default timeout for individual read operations in seconds (30 seconds).
///
/// Timeout for individual read operations to prevent hangs when
/// Content-Length doesn't match actual body size.
pub const DEFAULT_READ_TIMEOUT_SECS: u64 = 30;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn test_timeout_values_are_reasonable() {
        // Exec timeout should be greater than request timeout
        assert!(DEFAULT_EXEC_TIMEOUT_SECS > DEFAULT_REQUEST_TIMEOUT_SECS);

        // Streaming timeout should be greater than request timeout
        assert!(DEFAULT_STREAMING_TIMEOUT_SECS > DEFAULT_REQUEST_TIMEOUT_SECS);

        // Health check should be short
        assert!(DEFAULT_HEALTH_CHECK_TIMEOUT_SECS <= 10);

        // Batch timeout should be reasonable
        assert!(DEFAULT_BATCH_TIMEOUT_SECS >= 60);
    }
}
