//! Output format system for Cortex CLI.
//!
//! Supports multiple output formats for different use cases:
//! - text: Human-readable output (default)
//! - json: Structured JSON for automation
//! - stream-json: Streaming JSONL for real-time processing
//! - stream-jsonrpc: JSON-RPC for SDK integration

#![allow(clippy::print_stdout, clippy::print_stderr)]

use serde::{Deserialize, Serialize};
use std::io::Write;

/// Output format for CLI results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Human-readable text output.
    #[default]
    Text,
    /// Structured JSON output.
    Json,
    /// Streaming JSONL (one JSON object per line).
    StreamJson,
    /// JSON-RPC streaming format for SDK integration.
    StreamJsonRpc,
}

impl OutputFormat {
    /// Parse from string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "text" | "human" => Some(Self::Text),
            "json" => Some(Self::Json),
            "stream-json" | "streamjson" | "jsonl" | "debug" => Some(Self::StreamJson),
            "stream-jsonrpc" | "jsonrpc" => Some(Self::StreamJsonRpc),
            _ => None,
        }
    }

    /// Check if this is a streaming format.
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::StreamJson | Self::StreamJsonRpc)
    }

    /// Check if this is a JSON format.
    pub fn is_json(&self) -> bool {
        !matches!(self, Self::Text)
    }
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text => write!(f, "text"),
            Self::Json => write!(f, "json"),
            Self::StreamJson => write!(f, "stream-json"),
            Self::StreamJsonRpc => write!(f, "stream-jsonrpc"),
        }
    }
}

/// Final result output for JSON format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonResult {
    #[serde(rename = "type")]
    pub result_type: String,
    pub subtype: String,
    pub is_error: bool,
    pub duration_ms: u64,
    pub num_turns: u32,
    pub result: String,
    pub session_id: String,
}

impl JsonResult {
    /// Create a success result.
    pub fn success(result: String, session_id: String, duration_ms: u64, num_turns: u32) -> Self {
        Self {
            result_type: "result".to_string(),
            subtype: "success".to_string(),
            is_error: false,
            duration_ms,
            num_turns,
            result,
            session_id,
        }
    }

    /// Create an error result.
    pub fn error(message: String, session_id: String, duration_ms: u64) -> Self {
        Self {
            result_type: "result".to_string(),
            subtype: "error".to_string(),
            is_error: true,
            duration_ms,
            num_turns: 0,
            result: message,
            session_id,
        }
    }
}

/// Streaming event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Session initialization.
    System {
        subtype: String,
        cwd: String,
        session_id: String,
        tools: Vec<String>,
        model: String,
    },
    /// User or assistant message.
    Message {
        role: String,
        id: String,
        text: String,
        timestamp: u64,
        session_id: String,
    },
    /// Tool call by the agent.
    ToolCall {
        id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        #[serde(rename = "toolId")]
        tool_id: String,
        #[serde(rename = "toolName")]
        tool_name: String,
        parameters: serde_json::Value,
        timestamp: u64,
        session_id: String,
    },
    /// Result from tool execution.
    ToolResult {
        id: String,
        #[serde(rename = "messageId")]
        message_id: String,
        #[serde(rename = "toolId")]
        tool_id: String,
        #[serde(rename = "isError")]
        is_error: bool,
        value: String,
        timestamp: u64,
        session_id: String,
    },
    /// Session completion.
    Completion {
        #[serde(rename = "finalText")]
        final_text: String,
        #[serde(rename = "numTurns")]
        num_turns: u32,
        #[serde(rename = "durationMs")]
        duration_ms: u64,
        session_id: String,
        timestamp: u64,
    },
    /// Error event.
    Error {
        message: String,
        code: Option<String>,
        session_id: String,
        timestamp: u64,
    },
}

impl StreamEvent {
    /// Get current timestamp in milliseconds.
    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Create a system init event.
    pub fn system_init(cwd: &str, session_id: &str, tools: Vec<String>, model: &str) -> Self {
        Self::System {
            subtype: "init".to_string(),
            cwd: cwd.to_string(),
            session_id: session_id.to_string(),
            tools,
            model: model.to_string(),
        }
    }

    /// Create a user message event.
    pub fn user_message(id: &str, text: &str, session_id: &str) -> Self {
        Self::Message {
            role: "user".to_string(),
            id: id.to_string(),
            text: text.to_string(),
            timestamp: Self::now(),
            session_id: session_id.to_string(),
        }
    }

    /// Create an assistant message event.
    pub fn assistant_message(id: &str, text: &str, session_id: &str) -> Self {
        Self::Message {
            role: "assistant".to_string(),
            id: id.to_string(),
            text: text.to_string(),
            timestamp: Self::now(),
            session_id: session_id.to_string(),
        }
    }

    /// Create a tool call event.
    pub fn tool_call(
        id: &str,
        message_id: &str,
        tool_name: &str,
        parameters: serde_json::Value,
        session_id: &str,
    ) -> Self {
        Self::ToolCall {
            id: id.to_string(),
            message_id: message_id.to_string(),
            tool_id: tool_name.to_string(),
            tool_name: tool_name.to_string(),
            parameters,
            timestamp: Self::now(),
            session_id: session_id.to_string(),
        }
    }

