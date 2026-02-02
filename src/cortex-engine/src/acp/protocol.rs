//! ACP Protocol types and JSON-RPC handling.
//!
//! This module provides the JSON-RPC infrastructure for the ACP protocol,
//! including request/response types and error handling.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request for ACP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpRequest {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Request ID.
    pub id: AcpRequestId,
    /// Method name.
    pub method: String,
    /// Parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl AcpRequest {
    /// Create a new request.
    pub fn new(id: impl Into<AcpRequestId>, method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: id.into(),
            method: method.into(),
            params: None,
        }
    }

    /// Set parameters.
    pub fn with_params(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }
}

/// JSON-RPC response for ACP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpResponse {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Request ID.
    pub id: AcpRequestId,
    /// Result (success).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error (failure).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AcpError>,
}

impl AcpResponse {
    /// Create a success response.
    pub fn success(id: AcpRequestId, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: AcpRequestId, error: AcpError) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(error),
        }
    }
}

/// JSON-RPC notification for ACP (no response expected).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpNotification {
    /// JSON-RPC version.
    pub jsonrpc: String,
    /// Method name.
    pub method: String,
    /// Parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl AcpNotification {
    /// Create a new notification.
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            method: method.into(),
            params: None,
        }
    }

    /// Set parameters.
    pub fn with_params(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }
}

/// Request ID (can be string or number).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum AcpRequestId {
    /// Numeric ID.
    Number(i64),
    /// String ID.
    String(String),
}

impl Default for AcpRequestId {
    fn default() -> Self {
        Self::Number(0)
    }
}

impl From<i64> for AcpRequestId {
    fn from(id: i64) -> Self {
        Self::Number(id)
    }
}

impl From<String> for AcpRequestId {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for AcpRequestId {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

/// JSON-RPC error for ACP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcpError {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Additional data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl AcpError {
    /// Parse error (-32700).
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self {
            code: -32700,
            message: message.into(),
            data: None,
        }
    }

    /// Invalid request (-32600).
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            code: -32600,
            message: message.into(),
            data: None,
        }
    }

    /// Method not found (-32601).
    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: -32601,
            message: format!("Method not found: {method}"),
            data: None,
        }
    }

    /// Invalid params (-32602).
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self {
            code: -32602,
            message: message.into(),
            data: None,
        }
    }

    /// Internal error (-32603).
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: -32603,
            message: message.into(),
            data: None,
        }
    }

    /// Session not found (-32001).
    pub fn session_not_found(session_id: &str) -> Self {
        Self {
            code: -32001,
            message: format!("Session not found: {session_id}"),
            data: None,
        }
    }

    /// Session cancelled (-32002).
    pub fn session_cancelled() -> Self {
        Self {
            code: -32002,
            message: "Session was cancelled".to_string(),
            data: None,
        }
    }

    /// Authentication required (-32003).
    pub fn auth_required() -> Self {
        Self {
            code: -32003,
            message: "Authentication required".to_string(),
            data: None,
        }
    }
}

/// ACP method names.
pub mod methods {
    /// Initialize the connection.
    pub const INITIALIZE: &str = "initialize";
    /// Create a new session.
    pub const SESSION_NEW: &str = "session/new";
    /// Load an existing session.
    pub const SESSION_LOAD: &str = "session/load";
    /// List available sessions.
    pub const SESSION_LIST: &str = "session/list";
    /// Send a prompt to a session.
    pub const SESSION_PROMPT: &str = "session/prompt";
    /// Cancel the current operation.
    pub const SESSION_CANCEL: &str = "session/cancel";
    /// Session update notification.
    pub const SESSION_UPDATE: &str = "session/update";
    /// Get available models.
    pub const MODELS_LIST: &str = "models/list";
    /// Get available agents.
    pub const AGENTS_LIST: &str = "agents/list";
}
