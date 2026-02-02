//! Error types for the app server.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

/// Application error type.
#[derive(Debug, Error)]
pub enum AppError {
    /// Authentication error.
    #[error("Authentication failed: {0}")]
    Authentication(String),

    /// Authorization error.
    #[error("Not authorized: {0}")]
    Authorization(String),

    /// Validation error.
    #[error("Validation failed: {0}")]
    Validation(String),

    /// Resource not found.
    #[error("Not found: {0}")]
    NotFound(String),

    /// Resource already exists.
    #[error("Already exists: {0}")]
    Conflict(String),

    /// Rate limit exceeded.
    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    /// Provider error.
    #[error("Provider error: {0}")]
    Provider(String),

    /// Session error.
    #[error("Session error: {0}")]
    Session(String),

    /// Internal server error.
    #[error("Internal error: {0}")]
    Internal(String),

    /// Request timeout.
    #[error("Request timeout")]
    Timeout,

    /// Payload too large.
    #[error("Payload too large")]
    PayloadTooLarge,

    /// Bad request.
    #[error("Bad request: {0}")]
    BadRequest(String),

    /// Service unavailable.
    #[error("Service unavailable: {0}")]
    Unavailable(String),

    /// Feature not implemented.
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Resource gone (expired).
    #[error("Gone: {0}")]
    Gone(String),
}

impl AppError {
    /// Get the HTTP status code for this error.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::Authentication(_) => StatusCode::UNAUTHORIZED,
            Self::Authorization(_) => StatusCode::FORBIDDEN,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
            Self::Provider(_) => StatusCode::BAD_GATEWAY,
            Self::Session(_) => StatusCode::BAD_REQUEST,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Timeout => StatusCode::GATEWAY_TIMEOUT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            Self::Gone(_) => StatusCode::GONE,
        }
    }

    /// Get the error code string.
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::Authentication(_) => "authentication_failed",
            Self::Authorization(_) => "not_authorized",
            Self::Validation(_) => "validation_error",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::RateLimitExceeded => "rate_limit_exceeded",
            Self::Provider(_) => "provider_error",
            Self::Session(_) => "session_error",
            Self::Internal(_) => "internal_error",
            Self::Timeout => "timeout",
            Self::PayloadTooLarge => "payload_too_large",
            Self::BadRequest(_) => "bad_request",
            Self::Unavailable(_) => "service_unavailable",
            Self::NotImplemented(_) => "not_implemented",
            Self::Gone(_) => "gone",
        }
    }
}

/// Error response body.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// Error type.
    pub error: ErrorDetail,
}

/// Error detail.
#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    /// Error code.
    pub code: String,
    /// Error message.
    pub message: String,
    /// Additional details.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    /// Request ID for tracing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        use axum::http::header;

        let status = self.status_code();
        let is_rate_limited = matches!(self, AppError::RateLimitExceeded);

        let body = ErrorResponse {
            error: ErrorDetail {
                code: self.error_code().to_string(),
                message: self.to_string(),
                details: None,
                request_id: None,
            },
        };

        let mut response = (status, Json(body)).into_response();

        // Add Retry-After header for rate limit responses (429)
        // This helps clients implement proper backoff strategies
        if is_rate_limited {
            response.headers_mut().insert(
                header::RETRY_AFTER,
                "60".parse().unwrap(), // Suggest retry after 60 seconds
            );
        }

        response
    }
}

/// Result type for the app server.
pub type AppResult<T> = Result<T, AppError>;

impl From<anyhow::Error> for AppError {
    fn from(error: anyhow::Error) -> Self {
        Self::Internal(error.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(error: serde_json::Error) -> Self {
        Self::Validation(error.to_string())
    }
}

impl From<cortex_engine::error::CortexError> for AppError {
    fn from(error: cortex_engine::error::CortexError) -> Self {
        use cortex_engine::error::CortexError;
        match &error {
            CortexError::RateLimitExceeded => Self::RateLimitExceeded,
            CortexError::RateLimit(_) => Self::RateLimitExceeded,
            CortexError::Timeout => Self::Timeout,
            _ => Self::Internal(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_status_codes() {
        assert_eq!(
            AppError::Authentication("test".into()).status_code(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            AppError::NotFound("test".into()).status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            AppError::RateLimitExceeded.status_code(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn test_error_codes() {
        assert_eq!(
            AppError::Authentication("test".into()).error_code(),
            "authentication_failed"
        );
        assert_eq!(
            AppError::Validation("test".into()).error_code(),
            "validation_error"
        );
    }
}