    /// Create a tool result event.
    pub fn tool_result(
        id: &str,
        message_id: &str,
        tool_id: &str,
        value: &str,
        is_error: bool,
        session_id: &str,
    ) -> Self {
        Self::ToolResult {
            id: id.to_string(),
            message_id: message_id.to_string(),
            tool_id: tool_id.to_string(),
            is_error,
            value: value.to_string(),
            timestamp: Self::now(),
            session_id: session_id.to_string(),
        }
    }

    /// Create a completion event.
    pub fn completion(
        final_text: &str,
        num_turns: u32,
        duration_ms: u64,
        session_id: &str,
    ) -> Self {
        Self::Completion {
            final_text: final_text.to_string(),
            num_turns,
            duration_ms,
            session_id: session_id.to_string(),
            timestamp: Self::now(),
        }
    }

    /// Create an error event.
    pub fn error(message: &str, code: Option<&str>, session_id: &str) -> Self {
        Self::Error {
            message: message.to_string(),
            code: code.map(std::string::ToString::to_string),
            session_id: session_id.to_string(),
            timestamp: Self::now(),
        }
    }
}

/// Output writer that handles different formats.
pub struct OutputWriter {
    format: OutputFormat,
    session_id: String,
}

impl OutputWriter {
    /// Create a new output writer.
    pub fn new(format: OutputFormat, session_id: &str) -> Self {
        Self {
            format,
            session_id: session_id.to_string(),
        }
    }

    /// Write a stream event.
    pub fn write_event(&self, event: &StreamEvent) -> std::io::Result<()> {
        if self.format.is_streaming() {
            let json = serde_json::to_string(event)?;
            let mut stdout = std::io::stdout();
            writeln!(stdout, "{json}")?;
            stdout.flush()?;
        }
        Ok(())
    }

    /// Write final result.
    pub fn write_result(
        &self,
        result: &str,
        is_error: bool,
        duration_ms: u64,
        num_turns: u32,
    ) -> std::io::Result<()> {
        match self.format {
            OutputFormat::Text => {
                if is_error {
                    eprintln!("{result}");
                } else {
                    println!("{result}");
                }
            }
            OutputFormat::Json => {
                let json_result = if is_error {
                    JsonResult::error(result.to_string(), self.session_id.clone(), duration_ms)
                } else {
                    JsonResult::success(
                        result.to_string(),
                        self.session_id.clone(),
                        duration_ms,
                        num_turns,
                    )
                };
                let json = serde_json::to_string_pretty(&json_result)?;
                println!("{json}");
            }
            OutputFormat::StreamJson | OutputFormat::StreamJsonRpc => {
                let event =
                    StreamEvent::completion(result, num_turns, duration_ms, &self.session_id);
                self.write_event(&event)?;
            }
        }
        Ok(())
    }

    /// Write text output (for Text format only).
    pub fn write_text(&self, text: &str) -> std::io::Result<()> {
        if matches!(self.format, OutputFormat::Text) {
            print!("{text}");
            std::io::stdout().flush()?;
        }
        Ok(())
    }

    /// Write error message.
    pub fn write_error(&self, message: &str) -> std::io::Result<()> {
        match self.format {
            OutputFormat::Text => {
                eprintln!("Error: {message}");
            }
            OutputFormat::Json => {
                let json_result =
                    JsonResult::error(message.to_string(), self.session_id.clone(), 0);
                let json = serde_json::to_string_pretty(&json_result)?;
                eprintln!("{json}");
            }
            OutputFormat::StreamJson | OutputFormat::StreamJsonRpc => {
                let event = StreamEvent::error(message, None, &self.session_id);
                let json = serde_json::to_string(&event)?;
                eprintln!("{json}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parsing() {
        assert_eq!(OutputFormat::from_str("text"), Some(OutputFormat::Text));
        assert_eq!(OutputFormat::from_str("json"), Some(OutputFormat::Json));
        assert_eq!(
            OutputFormat::from_str("stream-json"),
            Some(OutputFormat::StreamJson)
        );
        assert_eq!(
            OutputFormat::from_str("debug"),
            Some(OutputFormat::StreamJson)
        );
    }

    #[test]
    fn test_json_result() {
        let result = JsonResult::success(
            "Task completed".to_string(),
            "session-123".to_string(),
            5000,
            3,
        );

        assert!(!result.is_error);
        assert_eq!(result.num_turns, 3);

        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("success"));
    }

    #[test]
    fn test_stream_events() {
        let event = StreamEvent::system_init(
            "/home/user",
            "session-1",
            vec!["Read".to_string()],
            "claude",
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("system"));

        let event = StreamEvent::tool_call(
            "call-1",
            "msg-1",
            "Read",
            serde_json::json!({"path": "/test"}),
            "session-1",
        );
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("tool_call"));
    }
}
