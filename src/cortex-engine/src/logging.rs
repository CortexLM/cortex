//! Logging utilities.
//!
//! Provides structured logging, log rotation, and log formatting
//! for the Cortex CLI.

#![allow(clippy::print_stderr)]

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

/// Log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum LogLevel {
    /// Trace level (most verbose).
    Trace,
    /// Debug level.
    Debug,
    /// Info level.
    #[default]
    Info,
    /// Warning level.
    Warn,
    /// Error level.
    Error,
    /// Fatal level (least verbose).
    Fatal,
}

impl LogLevel {
    /// Get level name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Trace => "TRACE",
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
            Self::Fatal => "FATAL",
        }
    }

    /// Get short name.
    pub fn short(&self) -> &'static str {
        match self {
            Self::Trace => "TRC",
            Self::Debug => "DBG",
            Self::Info => "INF",
            Self::Warn => "WRN",
            Self::Error => "ERR",
            Self::Fatal => "FTL",
        }
    }

    /// Get ANSI color code.
    pub fn color(&self) -> &'static str {
        match self {
            Self::Trace => "\x1b[90m",
            Self::Debug => "\x1b[36m",
            Self::Info => "\x1b[32m",
            Self::Warn => "\x1b[33m",
            Self::Error => "\x1b[31m",
            Self::Fatal => "\x1b[35m",
        }
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "trace" => Ok(Self::Trace),
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" | "warning" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            "fatal" => Ok(Self::Fatal),
            _ => Err(format!("Unknown log level: {s}")),
        }
    }
}

/// Log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// Timestamp.
    pub timestamp: u64,
    /// Log level.
    pub level: LogLevel,
    /// Message.
    pub message: String,
    /// Target (module/component).
    pub target: Option<String>,
    /// Fields.
    pub fields: HashMap<String, serde_json::Value>,
    /// Span/trace ID.
    pub span_id: Option<String>,
}

impl LogEntry {
    /// Create a new log entry.
    pub fn new(level: LogLevel, message: impl Into<String>) -> Self {
        Self {
            timestamp: timestamp_now(),
            level,
            message: message.into(),
            target: None,
            fields: HashMap::new(),
            span_id: None,
        }
    }

    /// Set target.
    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Add field.
    pub fn field(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.fields.insert(key.into(), v);
        }
        self
    }

    /// Set span ID.
    pub fn span(mut self, span_id: impl Into<String>) -> Self {
        self.span_id = Some(span_id.into());
        self
    }

    /// Format as text.
    pub fn format_text(&self, colored: bool) -> String {
        let ts = format_timestamp(self.timestamp);
        let level = if colored {
            format!("{}{}\x1b[0m", self.level.color(), self.level.short())
        } else {
            self.level.short().to_string()
        };

        let target = self.target.as_deref().unwrap_or("-");

        if self.fields.is_empty() {
            format!("{} {} [{}] {}", ts, level, target, self.message)
        } else {
            let fields: Vec<String> = self
                .fields
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect();
            format!(
                "{} {} [{}] {} {}",
                ts,
                level,
                target,
                self.message,
                fields.join(" ")
            )
        }
    }

    /// Format as JSON.
    pub fn format_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| self.message.clone())
    }
}

/// Log format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum LogFormat {
    /// Plain text.
    #[default]
    Text,
    /// JSON.
    Json,
    /// Compact text.
    Compact,
}

/// Logger configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggerConfig {
    /// Minimum log level.
    pub level: LogLevel,
    /// Log format.
    pub format: LogFormat,
    /// Output to stderr.
    pub stderr: bool,
    /// File output path.
    pub file: Option<PathBuf>,
    /// Enable colors.
    pub colors: bool,
    /// Include timestamps.
    pub timestamps: bool,
    /// Maximum file size (bytes) before rotation.
    pub max_file_size: u64,
    /// Maximum number of log files.
    pub max_files: u32,
}

impl Default for LoggerConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
            format: LogFormat::Text,
            stderr: true,
            file: None,
            colors: true,
            timestamps: true,
            max_file_size: 10 * 1024 * 1024, // 10MB
            max_files: 5,
        }
    }
}

/// Logger.
pub struct Logger {
    /// Configuration.
    config: LoggerConfig,
    /// File writer.
    file_writer: Option<RwLock<BufWriter<File>>>,
    /// Current file size.
    current_file_size: RwLock<u64>,
}

impl Logger {
    /// Create a new logger.
    pub fn new(config: LoggerConfig) -> std::io::Result<Self> {
        let file_writer = if let Some(ref path) = config.file {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let file = OpenOptions::new().create(true).append(true).open(path)?;
            let size = file.metadata()?.len();
            Some((RwLock::new(BufWriter::new(file)), size))
        } else {
            None
        };

        let (writer, size) = match file_writer {
            Some((w, s)) => (Some(w), s),
            None => (None, 0),
        };

        Ok(Self {
            config,
            file_writer: writer,
            current_file_size: RwLock::new(size),
        })
    }

