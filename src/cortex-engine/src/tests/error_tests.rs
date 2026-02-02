//! Tests for error module.

use crate::error::*;
use std::path::PathBuf;

#[test]
fn test_cortex_error_config() {
    let err = CortexError::config("test error");
    assert!(matches!(err, CortexError::Config(_)));
    assert_eq!(err.to_string(), "Configuration error: test error");
}

#[test]
fn test_cortex_error_model() {
    let err = CortexError::model("test model error");
    assert!(matches!(err, CortexError::Model(_)));
    assert_eq!(err.to_string(), "Model error: test model error");
}

#[test]
fn test_cortex_error_sandbox() {
    let err = CortexError::sandbox("sandbox failure");
    assert!(matches!(err, CortexError::Sandbox(_)));
}

#[test]
fn test_cortex_error_internal() {
    let err = CortexError::internal("internal issue");
    assert!(matches!(err, CortexError::Internal(_)));
}

#[test]
fn test_cortex_error_tool_execution() {
    let err = CortexError::tool_execution("grep", "file not found");
    assert!(matches!(err, CortexError::ToolExecution { .. }));
    assert!(err.to_string().contains("grep"));
    assert!(err.to_string().contains("file not found"));
}

#[test]
fn test_cortex_error_mcp() {
    let err = CortexError::mcp("test-server", "connection failed");
    assert!(matches!(err, CortexError::Mcp { .. }));
    assert!(err.to_string().contains("test-server"));
}

#[test]
fn test_cortex_error_mcp_error() {
    let err = CortexError::mcp_error("generic mcp error");
    assert!(matches!(err, CortexError::Mcp { .. }));
}

#[test]
fn test_cortex_error_invalid_input() {
    let err = CortexError::invalid_input("bad input");
    assert!(matches!(err, CortexError::InvalidInput(_)));
}

#[test]
fn test_is_retriable_rate_limit() {
    assert!(CortexError::RateLimitExceeded.is_retriable());
}

#[test]
fn test_is_retriable_rate_limit_string() {
    assert!(CortexError::RateLimit("too many requests".to_string()).is_retriable());
}

#[test]
fn test_is_retriable_timeout() {
    assert!(CortexError::Timeout.is_retriable());
}

#[test]
fn test_is_retriable_connection_failed() {
    let err = CortexError::ConnectionFailed {
        endpoint: "api.example.com".to_string(),
        message: "connection refused".to_string(),
    };
    assert!(err.is_retriable());
}

#[test]
fn test_is_retriable_backend_unavailable() {
    assert!(CortexError::BackendUnavailable("server down".to_string()).is_retriable());
}

#[test]
fn test_not_retriable_config() {
    assert!(!CortexError::config("invalid").is_retriable());
}

#[test]
fn test_not_retriable_model() {
    assert!(!CortexError::model("unknown model").is_retriable());
}

#[test]
fn test_is_auth_error_auth() {
    assert!(CortexError::Auth("unauthorized".to_string()).is_auth_error());
}

#[test]
fn test_is_auth_error_api_key_not_found() {
    let err = CortexError::ApiKeyNotFound {
        provider: "openai".to_string(),
    };
    assert!(err.is_auth_error());
}

#[test]
fn test_is_auth_error_token_expired() {
    assert!(CortexError::TokenExpired.is_auth_error());
}

#[test]
fn test_not_auth_error_model() {
    assert!(!CortexError::model("test").is_auth_error());
}

#[test]
fn test_config_not_found() {
    let err = CortexError::ConfigNotFound {
        path: PathBuf::from("/etc/config.toml"),
    };
    assert!(err.to_string().contains("/etc/config.toml"));
}

#[test]
fn test_invalid_config() {
    let err = CortexError::InvalidConfig {
        field: "api_key".to_string(),
        message: "cannot be empty".to_string(),
    };
    assert!(err.to_string().contains("api_key"));
    assert!(err.to_string().contains("cannot be empty"));
}

#[test]
fn test_model_not_found() {
    let err = CortexError::ModelNotFound {
        model: "gpt-5".to_string(),
    };
    assert!(err.to_string().contains("gpt-5"));
}

#[test]
fn test_provider_not_found() {
    let err = CortexError::ProviderNotFound {
        provider: "unknown_provider".to_string(),
    };
    assert!(err.to_string().contains("unknown_provider"));
}

#[test]
fn test_context_window_exceeded() {
    let err = CortexError::ContextWindowExceeded {
        used: 10000,
        limit: 8000,
    };
    assert!(err.to_string().contains("10000"));
    assert!(err.to_string().contains("8000"));
}

#[test]
fn test_tool_timeout() {
    let err = CortexError::ToolTimeout {
        tool: "execute".to_string(),
        timeout_ms: 30000,
    };
    assert!(err.to_string().contains("execute"));
    assert!(err.to_string().contains("30000"));
}

#[test]
fn test_unknown_tool() {
    let err = CortexError::UnknownTool {
        name: "invalid_tool".to_string(),
    };
    assert!(err.to_string().contains("invalid_tool"));
}

#[test]
fn test_file_not_found() {
    let err = CortexError::FileNotFound {
        path: PathBuf::from("/missing/file.txt"),
    };
    assert!(err.to_string().contains("/missing/file.txt"));
}

#[test]
fn test_permission_denied() {
    let err = CortexError::PermissionDenied {
        path: PathBuf::from("/restricted/file"),
    };
    assert!(err.to_string().contains("/restricted/file"));
}

#[test]
fn test_sandbox_not_available() {
    let err = CortexError::SandboxNotAvailable;
    assert!(err.to_string().contains("not available"));
}

#[test]
fn test_sandbox_denied() {
    let err = CortexError::SandboxDenied {
        command: "rm -rf /".to_string(),
    };
    assert!(err.to_string().contains("rm -rf /"));
}

#[test]
fn test_mcp_server_not_found() {
    let err = CortexError::McpServerNotFound {
        server: "missing_server".to_string(),
    };
    assert!(err.to_string().contains("missing_server"));
}

#[test]
fn test_channel_closed() {
    let err = CortexError::ChannelClosed;
    assert!(err.to_string().contains("closed"));
}

#[test]
fn test_cancelled() {
    let err = CortexError::Cancelled;
    assert!(err.to_string().contains("cancelled"));
}

#[test]
fn test_provider_error() {
    let err = CortexError::Provider("test provider error".to_string());
    assert!(err.to_string().contains("test provider error"));
}

#[test]
fn test_provider_error_struct() {
    let err = CortexError::ProviderError {
        message: "detailed error".to_string(),
    };
    assert!(err.to_string().contains("detailed error"));
}

#[test]
fn test_not_found() {
    let err = CortexError::NotFound("resource".to_string());
    assert!(err.to_string().contains("resource"));
}

#[test]
fn test_serialization() {
    let err = CortexError::Serialization("parse error".to_string());
    assert!(err.to_string().contains("parse error"));
}
