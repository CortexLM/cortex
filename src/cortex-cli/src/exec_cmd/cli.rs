//! CLI argument definitions for exec command.

use std::path::PathBuf;

use clap::Parser;

use super::autonomy::AutonomyLevel;
use super::output::{ExecInputFormat, ExecOutputFormat};

/// Execute a single command in non-interactive (headless) mode.
///
/// Designed for CI/CD pipelines, shell scripts, and batch processing.
/// Unlike the interactive CLI, `cortex exec` runs as a one-shot command
/// that completes a task and exits.
#[derive(Debug, Parser)]
#[command(allow_hyphen_values = true)]
pub struct ExecCli {
    /// The prompt to execute.
    /// Can also be provided via stdin or --file.
    /// Prompts starting with a dash (e.g., "--help explain this") are supported.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub prompt: Vec<String>,

    /// Read prompt from file.
    #[arg(short = 'f', long = "file", value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// Output format.
    #[arg(
        short = 'o',
        long = "output-format",
        value_enum,
        default_value = "text"
    )]
    pub output_format: ExecOutputFormat,

    /// Input format (for multi-turn sessions).
    #[arg(long = "input-format", value_enum, default_value = "text")]
    pub input_format: ExecInputFormat,

    /// Autonomy level for operations.
    /// - read-only: No modifications (default, safest)
    /// - low: Basic file operations only
    /// - medium: Package install, builds, local git
    /// - high: Full access including git push
    #[arg(long = "auto", value_enum)]
    pub autonomy: Option<AutonomyLevel>,

    /// Skip ALL permission checks (DANGEROUS).
    /// Only use in completely isolated environments like Docker containers.
    /// Cannot be combined with --auto.
    #[arg(long = "skip-permissions-unsafe", conflicts_with = "autonomy")]
    pub skip_permissions: bool,

    /// Model ID to use.
    #[arg(short = 'm', long = "model")]
    pub model: Option<String>,

    /// Model ID to use for spec mode.
    #[arg(long = "spec-model")]
    pub spec_model: Option<String>,

    /// Start in specification mode (plan before executing).
    #[arg(long = "use-spec")]
    pub use_spec: bool,

    /// Reasoning effort level.
    #[arg(short = 'r', long = "reasoning-effort")]
    pub reasoning_effort: Option<String>,

    /// Session ID to continue (requires a prompt).
    #[arg(short = 's', long = "session-id")]
    pub session_id: Option<String>,

    /// Enable specific tools (comma or space separated).
    #[arg(long = "enabled-tools", value_delimiter = ',')]
    pub enabled_tools: Vec<String>,

    /// Disable specific tools (comma or space separated).
    #[arg(long = "disabled-tools", value_delimiter = ',')]
    pub disabled_tools: Vec<String>,

    /// List available tools for the selected model and exit.
    #[arg(long = "list-tools")]
    pub list_tools: bool,

    /// Working directory path.
    #[arg(long = "cwd", value_name = "PATH")]
    pub cwd: Option<PathBuf>,

    /// Maximum number of turns before stopping.
    #[arg(long = "max-turns", default_value = "100")]
    pub max_turns: usize,

    /// Timeout in seconds (0 for no timeout).
    #[arg(long = "timeout", default_value = "600")]
    pub timeout: u64,

    /// Image files to attach to the prompt.
    #[arg(short = 'i', long = "image", action = clap::ArgAction::Append)]
    pub images: Vec<PathBuf>,

    /// Verbose output (show tool calls, reasoning).
    #[arg(short = 'v', long = "verbose")]
    pub verbose: bool,

    /// Custom system prompt to use instead of the default.
    /// Defines the AI's persona and behavior for this execution.
    #[arg(long = "system")]
    pub system_prompt: Option<String>,

    /// Maximum tokens for response generation.
    /// Limits the length of the AI's response.
    #[arg(long = "max-tokens")]
    pub max_tokens: Option<u32>,

    // ═══════════════════════════════════════════════════════════════════════════
    // Additional Flags (Issues #2715-2740)
    // ═══════════════════════════════════════════════════════════════════════════
    /// Include the prompt in the output (echo mode).
    /// When enabled, the original prompt will be included at the beginning of the response.
    #[arg(long = "echo", alias = "include-prompt")]
    pub echo: bool,

    /// User identifier for tracking and rate limiting.
    /// This ID is passed to the API for usage tracking purposes.
    #[arg(long = "user", value_name = "USER_ID")]
    pub user: Option<String>,

    /// Suffix text for completion insertion mode.
    /// The model will generate text to insert between the prompt and this suffix.
    #[arg(long = "suffix", value_name = "TEXT")]
    pub suffix: Option<String>,

    /// Response format for structured output.
    /// Valid values: text, json, json_object
    #[arg(long = "response-format", alias = "format-type", value_name = "FORMAT")]
    pub response_format: Option<String>,

    /// URLs to fetch and include in the context.
    /// Content from these URLs will be fetched and added to the prompt.
    #[arg(long = "url", action = clap::ArgAction::Append, value_name = "URL")]
    pub urls: Vec<String>,

    /// Read input from the system clipboard.
    /// The clipboard content will be appended to the prompt.
    #[arg(long = "clipboard", alias = "paste")]
    pub clipboard: bool,

    /// Include current git diff in the context.
    /// Useful for code review tasks.
    #[arg(long = "git-diff", alias = "diff")]
    pub git_diff: bool,

    /// Include only files matching these patterns.
    /// Supports glob patterns like "*.py" or "src/**/*.rs".
    #[arg(long = "include", action = clap::ArgAction::Append, value_name = "PATTERN")]
    pub include_patterns: Vec<String>,

    /// Exclude files matching these patterns.
    /// Supports glob patterns like "*.test.js" or "node_modules/**".
    #[arg(long = "exclude", action = clap::ArgAction::Append, value_name = "PATTERN")]
    pub exclude_patterns: Vec<String>,

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
    #[arg(short = 'n', long = "n")]
    pub num_completions: Option<u32>,

    /// Generate best_of completions and return the best one.
    /// Must be greater than n if n is specified.
    #[arg(long = "best-of")]
    pub best_of: Option<u32>,

    /// JSON schema for structured output.
    /// Can be inline JSON (e.g., '{"type":"object","properties":{...}}')
    /// or a path to a JSON schema file.
    #[arg(long = "output-schema", value_name = "SCHEMA")]
    pub output_schema: Option<String>,
}
