//! JSON-RPC types and utilities for exec mode.

use cortex_protocol::{ConversationId, Event, EventMsg};
use serde::{Deserialize, Serialize};

/// JSON-RPC request.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: Option<String>,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Map<String, serde_json::Value>,
}

/// JSON-RPC response.
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error.
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn result(id: serde_json::Value, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: serde_json::Value, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }

    pub fn notification(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: serde_json::json!(null),
            result: Some(serde_json::json!({
                "method": method,
                "params": params,
            })),
            error: None,
        }
    }
}

/// Convert event to JSON-RPC notification.
pub fn event_to_jsonrpc(event: &Event, session_id: &ConversationId) -> JsonRpcResponse {
    let (method, params) = match &event.msg {
        EventMsg::AgentMessage(msg) => (
            "message",
            serde_json::json!({
                "role": "assistant",
                "id": msg.id,
                "content": msg.message,
                "session_id": session_id.to_string(),
            }),
        ),
        EventMsg::AgentMessageDelta(delta) => (
            "message_delta",
            serde_json::json!({
                "delta": delta.delta,
                "session_id": session_id.to_string(),
            }),
        ),
        EventMsg::ExecCommandBegin(cmd) => (
            "tool_call_start",
            serde_json::json!({
                "call_id": cmd.call_id,
                "tool": "Execute",
                "command": cmd.command,
                "cwd": cmd.cwd.display().to_string(),
            }),
        ),
        EventMsg::ExecCommandEnd(cmd) => (
            "tool_call_end",
            serde_json::json!({
                "call_id": cmd.call_id,
                "tool": "Execute",
                "exit_code": cmd.exit_code,
                "output": cmd.formatted_output,
                "duration_ms": cmd.duration_ms,
            }),
        ),
        EventMsg::McpToolCallBegin(mcp) => (
            "tool_call_start",
            serde_json::json!({
                "call_id": mcp.call_id,
                "tool": mcp.invocation.tool,
                "server": mcp.invocation.server,
                "arguments": mcp.invocation.arguments,
            }),
        ),
        EventMsg::McpToolCallEnd(mcp) => (
            "tool_call_end",
            serde_json::json!({
                "call_id": mcp.call_id,
                "tool": mcp.invocation.tool,
                "result": mcp.result,
                "duration_ms": mcp.duration_ms,
            }),
        ),
        EventMsg::TaskComplete(tc) => (
            "task_complete",
            serde_json::json!({
                "last_message": tc.last_agent_message,
                "session_id": session_id.to_string(),
            }),
        ),
        EventMsg::Error(e) => (
            "error",
            serde_json::json!({
                "message": e.message,
                "session_id": session_id.to_string(),
            }),
        ),
        _ => {
            return JsonRpcResponse::notification(
                "event",
                serde_json::json!({
                    "type": format!("{:?}", event.msg).split('(').next().unwrap_or("unknown"),
                    "session_id": session_id.to_string(),
                }),
            );
        }
    };

    JsonRpcResponse::notification(method, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // JsonRpcRequest tests
    // =========================================================================

    #[test]
    fn test_jsonrpc_request_deserialization_minimal() {
        let json = r#"{"method": "test_method"}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(request.method, "test_method");
        assert!(request.jsonrpc.is_none());
        assert!(request.id.is_none());
        assert!(request.params.is_empty());
    }

    #[test]
    fn test_jsonrpc_request_deserialization_full() {
        let json = r#"{
            "jsonrpc": "2.0",
            "id": 123,
            "method": "execute",
            "params": {"prompt": "hello", "timeout": 30}
        }"#;
        let request: JsonRpcRequest = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(request.jsonrpc, Some("2.0".to_string()));
        assert_eq!(request.id, Some(serde_json::json!(123)));
        assert_eq!(request.method, "execute");
        assert_eq!(
            request.params.get("prompt"),
            Some(&serde_json::json!("hello"))
        );
        assert_eq!(request.params.get("timeout"), Some(&serde_json::json!(30)));
    }

    #[test]
    fn test_jsonrpc_request_deserialization_string_id() {
        let json = r#"{"id": "request-uuid-123", "method": "test"}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).expect("Should deserialize");
        assert_eq!(request.id, Some(serde_json::json!("request-uuid-123")));
    }

    #[test]
    fn test_jsonrpc_request_deserialization_null_id() {
        let json = r#"{"id": null, "method": "test"}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).expect("Should deserialize");
        // When id is explicitly set to null in JSON, serde deserializes it as None for Option<Value>
        // This is because serde treats JSON null as absence of value for Option types
        assert!(request.id.is_none() || request.id == Some(serde_json::Value::Null));
    }

    // =========================================================================
    // JsonRpcResponse tests
    // =========================================================================

    #[test]
    fn test_jsonrpc_response_result() {
        let result =
            JsonRpcResponse::result(serde_json::json!(1), serde_json::json!({"status": "ok"}));
        assert_eq!(result.jsonrpc, "2.0");
        assert!(result.result.is_some());
        assert!(result.error.is_none());

        // Check the result content
        let result_value = result.result.unwrap();
        assert_eq!(result_value.get("status"), Some(&serde_json::json!("ok")));
    }

    #[test]
    fn test_jsonrpc_response_error() {
        let error =
            JsonRpcResponse::error(serde_json::json!(2), -32600, "Invalid request".to_string());
        assert_eq!(error.jsonrpc, "2.0");
        assert!(error.result.is_none());
        assert!(error.error.is_some());

        let err = error.error.unwrap();
        assert_eq!(err.code, -32600);
        assert_eq!(err.message, "Invalid request");
        assert!(err.data.is_none());
    }

    #[test]
    fn test_jsonrpc_response_notification() {
        let notification =
            JsonRpcResponse::notification("message", serde_json::json!({"text": "hello"}));
        assert_eq!(notification.jsonrpc, "2.0");
        assert_eq!(notification.id, serde_json::Value::Null);
        assert!(notification.result.is_some());
        assert!(notification.error.is_none());

        let result = notification.result.unwrap();
        assert_eq!(result.get("method"), Some(&serde_json::json!("message")));
        assert!(result.get("params").is_some());
    }

    #[test]
    fn test_jsonrpc_response_result_with_string_id() {
        let result = JsonRpcResponse::result(
            serde_json::json!("request-123"),
            serde_json::json!({"data": [1, 2, 3]}),
        );
        assert_eq!(result.id, serde_json::json!("request-123"));
    }

    #[test]
    fn test_jsonrpc_response_result_with_null_id() {
        let result = JsonRpcResponse::result(serde_json::Value::Null, serde_json::json!(true));
        assert!(result.id.is_null());
    }

    #[test]
    fn test_jsonrpc_response_error_codes() {
        // Standard JSON-RPC error codes
        let parse_error =
            JsonRpcResponse::error(serde_json::json!(null), -32700, "Parse error".to_string());
        assert_eq!(parse_error.error.as_ref().unwrap().code, -32700);

        let invalid_request = JsonRpcResponse::error(
            serde_json::json!(null),
            -32600,
            "Invalid Request".to_string(),
        );
        assert_eq!(invalid_request.error.as_ref().unwrap().code, -32600);

        let method_not_found = JsonRpcResponse::error(
            serde_json::json!(null),
            -32601,
            "Method not found".to_string(),
        );
        assert_eq!(method_not_found.error.as_ref().unwrap().code, -32601);

        let invalid_params = JsonRpcResponse::error(
            serde_json::json!(null),
            -32602,
            "Invalid params".to_string(),
        );
        assert_eq!(invalid_params.error.as_ref().unwrap().code, -32602);

        let internal_error = JsonRpcResponse::error(
            serde_json::json!(null),
            -32603,
            "Internal error".to_string(),
        );
        assert_eq!(internal_error.error.as_ref().unwrap().code, -32603);
    }

    // =========================================================================
    // JsonRpcError tests
    // =========================================================================

    #[test]
    fn test_jsonrpc_error_serialization() {
        let error = JsonRpcError {
            code: -32000,
            message: "Server error".to_string(),
            data: None,
        };

        let json = serde_json::to_string(&error).expect("Should serialize");
        assert!(json.contains("-32000"));
        assert!(json.contains("Server error"));
        // data should be omitted when None (skip_serializing_if)
        assert!(!json.contains("data"));
    }

    #[test]
    fn test_jsonrpc_error_serialization_with_data() {
        let error = JsonRpcError {
            code: -32000,
            message: "Server error".to_string(),
            data: Some(serde_json::json!({"details": "additional info"})),
        };

        let json = serde_json::to_string(&error).expect("Should serialize");
        assert!(json.contains("data"));
        assert!(json.contains("additional info"));
    }

    // =========================================================================
    // JsonRpcResponse serialization tests
    // =========================================================================

    #[test]
    fn test_jsonrpc_response_serialization_result() {
        let response = JsonRpcResponse::result(serde_json::json!(1), serde_json::json!("success"));
        let json = serde_json::to_string(&response).expect("Should serialize");

        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"result\":\"success\""));
        // error should be omitted when None
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_jsonrpc_response_serialization_error() {
        let response =
            JsonRpcResponse::error(serde_json::json!(2), -32600, "Bad request".to_string());
        let json = serde_json::to_string(&response).expect("Should serialize");

        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":2"));
        assert!(json.contains("\"error\""));
        assert!(json.contains("-32600"));
        assert!(json.contains("Bad request"));
        // result should be omitted when None
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn test_jsonrpc_response_roundtrip() {
        let original = JsonRpcResponse::result(
            serde_json::json!(42),
            serde_json::json!({"key": "value", "count": 100}),
        );

        let json = serde_json::to_string(&original).expect("Should serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("Should deserialize");

        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["id"], 42);
        assert_eq!(parsed["result"]["key"], "value");
        assert_eq!(parsed["result"]["count"], 100);
    }

    // =========================================================================
    // Notification tests
    // =========================================================================

    #[test]
    fn test_jsonrpc_notification_various_methods() {
        let methods = [
            "message",
            "message_delta",
            "tool_call_start",
            "tool_call_end",
            "task_complete",
            "error",
        ];

        for method in methods {
            let notification = JsonRpcResponse::notification(method, serde_json::json!({}));
            let result = notification.result.as_ref().expect("Should have result");
            assert_eq!(result["method"], method);
        }
    }

    #[test]
    fn test_jsonrpc_notification_with_complex_params() {
        let params = serde_json::json!({
            "role": "assistant",
            "content": "Hello, world!",
            "metadata": {
                "tokens": 100,
                "model": "gpt-4"
            }
        });

        let notification = JsonRpcResponse::notification("message", params.clone());
        let result = notification.result.unwrap();
        assert_eq!(result["params"], params);
    }
}