    /// Create with default config.
    pub fn default_logger() -> Self {
        Self::new(LoggerConfig::default()).unwrap()
    }

    /// Log an entry.
    pub async fn log(&self, entry: LogEntry) {
        if entry.level < self.config.level {
            return;
        }

        let formatted = match self.config.format {
            LogFormat::Text => entry.format_text(self.config.colors),
            LogFormat::Json => entry.format_json(),
            LogFormat::Compact => format!(
                "{} {} {}",
                entry.level.short(),
                entry.target.as_deref().unwrap_or("-"),
                entry.message
            ),
        };

        // Write to stderr
        if self.config.stderr {
            eprintln!("{formatted}");
        }

        // Write to file
        if let Some(ref writer) = self.file_writer {
            let bytes = formatted.as_bytes();
            let mut w = writer.write().await;
            if let Err(e) = writeln!(w, "{formatted}") {
                eprintln!("Failed to write log: {e}");
            }
            let _ = w.flush();

            // Update size and check for rotation
            let mut size = self.current_file_size.write().await;
            *size += bytes.len() as u64 + 1;

            if *size >= self.config.max_file_size {
                drop(w);
                drop(size);
                self.rotate().await;
            }
        }
    }

    /// Rotate log files.
    async fn rotate(&self) {
        if let Some(ref path) = self.config.file {
            // Rotate existing files
            for i in (0..self.config.max_files - 1).rev() {
                let from = if i == 0 {
                    path.clone()
                } else {
                    path.with_extension(format!("log.{i}"))
                };
                let to = path.with_extension(format!("log.{}", i + 1));

                if from.exists() {
                    let _ = fs::rename(&from, &to);
                }
            }

            // Delete oldest if over limit
            let oldest = path.with_extension(format!("log.{}", self.config.max_files));
            if oldest.exists() {
                let _ = fs::remove_file(oldest);
            }

            // Reset counter
            *self.current_file_size.write().await = 0;
        }
    }

    /// Log at trace level.
    pub async fn trace(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Trace, message)).await;
    }

    /// Log at debug level.
    pub async fn debug(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Debug, message)).await;
    }

    /// Log at info level.
    pub async fn info(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Info, message)).await;
    }

    /// Log at warn level.
    pub async fn warn(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Warn, message)).await;
    }

    /// Log at error level.
    pub async fn error(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Error, message)).await;
    }

    /// Log at fatal level.
    pub async fn fatal(&self, message: impl Into<String>) {
        self.log(LogEntry::new(LogLevel::Fatal, message)).await;
    }

    /// Create a child logger with target.
    pub fn with_target(&self, target: impl Into<String>) -> TargetedLogger<'_> {
        TargetedLogger {
            logger: self,
            target: target.into(),
        }
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::default_logger()
    }
}

/// Logger with a fixed target.
pub struct TargetedLogger<'a> {
    logger: &'a Logger,
    target: String,
}

impl<'a> TargetedLogger<'a> {
    /// Log at trace level.
    pub async fn trace(&self, message: impl Into<String>) {
        self.logger
            .log(LogEntry::new(LogLevel::Trace, message).target(&self.target))
            .await;
    }

    /// Log at debug level.
    pub async fn debug(&self, message: impl Into<String>) {
        self.logger
            .log(LogEntry::new(LogLevel::Debug, message).target(&self.target))
            .await;
    }

    /// Log at info level.
    pub async fn info(&self, message: impl Into<String>) {
        self.logger
            .log(LogEntry::new(LogLevel::Info, message).target(&self.target))
            .await;
    }

    /// Log at warn level.
    pub async fn warn(&self, message: impl Into<String>) {
        self.logger
            .log(LogEntry::new(LogLevel::Warn, message).target(&self.target))
            .await;
    }

    /// Log at error level.
    pub async fn error(&self, message: impl Into<String>) {
        self.logger
            .log(LogEntry::new(LogLevel::Error, message).target(&self.target))
            .await;
    }
}

/// Log collector for testing.
pub struct LogCollector {
    entries: RwLock<Vec<LogEntry>>,
    level: LogLevel,
}

