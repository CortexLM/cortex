//! Execution configuration types.

use serde::{Deserialize, Serialize};

/// Execution configuration for runtime behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Maximum number of concurrent agent threads (default: 4)
    #[serde(default = "default_max_agent_threads")]
    pub max_agent_threads: usize,

    /// Maximum number of concurrent tool executions (default: 8)
    #[serde(default = "default_max_tool_threads")]
    pub max_tool_threads: usize,

    /// Default timeout for shell commands in seconds (default: 120)
    #[serde(default = "default_command_timeout")]
    pub command_timeout_seconds: u64,

    /// Default timeout for HTTP requests in seconds (default: 60)
    #[serde(default = "default_http_timeout")]
    pub http_timeout_seconds: u64,

    /// Maximum retries for failed API requests (default: 3)
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,

    /// Retry delay in milliseconds (default: 1000)
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,

    /// Enable streaming responses (default: true)
    #[serde(default = "default_true")]
    pub streaming: bool,

    /// Maximum file size to read in bytes (default: 10MB)
    #[serde(default = "default_max_file_size")]
    pub max_file_size_bytes: u64,

    /// Maximum number of files to process in batch operations (default: 100)
    #[serde(default = "default_max_batch_files")]
    pub max_batch_files: usize,

    /// Enable verbose output (default: false)
    #[serde(default)]
    pub verbose: bool,

    /// Working directory for command execution (default: current directory)
    #[serde(default)]
    pub working_directory: Option<String>,
}

fn default_max_agent_threads() -> usize {
    4
}

fn default_max_tool_threads() -> usize {
    8
}

fn default_command_timeout() -> u64 {
    120
}

fn default_http_timeout() -> u64 {
    60
}

fn default_max_retries() -> u32 {
    3
}

fn default_retry_delay_ms() -> u64 {
    1000
}

fn default_true() -> bool {
    true
}

fn default_max_file_size() -> u64 {
    10 * 1024 * 1024 // 10MB
}

fn default_max_batch_files() -> usize {
    100
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_agent_threads: default_max_agent_threads(),
            max_tool_threads: default_max_tool_threads(),
            command_timeout_seconds: default_command_timeout(),
            http_timeout_seconds: default_http_timeout(),
            max_retries: default_max_retries(),
            retry_delay_ms: default_retry_delay_ms(),
            streaming: default_true(),
            max_file_size_bytes: default_max_file_size(),
            max_batch_files: default_max_batch_files(),
            verbose: false,
            working_directory: None,
        }
    }
}
