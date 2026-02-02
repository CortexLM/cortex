//! Error types for Slack integration.
//!
//! Provides comprehensive error handling for all Slack API operations,
//! including network errors, API errors, authentication failures,
//! and WebSocket connection issues.

use thiserror::Error;

/// Errors that can occur during Slack operations.
#[derive(Error, Debug)]
pub enum SlackError {
    /// Configuration error (missing or invalid config).
    #[error("Configuration error: {0}")]
    Config(String),

    /// Authentication error (invalid token, expired, etc.).
    #[error("Authentication error: {0}")]
    Auth(String),

    /// API request failed.
    #[error("Slack API error: {0}")]
    Api(String),

    /// API rate limited.
    #[error("Rate limited: retry after {retry_after_secs} seconds")]
    RateLimited {
        /// Seconds to wait before retrying.
        retry_after_secs: u64,
    },

    /// Network/HTTP error.
    #[error("Network error: {0}")]
    Network(String),

    /// WebSocket connection error.
    #[error("WebSocket error: {0}")]
    WebSocket(String),

    /// Message formatting error.
    #[error("Message formatting error: {0}")]
    Formatting(String),

    /// JSON serialization/deserialization error.
    #[error("JSON error: {0}")]
    Json(String),

    /// Request signature verification failed.
    #[error("Signature verification failed: {0}")]
    SignatureVerification(String),

    /// Operation timed out.
    #[error("Operation timed out: {0}")]
    Timeout(String),

    /// Channel not found or bot not in channel.
    #[error("Channel error: {0}")]
    Channel(String),

    /// User not found.
    #[error("User error: {0}")]
    User(String),

    /// Invalid payload received from Slack.
    #[error("Invalid payload: {0}")]
    InvalidPayload(String),

    /// Credential storage error.
    #[error("Credential storage error: {0}")]
    CredentialStorage(String),

    /// Internal error.
    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<reqwest::Error> for SlackError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_timeout() {
            SlackError::Timeout(err.to_string())
        } else if err.is_connect() {
            SlackError::Network(format!("Connection failed: {}", err))
        } else {
            SlackError::Network(err.to_string())
        }
    }
}

impl From<serde_json::Error> for SlackError {
    fn from(err: serde_json::Error) -> Self {
        SlackError::Json(err.to_string())
    }
}

impl From<tokio_tungstenite::tungstenite::Error> for SlackError {
    fn from(err: tokio_tungstenite::tungstenite::Error) -> Self {
        SlackError::WebSocket(err.to_string())
    }
}

impl From<cortex_keyring_store::KeyringError> for SlackError {
    fn from(err: cortex_keyring_store::KeyringError) -> Self {
        SlackError::CredentialStorage(err.to_string())
    }
}

impl From<std::env::VarError> for SlackError {
    fn from(err: std::env::VarError) -> Self {
        SlackError::Config(format!("Environment variable error: {}", err))
    }
}

/// Result type for Slack operations.
pub type SlackResult<T> = std::result::Result<T, SlackError>;

/// Represents a Slack API response error.
#[derive(Debug, Clone)]
pub struct SlackApiError {
    /// Error code from Slack (e.g., "channel_not_found").
    pub code: String,
    /// Human-readable error message.
    pub message: String,
    /// Whether this error is retryable.
    pub retryable: bool,
}

impl SlackApiError {
    /// Create a new API error.
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let retryable = Self::is_retryable_code(&code);
        Self {
            code,
            message: message.into(),
            retryable,
        }
    }

    /// Check if an error code is retryable.
    fn is_retryable_code(code: &str) -> bool {
        matches!(
            code,
            "rate_limited"
                | "service_unavailable"
                | "internal_error"
                | "request_timeout"
                | "fatal_error"
        )
    }
}

impl From<SlackApiError> for SlackError {
    fn from(err: SlackApiError) -> Self {
        if err.code == "rate_limited" {
            // Default retry after 30 seconds if not specified
            SlackError::RateLimited {
                retry_after_secs: 30,
            }
        } else if err.code == "invalid_auth" || err.code == "account_inactive" {
            SlackError::Auth(err.message)
        } else if err.code == "channel_not_found" || err.code == "not_in_channel" {
            SlackError::Channel(err.message)
        } else if err.code == "user_not_found" {
            SlackError::User(err.message)
        } else {
            SlackError::Api(format!("{}: {}", err.code, err.message))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = SlackError::Config("missing token".to_string());
        assert_eq!(err.to_string(), "Configuration error: missing token");

        let err = SlackError::RateLimited {
            retry_after_secs: 60,
        };
        assert_eq!(err.to_string(), "Rate limited: retry after 60 seconds");
    }

    #[test]
    fn test_api_error_retryable() {
        let err = SlackApiError::new("rate_limited", "Too many requests");
        assert!(err.retryable);

        let err = SlackApiError::new("channel_not_found", "Channel not found");
        assert!(!err.retryable);
    }

    #[test]
    fn test_api_error_conversion() {
        let api_err = SlackApiError::new("invalid_auth", "Token has been revoked");
        let slack_err: SlackError = api_err.into();
        assert!(matches!(slack_err, SlackError::Auth(_)));

        let api_err = SlackApiError::new("channel_not_found", "Channel not found");
        let slack_err: SlackError = api_err.into();
        assert!(matches!(slack_err, SlackError::Channel(_)));
    }
}