impl LogCollector {
    /// Create a new collector.
    pub fn new(level: LogLevel) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            level,
        }
    }

    /// Collect a log entry.
    pub async fn collect(&self, entry: LogEntry) {
        if entry.level >= self.level {
            self.entries.write().await.push(entry);
        }
    }

    /// Get collected entries.
    pub async fn entries(&self) -> Vec<LogEntry> {
        self.entries.read().await.clone()
    }

    /// Clear entries.
    pub async fn clear(&self) {
        self.entries.write().await.clear();
    }

    /// Get entry count.
    pub async fn count(&self) -> usize {
        self.entries.read().await.len()
    }

    /// Find entries by level.
    pub async fn by_level(&self, level: LogLevel) -> Vec<LogEntry> {
        self.entries
            .read()
            .await
            .iter()
            .filter(|e| e.level == level)
            .cloned()
            .collect()
    }

    /// Find entries containing message.
    pub async fn containing(&self, text: &str) -> Vec<LogEntry> {
        self.entries
            .read()
            .await
            .iter()
            .filter(|e| e.message.contains(text))
            .cloned()
            .collect()
    }
}

impl Default for LogCollector {
    fn default() -> Self {
        Self::new(LogLevel::Trace)
    }
}

/// Span for tracing.
pub struct Span {
    /// Span ID.
    pub id: String,
    /// Parent span ID.
    pub parent_id: Option<String>,
    /// Name.
    pub name: String,
    /// Start time.
    pub start_time: u64,
    /// Fields.
    pub fields: HashMap<String, serde_json::Value>,
}

impl Span {
    /// Create a new span.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: generate_span_id(),
            parent_id: None,
            name: name.into(),
            start_time: timestamp_now(),
            fields: HashMap::new(),
        }
    }

    /// Create a child span.
    pub fn child(&self, name: impl Into<String>) -> Self {
        Self {
            id: generate_span_id(),
            parent_id: Some(self.id.clone()),
            name: name.into(),
            start_time: timestamp_now(),
            fields: HashMap::new(),
        }
    }

    /// Add field.
    pub fn field(mut self, key: impl Into<String>, value: impl Serialize) -> Self {
        if let Ok(v) = serde_json::to_value(value) {
            self.fields.insert(key.into(), v);
        }
        self
    }

    /// Get duration.
    pub fn duration(&self) -> u64 {
        timestamp_now().saturating_sub(self.start_time)
    }
}

/// Format timestamp.
fn format_timestamp(ts: u64) -> String {
    let secs = ts;
    let hours = (secs / 3600) % 24;
    let mins = (secs / 60) % 60;
    let secs = secs % 60;

    format!("{hours:02}:{mins:02}:{secs:02}")
}

/// Get current timestamp.
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Generate span ID.
fn generate_span_id() -> String {
    use std::time::SystemTime;
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("span_{ts:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_level_ordering() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warn);
        assert!(LogLevel::Warn < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Fatal);
    }

    #[test]
    fn test_log_level_parse() {
        assert_eq!("info".parse::<LogLevel>().unwrap(), LogLevel::Info);
        assert_eq!("DEBUG".parse::<LogLevel>().unwrap(), LogLevel::Debug);
        assert_eq!("warning".parse::<LogLevel>().unwrap(), LogLevel::Warn);
    }

    #[test]
    fn test_log_entry() {
        let entry = LogEntry::new(LogLevel::Info, "Test message")
            .target("test")
            .field("key", "value");

        assert_eq!(entry.level, LogLevel::Info);
        assert_eq!(entry.message, "Test message");
        assert_eq!(entry.target, Some("test".to_string()));
        assert!(entry.fields.contains_key("key"));
    }

    #[test]
    fn test_log_entry_format_text() {
        let entry = LogEntry::new(LogLevel::Info, "Hello");
        let formatted = entry.format_text(false);
        assert!(formatted.contains("INF"));
        assert!(formatted.contains("Hello"));
    }

    #[test]
    fn test_log_entry_format_json() {
        let entry = LogEntry::new(LogLevel::Error, "Error occurred").field("code", 500);
        let json = entry.format_json();
        assert!(json.contains("\"level\":\"error\""));
        assert!(json.contains("\"code\":500"));
    }

    #[tokio::test]
    async fn test_log_collector() {
        let collector = LogCollector::new(LogLevel::Info);

        collector
            .collect(LogEntry::new(LogLevel::Debug, "debug"))
            .await;
        collector
            .collect(LogEntry::new(LogLevel::Info, "info"))
            .await;
        collector
            .collect(LogEntry::new(LogLevel::Error, "error"))
            .await;

        // Debug is below Info threshold
        assert_eq!(collector.count().await, 2);

        let errors = collector.by_level(LogLevel::Error).await;
        assert_eq!(errors.len(), 1);
    }

    #[test]
    fn test_span() {
        let parent = Span::new("parent");
        let child = parent.child("child");

        assert_eq!(child.parent_id, Some(parent.id.clone()));
        assert_ne!(child.id, parent.id);
    }
}
