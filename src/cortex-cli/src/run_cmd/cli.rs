//! CLI argument definitions for the run command.

use clap::{Parser, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Output format options.
#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// Human-readable formatted output with colors and styling.
    #[default]
    Default,
    /// Raw JSON events for machine processing.
    Json,
    /// JSON Lines format (one JSON object per line).
    Jsonl,
}

impl std::fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OutputFormat::Default => write!(f, "default"),
            OutputFormat::Json => write!(f, "json"),
            OutputFormat::Jsonl => write!(f, "jsonl"),
        }
    }
}

/// Run CLI command for non-interactive execution.
#[derive(Debug, Parser)]
#[command(allow_hyphen_values = true)]
pub struct RunCli {
    /// Message to send to the AI agent.
    /// Multiple arguments are joined with spaces.
    /// Note: Prompts starting with a dash (e.g., "--help explain this flag")
    /// are supported and won't be interpreted as flags.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub message: Vec<String>,

    /// Execute a predefined command instead of a prompt.
    /// Use message arguments as command arguments.
    #[arg(long = "command")]
    pub command: Option<String>,

    /// Continue the most recent session.
    #[arg(short = 'c', long = "continue")]
    pub continue_session: bool,

    /// Specify a session ID to continue.
    #[arg(short = 's', long = "session", conflicts_with = "continue_session")]
    pub session_id: Option<String>,

    /// Automatically share the session and print the share URL.
    #[arg(long = "share")]
    pub share: bool,

    /// Model to use in provider/model format (e.g., anthropic/claude-3-5-sonnet).
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// Agent to use for this request.
    #[arg(long = "agent")]
    pub agent: Option<String>,

    /// Output format: default (formatted), json, or jsonl.
    #[arg(long = "format", value_enum, default_value_t = OutputFormat::Default)]
    pub format: OutputFormat,

    /// Output format alias (same as --format).
    /// Valid values: default, json, jsonl.
    #[arg(long = "output", value_enum, conflicts_with = "format")]
    pub output: Option<OutputFormat>,

    /// File(s) to attach to the message.
    /// Can be specified multiple times.
    #[arg(short = 'f', long = "file", action = clap::ArgAction::Append)]
    pub files: Vec<PathBuf>,

    /// Title for the session.
    /// If empty string is provided, uses truncated prompt.
    #[arg(long = "title")]
    pub title: Option<String>,

    /// Attach to a running Cortex server instead of starting locally.
    /// Value should be the server URL (e.g., "http://localhost:3000").
    /// Use this to connect to a remote Cortex instance or an existing
    /// local server started with 'cortex serve'.
    #[arg(long = "attach", value_name = "URL")]
    pub attach: Option<String>,

    /// Port for the local server (defaults to random port).
    #[arg(long = "port")]
    pub port: Option<u16>,

    /// Model temperature (0.0-2.0).
    /// Lower values make output more deterministic.
    #[arg(short = 't', long = "temperature")]
    pub temperature: Option<f32>,

    /// Top-p (nucleus) sampling parameter.
    /// Controls diversity of token selection.
    #[arg(long = "top-p")]
    pub top_p: Option<f32>,

    /// Top-k sampling parameter.
    /// Limits token selection to k most probable tokens.
    /// Note: Not all providers support combining --top-k with --temperature.
    #[arg(long = "top-k")]
    pub top_k: Option<u32>,

    /// Random seed for reproducible outputs.
    #[arg(long = "seed")]
    pub seed: Option<u64>,

    /// Send a desktop notification when the task completes.
    #[arg(short = 'n', long = "notification")]
    pub notification: bool,

    /// Stream output as it arrives (default behavior).
    /// Use --no-stream to buffer and wait for the complete response.
    #[arg(long = "stream", default_value_t = true, action = clap::ArgAction::SetTrue, overrides_with = "no_stream")]
    pub stream: bool,

    /// Disable streaming - wait for complete response before outputting.
    /// This is the opposite of --stream (which is enabled by default).
    #[arg(long = "no-stream", action = clap::ArgAction::SetTrue)]
    pub no_stream: bool,

