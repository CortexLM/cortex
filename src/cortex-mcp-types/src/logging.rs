//! Logging types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Log level.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Debug level.
    Debug,
    /// Info level.
    Info,
    /// Notice level.
    Notice,
    /// Warning level.
    Warning,
    /// Error level.
    Error,
    /// Critical level.
    Critical,
    /// Alert level.
    Alert,
    /// Emergency level.
    Emergency,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Debug => write!(f, "debug"),
            Self::Info => write!(f, "info"),
            Self::Notice => write!(f, "notice"),
            Self::Warning => write!(f, "warning"),
            Self::Error => write!(f, "error"),
            Self::Critical => write!(f, "critical"),
            Self::Alert => write!(f, "alert"),
            Self::Emergency => write!(f, "emergency"),
        }
    }
}

/// Set log level parameters.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetLogLevelParams {
    /// Log level to set.
    pub level: LogLevel,
}

/// Log message notification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LogMessage {
    /// Log level.
    pub level: LogLevel,
    /// Logger name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger: Option<String>,
    /// Log data.
    pub data: Value,
}

impl LogMessage {
    /// Create a new log message.
    pub fn new(level: LogLevel, data: Value) -> Self {
        Self {
            level,
            logger: None,
            data,
        }
    }

    /// Set the logger name.
    pub fn with_logger(mut self, logger: impl Into<String>) -> Self {
        self.logger = Some(logger.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Critical);
    }
}
