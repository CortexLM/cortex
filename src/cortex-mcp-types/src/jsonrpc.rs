//! JSON-RPC 2.0 types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC version string.
pub const JSONRPC_VERSION: &str = "2.0";

/// JSON-RPC request ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum RequestId {
    /// Numeric request ID.
    Number(i64),
    /// String request ID.
    String(String),
}

impl From<i64> for RequestId {
    fn from(id: i64) -> Self {
        Self::Number(id)
    }
}

impl From<i32> for RequestId {
    fn from(id: i32) -> Self {
        Self::Number(id as i64)
    }
}

impl From<u32> for RequestId {
    fn from(id: u32) -> Self {
        Self::Number(id as i64)
    }
}

impl From<String> for RequestId {
    fn from(id: String) -> Self {
        Self::String(id)
    }
}

impl From<&str> for RequestId {
    fn from(id: &str) -> Self {
        Self::String(id.to_string())
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(n) => write!(f, "{n}"),
            Self::String(s) => write!(f, "{s}"),
        }
    }
}

/// JSON-RPC 2.0 request.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcRequest {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Request ID.
    pub id: RequestId,
    /// Method name.
    pub method: String,
    /// Optional parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    /// Create a new JSON-RPC request.
    pub fn new(id: impl Into<RequestId>, method: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            method: method.into(),
            params: None,
        }
    }

    /// Add parameters to the request.
    pub fn with_params(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }
}

/// JSON-RPC 2.0 response.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcResponse {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Request ID (matches the request).
    pub id: RequestId,
    /// Result on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error on failure.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    /// Create a success response.
    pub fn success(id: impl Into<RequestId>, result: Value) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn error(id: impl Into<RequestId>, error: JsonRpcError) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            result: None,
            error: Some(error),
        }
    }

    /// Check if the response indicates success.
    pub fn is_success(&self) -> bool {
        self.error.is_none()
    }

    /// Check if the response indicates an error.
    pub fn is_error(&self) -> bool {
        self.error.is_some()
    }

    /// Convert to Result.
    pub fn into_result(self) -> Result<Value, JsonRpcError> {
        match self.error {
            Some(e) => Err(e),
            None => Ok(self.result.unwrap_or(Value::Null)),
        }
    }
}

/// JSON-RPC 2.0 notification (no response expected).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcNotification {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Method name.
    pub method: String,
    /// Optional parameters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    /// Create a new JSON-RPC notification.
    pub fn new(method: impl Into<String>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params: None,
        }
    }

    /// Add parameters to the notification.
    pub fn with_params(mut self, params: Value) -> Self {
        self.params = Some(params);
        self
    }
}

/// JSON-RPC 2.0 error.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct JsonRpcError {
    /// Error code.
    pub code: i32,
    /// Error message.
    pub message: String,
    /// Additional error data.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcError {
    /// Create a new error.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Add data to the error.
    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    /// Parse error (-32700).
    pub fn parse_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::PARSE_ERROR, message)
    }

    /// Invalid request error (-32600).
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INVALID_REQUEST, message)
    }

    /// Method not found error (-32601).
    pub fn method_not_found(method: &str) -> Self {
        Self::new(
            ErrorCode::METHOD_NOT_FOUND,
            format!("Method not found: {method}"),
        )
    }

    /// Invalid params error (-32602).
    pub fn invalid_params(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INVALID_PARAMS, message)
    }

    /// Internal error (-32603).
    pub fn internal_error(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::INTERNAL_ERROR, message)
    }

    /// Server error (custom code in -32000 to -32099 range).
    pub fn server_error(code: i32, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }
}

impl std::fmt::Display for JsonRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for JsonRpcError {}

/// Standard JSON-RPC error codes.
pub struct ErrorCode;

impl ErrorCode {
    /// Parse error - Invalid JSON was received.
    pub const PARSE_ERROR: i32 = -32700;
    /// Invalid request - The JSON sent is not a valid Request object.
    pub const INVALID_REQUEST: i32 = -32600;
    /// Method not found - The method does not exist.
    pub const METHOD_NOT_FOUND: i32 = -32601;
    /// Invalid params - Invalid method parameter(s).
    pub const INVALID_PARAMS: i32 = -32602;
    /// Internal error - Internal JSON-RPC error.
    pub const INTERNAL_ERROR: i32 = -32603;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_from() {
        let id1: RequestId = 42.into();
        let id2: RequestId = "test".into();
        let id3: RequestId = "123".to_string().into();

        assert!(matches!(id1, RequestId::Number(42)));
        assert!(matches!(id2, RequestId::String(ref s) if s == "test"));
        assert!(matches!(id3, RequestId::String(ref s) if s == "123"));
    }

    #[test]
    fn test_json_rpc_request() {
        let request =
            JsonRpcRequest::new(1, "test/method").with_params(serde_json::json!({"key": "value"}));

        assert_eq!(request.jsonrpc, JSONRPC_VERSION);
        assert_eq!(request.method, "test/method");
        assert!(request.params.is_some());

        let json = serde_json::to_string(&request).expect("serialization should succeed");
        assert!(json.contains("test/method"));
    }

    #[test]
    fn test_json_rpc_response() {
        let success = JsonRpcResponse::success(1, serde_json::json!("ok"));
        assert!(success.is_success());
        assert!(!success.is_error());

        let error = JsonRpcResponse::error(1, JsonRpcError::method_not_found("test"));
        assert!(!error.is_success());
        assert!(error.is_error());
    }

    #[test]
    fn test_json_rpc_error_codes() {
        let parse_error = JsonRpcError::parse_error("Invalid JSON");
        assert_eq!(parse_error.code, ErrorCode::PARSE_ERROR);

        let method_not_found = JsonRpcError::method_not_found("unknown");
        assert_eq!(method_not_found.code, ErrorCode::METHOD_NOT_FOUND);
    }
}