    /// Copy the final AI response to the system clipboard.
    #[arg(short = 'C', long = "copy")]
    pub copy: bool,

    /// Save the final response to a file.
    /// Parent directory will be created automatically if it doesn't exist.
    #[arg(short = 'o', long = "output-file", value_name = "FILE")]
    pub output_file: Option<PathBuf>,

    /// Working directory override.
    #[arg(long = "cwd", value_name = "DIR")]
    pub cwd: Option<PathBuf>,

    /// Additional directories that should be writable.
    /// Can be specified multiple times to add multiple directories.
    #[arg(long = "add-dir", value_name = "DIR", action = clap::ArgAction::Append)]
    pub add_dir: Vec<PathBuf>,

    /// Enable verbose/debug output.
    #[arg(long = "verbose", short = 'v')]
    pub verbose: bool,

    /// Timeout in seconds (0 for no timeout).
    #[arg(long = "timeout", default_value_t = 0)]
    pub timeout: u64,

    /// Preview what would be sent without executing.
    /// Shows estimated token counts including system prompt and tool definitions.
    #[arg(long = "dry-run")]
    pub dry_run: bool,

    /// Maximum tokens for response (used for token validation).
    /// If specified, cortex will validate that prompt + max_tokens
    /// does not exceed the model's context limit before making the API call.
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,

    /// Custom system prompt to use instead of the default.
    /// Defines the AI's persona and behavior.
    #[arg(long = "system")]
    pub system_prompt: Option<String>,

    /// Path to a JSON schema file for structured output.
    /// When provided, the AI response will be validated against this schema.
    #[arg(long = "schema")]
    pub schema: Option<PathBuf>,

    /// Suppress non-essential output (quiet mode).
    /// Only show the final result or errors.
    #[arg(long = "quiet", short = 'q', conflicts_with = "verbose")]
    pub quiet: bool,

    /// Disable progress indicators (useful for CI/CD environments).
    #[arg(long = "no-progress")]
    pub no_progress: bool,

    /// Bypass any cached responses and force a fresh request.
    #[arg(long = "no-cache")]
    pub no_cache: bool,

    /// Number of times to retry failed requests (default: 0).
    #[arg(long = "retry", default_value_t = 0)]
    pub retry: u32,

    // ═══════════════════════════════════════════════════════════════════════════
    // LLM Generation Parameters (Issues #2703, #2704, #2711, #2712, #2714)
    // ═══════════════════════════════════════════════════════════════════════════
    /// Frequency penalty (-2.0 to 2.0).
    /// Positive values penalize tokens based on their frequency in the text so far,
    /// decreasing the likelihood of repeating the same content verbatim.
    #[arg(long = "frequency-penalty")]
    pub frequency_penalty: Option<f32>,

    /// Presence penalty (-2.0 to 2.0).
    /// Positive values penalize new tokens based on whether they appear in the text so far,
    /// increasing the likelihood of talking about new topics.
    #[arg(long = "presence-penalty")]
    pub presence_penalty: Option<f32>,

    /// Stop sequences (can be specified multiple times).
    /// Generation will stop when any of these sequences is encountered.
    #[arg(long = "stop", action = clap::ArgAction::Append)]
    pub stop_sequences: Vec<String>,

    /// Request log probabilities for output tokens.
    /// Returns the log probabilities of the most likely tokens (up to 5).
    #[arg(long = "logprobs")]
    pub logprobs: Option<u8>,

    /// Number of completions to generate.
    /// Returns multiple independent completions for the same prompt.
    #[arg(long = "n")]
    pub num_completions: Option<u32>,

    /// Generate best_of completions and return the best one.
    /// Must be greater than n if n is specified.
    #[arg(long = "best-of")]
    pub best_of: Option<u32>,
}

impl RunCli {
    /// Determine if streaming output is enabled.
    /// --no-stream takes precedence over --stream if both are specified.
    pub fn is_streaming_enabled(&self) -> bool {
        if self.no_stream {
            return false;
        }
        self.stream
    }
}
