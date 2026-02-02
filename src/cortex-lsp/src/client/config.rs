//! LSP client configuration types.

use std::time::Duration;

/// Default timeout for LSP requests in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Default timeout for reading individual headers/content in seconds.
pub const DEFAULT_READ_TIMEOUT_SECS: u64 = 60;

/// Maximum allowed content length to prevent memory exhaustion (10MB).
pub const MAX_CONTENT_LENGTH: usize = 10 * 1024 * 1024;

/// Configuration for LSP client timeouts.
#[derive(Debug, Clone)]
pub struct LspClientConfig {
    /// Timeout for LSP requests (default: 30 seconds).
    pub request_timeout: Duration,
    /// Timeout for reading responses (default: 60 seconds).
    pub read_timeout: Duration,
    /// Maximum content length for responses (default: 10MB).
    pub max_content_length: usize,
}

impl Default for LspClientConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
            read_timeout: Duration::from_secs(DEFAULT_READ_TIMEOUT_SECS),
            max_content_length: MAX_CONTENT_LENGTH,
        }
    }
}

impl LspClientConfig {
    /// Create a new configuration with custom request timeout.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Create a new configuration with custom read timeout.
    pub fn with_read_timeout(mut self, timeout: Duration) -> Self {
        self.read_timeout = timeout;
        self
    }

    /// Create a new configuration with custom max content length.
    pub fn with_max_content_length(mut self, max_length: usize) -> Self {
        self.max_content_length = max_length;
        self
    }
}
