//! Output and input format definitions for exec mode.

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Output format for exec mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecOutputFormat {
    /// Human-readable text output (default).
    #[default]
    Text,

    /// Structured JSON output with final result.
    Json,

    /// Streaming JSON Lines showing execution in real-time.
    /// Each line is a separate JSON event.
    StreamJson,

    /// Deprecated alias for stream-json.
    Debug,

    /// JSON-RPC streaming protocol for multi-turn conversations.
    /// Reads JSONL from stdin, outputs JSON-RPC responses.
    StreamJsonrpc,
}

impl std::fmt::Display for ExecOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecOutputFormat::Text => write!(f, "text"),
            ExecOutputFormat::Json => write!(f, "json"),
            ExecOutputFormat::StreamJson => write!(f, "stream-json"),
            ExecOutputFormat::Debug => write!(f, "debug"),
            ExecOutputFormat::StreamJsonrpc => write!(f, "stream-jsonrpc"),
        }
    }
}

/// Input format for exec mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExecInputFormat {
    /// Standard text input (default).
    #[default]
    Text,

    /// JSON-RPC streaming for multi-turn sessions.
    StreamJsonrpc,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_display() {
        assert_eq!(ExecOutputFormat::Text.to_string(), "text");
        assert_eq!(ExecOutputFormat::Json.to_string(), "json");
        assert_eq!(ExecOutputFormat::StreamJson.to_string(), "stream-json");
        assert_eq!(
            ExecOutputFormat::StreamJsonrpc.to_string(),
            "stream-jsonrpc"
        );
    }
}
