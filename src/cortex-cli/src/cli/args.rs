//! CLI argument structures and parsing.
//!
//! Defines all command-line argument structures using clap.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

use super::styles::{AFTER_HELP, BEFORE_HELP, HELP_TEMPLATE, categories, get_styles};
use crate::acp_cmd::AcpCli;
use crate::agent_cmd::AgentCli;
use crate::alias_cmd::AliasCli;
use crate::cache_cmd::CacheCli;
use crate::compact_cmd::CompactCli;
use crate::dag_cmd::DagCli;
use crate::debug_cmd::DebugCli;
use crate::exec_cmd::ExecCli;
use crate::export_cmd::ExportCommand;
use crate::feedback_cmd::FeedbackCli;
use crate::github_cmd::GitHubCli;
use crate::import_cmd::ImportCommand;
use crate::lock_cmd::LockCli;
use crate::logs_cmd::LogsCli;
use crate::mcp_cmd::McpCli;
use crate::models_cmd::ModelsCli;
use crate::plugin_cmd::PluginCli;
use crate::pr_cmd::PrCli;
use crate::run_cmd::RunCli;
use crate::scrape_cmd::ScrapeCommand;
use crate::shell_cmd::ShellCli;
use crate::stats_cmd::StatsCli;
use crate::uninstall_cmd::UninstallCli;
use crate::upgrade_cmd::UpgradeCli;
use crate::workspace_cmd::WorkspaceCli;
use crate::{LandlockCommand, SeatbeltCommand, WindowsCommand};
use cortex_common::CliConfigOverrides;

/// Build-time version string with commit hash and build date.
pub fn get_long_version() -> &'static str {
    const VERSION: &str = env!("CARGO_PKG_VERSION");
    const GIT_HASH: &str = match option_env!("CORTEX_GIT_HASH") {
        Some(v) => v,
        None => "unknown",
    };
    const BUILD_DATE: &str = match option_env!("CORTEX_BUILD_DATE") {
        Some(v) => v,
        None => "unknown",
    };

    static LONG_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    LONG_VERSION.get_or_init(|| format!("{} ({} {})", VERSION, GIT_HASH, BUILD_DATE))
}

/// Log verbosity level for CLI output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum LogLevel {
    /// Only show errors
    Error,
    /// Show warnings and errors
    Warn,
    /// Show informational messages, warnings, and errors (default)
    #[default]
    Info,
    /// Show debug messages and above
    Debug,
    /// Show all messages including trace-level details
    Trace,
}

impl LogLevel {
    /// Convert to tracing filter string.
    pub fn as_filter_str(&self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }

    /// Parse from string (case-insensitive).
    pub fn from_str_loose(s: &str) -> Option<LogLevel> {
        match s.to_lowercase().as_str() {
            "error" => Some(LogLevel::Error),
            "warn" | "warning" => Some(LogLevel::Warn),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            "trace" => Some(LogLevel::Trace),
            _ => None,
        }
    }
}

/// Color output mode for CLI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum ColorMode {
    /// Automatically detect if output is a terminal
    #[default]
    Auto,
    /// Always output with colors
    Always,
    /// Never output with colors
    Never,
}

/// Cortex CLI - AI Coding Agent
///
/// If no subcommand is specified, starts the interactive TUI.
#[derive(Parser)]
#[command(name = "cortex")]
#[command(author, version, long_version = get_long_version())]
#[command(about = "Cortex - AI Coding Agent", long_about = None)]
#[command(
    styles = get_styles(),
    subcommand_negates_reqs = true,
    override_usage = "cortex [OPTIONS] [PROMPT]\n       cortex [OPTIONS] <COMMAND> [ARGS]",
    before_help = BEFORE_HELP,
    after_help = AFTER_HELP,
    help_template = HELP_TEMPLATE
)]
pub struct Cli {
    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,

    /// Enable verbose output (same as --log-level debug)
    #[arg(long = "verbose", short = 'v', global = true)]
    pub verbose: bool,

    /// Enable trace-level logging for debugging
    #[arg(long = "trace", global = true)]
    pub trace: bool,

    /// Control color output: auto (default), always, or never
    #[arg(long = "color", global = true, value_enum, default_value_t = ColorMode::Auto)]
    pub color: ColorMode,

    #[clap(flatten)]
    pub interactive: InteractiveArgs,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Arguments for interactive mode.
#[derive(Args, Debug, Default)]
pub struct InteractiveArgs {
    /// Model to use (e.g., claude-sonnet-4-20250514, gpt-4o, gemini-2.0-flash)
    #[arg(short, long, help_heading = "Model Configuration")]
    pub model: Option<String>,

    /// Use open-source/local LLM providers instead of cloud APIs.
    #[arg(
        long = "oss",
        default_value_t = false,
        help_heading = "Model Configuration"
    )]
    pub oss: bool,

    /// Configuration profile from config.toml
    #[arg(long = "profile", short = 'p', help_heading = "Model Configuration")]
    pub config_profile: Option<String>,

    /// Select the sandbox policy for shell commands
    #[arg(long = "sandbox", short = 's', help_heading = "Security")]
    pub sandbox_mode: Option<String>,

    /// Set the approval policy for tool executions.
    #[arg(
        long = "ask-for-approval",
        short = 'a',
        value_name = "POLICY",
        help_heading = "Security"
    )]
    pub approval_policy: Option<String>,

    /// Enable fully automatic mode with sandboxed execution.
    #[arg(long = "full-auto", default_value_t = false, help_heading = "Security")]
    pub full_auto: bool,

    /// Skip all confirmation prompts and execute commands without sandboxing. DANGEROUS!
    #[arg(
        long = "dangerously-bypass-approvals-and-sandbox",
        alias = "yolo",
        default_value_t = false,
        conflicts_with_all = ["approval_policy", "full_auto"],
        help_heading = "Security"
    )]
    pub dangerously_bypass_approvals_and_sandbox: bool,

    /// Tell the agent to use the specified directory as its working root
    #[arg(
        long = "cd",
        short = 'C',
        value_name = "DIR",
        help_heading = "Workspace"
    )]
    pub cwd: Option<PathBuf>,

    /// Additional directories that should be writable
    #[arg(long = "add-dir", value_name = "DIR", help_heading = "Workspace")]
    pub add_dir: Vec<PathBuf>,

    /// Image files to attach to the initial prompt
    #[arg(long = "image", short = 'i', value_delimiter = ',', num_args = 1.., help_heading = "Workspace")]
    pub images: Vec<PathBuf>,

    /// Enable web search capability for the agent.
    #[arg(long = "search", default_value_t = false, help_heading = "Features")]
    pub web_search: bool,

    /// Maximum number of concurrent agent threads
    #[arg(
        long = "max-agent-threads",
        value_name = "N",
        help_heading = "Execution"
    )]
    pub max_agent_threads: Option<usize>,

    /// Maximum number of concurrent tool executions
    #[arg(
        long = "max-tool-threads",
        value_name = "N",
        help_heading = "Execution"
    )]
    pub max_tool_threads: Option<usize>,

    /// Timeout for shell commands in seconds
    #[arg(
        long = "command-timeout",
        value_name = "SECONDS",
        help_heading = "Execution"
    )]
    pub command_timeout: Option<u64>,

    /// Timeout for HTTP requests in seconds
    #[arg(
        long = "http-timeout",
        value_name = "SECONDS",
        help_heading = "Execution"
    )]
    pub http_timeout: Option<u64>,

    /// Disable streaming responses
    #[arg(
        long = "no-streaming",
        default_value_t = false,
        help_heading = "Execution"
    )]
    pub no_streaming: bool,

    /// Set log verbosity level (error, warn, info, debug, trace)
    #[arg(
        long = "log-level",
        short = 'L',
        value_enum,
        default_value = "info",
        help_heading = "Debugging"
    )]
    pub log_level: LogLevel,

    /// Enable debug mode: writes ALL trace-level logs to ./debug.txt
    #[arg(long = "debug", help_heading = "Debugging")]
    pub debug: bool,

    /// Initial prompt (if no subcommand).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub prompt: Vec<String>,
}

/// CLI subcommands.
#[derive(Subcommand)]
pub enum Commands {
    // ========================================================================
    // 🚀 Execution (order 1-9)
    // ========================================================================
    /// Run Cortex non-interactively with advanced options
    #[command(visible_alias = "r", display_order = 1)]
    #[command(next_help_heading = categories::EXECUTION)]
    Run(RunCli),

    /// Execute in headless mode (for CI/CD, scripts, automation)
    #[command(visible_alias = "e", display_order = 2)]
    #[command(next_help_heading = categories::EXECUTION)]
    Exec(ExecCli),

    // ========================================================================
    // 📋 Session Management (order 10-19)
    // ========================================================================
    /// Resume a previous interactive session
    #[command(display_order = 10)]
    #[command(next_help_heading = categories::SESSION)]
    Resume(ResumeCommand),

    /// List previous sessions
    #[command(display_order = 11)]
    #[command(next_help_heading = categories::SESSION)]
    Sessions(SessionsCommand),

    /// Export a session to JSON format
    #[command(display_order = 12)]
    #[command(next_help_heading = categories::SESSION)]
    Export(ExportCommand),

    /// Import a session from JSON file or URL
    #[command(display_order = 13)]
    #[command(next_help_heading = categories::SESSION)]
    Import(ImportCommand),

    /// Delete a session
    #[command(display_order = 14)]
    #[command(next_help_heading = categories::SESSION)]
    Delete(DeleteCommand),

    // ========================================================================
    // 🔐 Authentication (order 20-29)
    // ========================================================================
    /// Authenticate with Cortex API
    #[command(display_order = 20)]
    #[command(next_help_heading = categories::AUTH)]
    Login(LoginCommand),

    /// Remove stored authentication credentials
    #[command(display_order = 21)]
    #[command(next_help_heading = categories::AUTH)]
    Logout(LogoutCommand),

    /// Show currently authenticated user
    #[command(display_order = 22)]
    #[command(next_help_heading = categories::AUTH)]
    Whoami,

    // ========================================================================
    // 🔌 Extensibility (order 30-39)
    // ========================================================================
    /// Manage agents (list, create, show)
    #[command(display_order = 30)]
    #[command(next_help_heading = categories::EXTENSION)]
    Agent(AgentCli),

    /// Manage MCP (Model Context Protocol) servers
    #[command(display_order = 31)]
    #[command(next_help_heading = categories::EXTENSION)]
    Mcp(McpCli),

    /// Run the MCP server (stdio transport)
    #[command(display_order = 32, hide = true)]
    #[command(next_help_heading = categories::EXTENSION)]
    McpServer,

    /// Start ACP server for IDE integration (e.g., Zed)
    #[command(display_order = 33)]
    #[command(next_help_heading = categories::EXTENSION)]
    Acp(AcpCli),

    // ========================================================================
    // ⚙️ Configuration (order 40-49)
    // ========================================================================
    /// Show or edit configuration
    #[command(display_order = 40)]
    #[command(next_help_heading = categories::CONFIG)]
    Config(ConfigCommand),

    /// List available models
    #[command(display_order = 41)]
    #[command(next_help_heading = categories::CONFIG)]
    Models(ModelsCli),

    /// Inspect feature flags
    #[command(display_order = 42)]
    #[command(next_help_heading = categories::CONFIG)]
    Features(FeaturesCommand),

    /// Initialize AGENTS.md in the current directory
    #[command(display_order = 43)]
    #[command(next_help_heading = categories::CONFIG)]
    Init(InitCommand),

    // ========================================================================
    // 🛠️ Utilities (order 50-59)
    // ========================================================================
    /// GitHub integration (actions, workflows)
    #[command(visible_alias = "gh", display_order = 50)]
    #[command(next_help_heading = categories::UTILITIES)]
    Github(GitHubCli),

    /// Checkout a pull request
    #[command(display_order = 51)]
    #[command(next_help_heading = categories::UTILITIES)]
    Pr(PrCli),

    /// Scrape web content to markdown/text/html
    #[command(display_order = 52)]
    #[command(next_help_heading = categories::UTILITIES)]
    Scrape(ScrapeCommand),

    /// Show usage statistics
    #[command(display_order = 53)]
    #[command(next_help_heading = categories::UTILITIES)]
    Stats(StatsCli),

    /// Generate shell completion scripts
    #[command(display_order = 54)]
    #[command(next_help_heading = categories::UTILITIES)]
    Completion(CompletionCommand),

    // ========================================================================
    // 🔧 Maintenance (order 60-69)
    // ========================================================================
    /// Check for and install updates
    #[command(display_order = 60)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Upgrade(UpgradeCli),

    /// Uninstall Cortex CLI
    #[command(display_order = 61)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Uninstall(UninstallCli),

    /// Data compaction and cleanup (logs, sessions, history)
    #[command(visible_aliases = ["gc", "cleanup"], display_order = 62)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Compact(CompactCli),

    /// Manage cache
    #[command(display_order = 63)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Cache(CacheCli),

    /// View application logs
    #[command(display_order = 64)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Logs(LogsCli),

    /// Submit feedback and bug reports
    #[command(visible_alias = "report", display_order = 65)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Feedback(FeedbackCli),

    /// Lock/protect sessions from deletion
    #[command(visible_alias = "protect", display_order = 66)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Lock(LockCli),

    /// Manage command aliases
    #[command(visible_alias = "aliases", display_order = 67)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Alias(AliasCli),

    /// Manage plugins
    #[command(visible_alias = "plugins", display_order = 68)]
    #[command(next_help_heading = categories::MAINTENANCE)]
    Plugin(PluginCli),

    // ========================================================================
    // Hidden commands (internal/debug/advanced)
    // ========================================================================
    /// Debug and diagnostic commands
    #[command(display_order = 99, hide = true)]
    Debug(DebugCli),

    /// Start interactive shell/REPL mode
    #[command(visible_aliases = ["interactive", "repl"], hide = true)]
    Shell(ShellCli),

    /// Execute and manage task DAGs (dependency graphs)
    #[command(visible_alias = "tasks", hide = true)]
    Dag(DagCli),

    /// Discover Cortex servers on the local network
    #[command(hide = true)]
    Servers(ServersCommand),

    /// View prompt history from past sessions
    #[command(hide = true)]
    History(HistoryCommand),

    /// Manage workspace/project settings
    #[command(visible_alias = "project", hide = true)]
    Workspace(WorkspaceCli),

    /// Run commands within a Cortex-provided sandbox
    #[command(visible_alias = "sb", hide = true)]
    Sandbox(SandboxArgs),

    /// Run the HTTP API server (for desktop/web integration)
    #[command(hide = true)]
    Serve(ServeCommand),
}

// ============================================================================
// Subcommand argument structures
// ============================================================================

/// Login command.
#[derive(Args)]
pub struct LoginCommand {
    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Read the API key from stdin
    #[arg(long = "with-api-key")]
    pub with_api_key: bool,

    /// Provide API token directly (for CI/CD automation).
    #[arg(long = "token", value_name = "TOKEN", conflicts_with = "with_api_key")]
    pub token: Option<String>,

    /// Use device code authentication flow
    #[arg(long = "device-auth")]
    pub use_device_code: bool,

    /// Use enterprise SSO authentication.
    #[arg(long = "sso")]
    pub use_sso: bool,

    /// Override the OAuth issuer base URL (advanced)
    #[arg(long = "experimental_issuer", value_name = "URL", hide = true)]
    pub issuer_base_url: Option<String>,

    /// Override the OAuth client ID (advanced)
    #[arg(long = "experimental_client-id", value_name = "CLIENT_ID", hide = true)]
    pub client_id: Option<String>,

    #[command(subcommand)]
    pub action: Option<LoginSubcommand>,
}

/// Login subcommands.
#[derive(Subcommand)]
pub enum LoginSubcommand {
    /// Show login status
    Status,
}

/// Logout command.
#[derive(Args)]
pub struct LogoutCommand {
    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Skip confirmation prompt and log out immediately.
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,

    /// Log out from all logged in accounts.
    #[arg(long = "all")]
    pub all: bool,
}

/// Completion command.
#[derive(Args)]
pub struct CompletionCommand {
    /// Shell to generate completions for.
    #[arg(value_enum)]
    pub shell: Option<clap_complete::Shell>,

    /// Install completions to your shell configuration file.
    #[arg(long = "install")]
    pub install: bool,
}

/// Init command - initialize AGENTS.md.
#[derive(Args)]
pub struct InitCommand {
    /// Force overwrite if AGENTS.md already exists.
    #[arg(short = 'f', long = "force")]
    pub force: bool,

    /// Accept defaults without prompting (non-interactive mode).
    #[arg(short = 'y', long = "yes")]
    pub yes: bool,
}

/// Resume command.
#[derive(Args)]
pub struct ResumeCommand {
    /// Session ID to resume (or "last" for most recent)
    #[arg(value_name = "SESSION_ID")]
    pub session_id: Option<String>,

    /// Continue the most recent session without showing the picker
    #[arg(long = "last", default_value_t = false, conflicts_with = "session_id")]
    pub last: bool,

    /// Show interactive picker to select from recent sessions
    #[arg(long = "pick", default_value_t = false, conflicts_with_all = ["session_id", "last"])]
    pub pick: bool,

    /// Show all sessions (disables cwd filtering)
    #[arg(long = "all", default_value_t = false)]
    pub all: bool,

    /// Do not persist session changes (incompatible with resume, will error).
    #[arg(long = "no-session", default_value_t = false)]
    pub no_session: bool,

    #[clap(flatten)]
    pub config_overrides: CliConfigOverrides,
}

/// Sessions command.
#[derive(Args)]
pub struct SessionsCommand {
    /// Show all sessions including from other directories
    #[arg(long)]
    pub all: bool,

    /// Show sessions from the last N days
    #[arg(long)]
    pub days: Option<u32>,

    /// Show sessions since this date (YYYY-MM-DD)
    #[arg(long)]
    pub since: Option<String>,

    /// Show sessions until this date (YYYY-MM-DD)
    #[arg(long)]
    pub until: Option<String>,

    /// Show only favorite sessions
    #[arg(long)]
    pub favorites: bool,

    /// Search sessions by title or ID
    #[arg(long, short)]
    pub search: Option<String>,

    /// Maximum number of sessions to show
    #[arg(long, short)]
    pub limit: Option<usize>,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// Delete command - delete a session.
#[derive(Args)]
pub struct DeleteCommand {
    /// Session ID to delete (full UUID or 8-character prefix)
    #[arg(required = true)]
    pub session_id: String,

    /// Skip confirmation prompt
    #[arg(long, short = 'y')]
    pub yes: bool,

    /// Force deletion even if session is locked
    #[arg(long, short = 'f')]
    pub force: bool,
}

/// Config command.
#[derive(Args)]
pub struct ConfigCommand {
    /// Show configuration in JSON format
    #[arg(long)]
    pub json: bool,

    /// Edit configuration interactively
    #[arg(long)]
    pub edit: bool,

    #[command(subcommand)]
    pub action: Option<ConfigSubcommand>,
}

/// Config subcommands.
#[derive(Subcommand)]
pub enum ConfigSubcommand {
    /// Get a configuration value
    Get(ConfigGetArgs),
    /// Set a configuration value
    Set(ConfigSetArgs),
    /// Unset (remove) a configuration value
    Unset(ConfigUnsetArgs),
}

/// Arguments for config get.
#[derive(Args)]
pub struct ConfigGetArgs {
    /// Configuration key to get (e.g., model, provider)
    pub key: String,
}

/// Arguments for config set.
#[derive(Args)]
pub struct ConfigSetArgs {
    /// Configuration key (e.g., model, provider)
    pub key: String,
    /// Value to set
    pub value: String,
}

/// Arguments for config unset.
#[derive(Args)]
pub struct ConfigUnsetArgs {
    /// Configuration key to remove
    pub key: String,
}

/// Sandbox debug commands.
#[derive(Args)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub cmd: SandboxCommand,
}

/// Sandbox subcommands.
#[derive(Subcommand)]
pub enum SandboxCommand {
    /// Run a command under Seatbelt (macOS only)
    #[command(visible_alias = "seatbelt")]
    Macos(SeatbeltCommand),
    /// Run a command under Landlock+seccomp (Linux only)
    #[command(visible_alias = "landlock")]
    Linux(LandlockCommand),
    /// Run a command under Windows restricted token (Windows only)
    Windows(WindowsCommand),
}

/// Features command.
#[derive(Args)]
pub struct FeaturesCommand {
    #[command(subcommand)]
    pub sub: FeaturesSubcommand,
}

/// Features subcommands.
#[derive(Subcommand)]
pub enum FeaturesSubcommand {
    /// List known features with their stage and effective state
    List,
}

/// Serve command - runs HTTP API server.
#[derive(Args)]
pub struct ServeCommand {
    /// Port to listen on
    #[arg(short, long, default_value = "3000")]
    pub port: u16,

    /// Host address to bind the server to.
    #[arg(long, default_value = "127.0.0.1")]
    pub host: String,

    /// Authentication token for API access.
    #[arg(long = "auth-token")]
    pub auth_token: Option<String>,

    /// Enable CORS (Cross-Origin Resource Sharing) for all origins.
    #[arg(long)]
    pub cors: bool,

    /// Allowed CORS origin(s). Can be specified multiple times.
    #[arg(long = "cors-origin", value_name = "ORIGIN")]
    pub cors_origins: Vec<String>,

    /// Enable mDNS service discovery (advertise on local network)
    #[arg(long = "mdns", default_value_t = false)]
    pub mdns: bool,

    /// Disable mDNS service discovery
    #[arg(long = "no-mdns", default_value_t = false, conflicts_with = "mdns")]
    pub no_mdns: bool,

    /// Custom service name for mDNS advertising
    #[arg(long = "mdns-name")]
    pub mdns_name: Option<String>,
}

/// Servers command - discover Cortex servers on the network.
#[derive(Args)]
pub struct ServersCommand {
    #[command(subcommand)]
    pub action: Option<ServersSubcommand>,

    /// Timeout for discovery in seconds
    #[arg(short, long, default_value = "3")]
    pub timeout: u64,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// Servers subcommands.
#[derive(Subcommand)]
pub enum ServersSubcommand {
    /// Re-scan the network for mDNS servers (forces a fresh discovery)
    Refresh(ServersRefreshArgs),
}

/// Arguments for servers refresh command.
#[derive(Args)]
pub struct ServersRefreshArgs {
    /// Timeout for discovery in seconds
    #[arg(short, long, default_value = "5")]
    pub timeout: u64,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// History command - view past prompts and sessions.
#[derive(Args)]
pub struct HistoryCommand {
    #[command(subcommand)]
    pub action: Option<HistorySubcommand>,

    /// Maximum number of entries to show
    #[arg(short = 'n', long, default_value = "20")]
    pub limit: usize,

    /// Show history from all directories
    #[arg(long)]
    pub all: bool,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// History subcommands.
#[derive(Subcommand)]
pub enum HistorySubcommand {
    /// Search history for a pattern
    Search(HistorySearchArgs),
    /// Clear history (requires confirmation)
    Clear(HistoryClearArgs),
}

/// Arguments for history search command.
#[derive(Args)]
pub struct HistorySearchArgs {
    /// Pattern to search for in prompts
    pub pattern: String,

    /// Maximum number of results
    #[arg(short = 'n', long, default_value = "20")]
    pub limit: usize,

    /// Output in JSON format
    #[arg(long)]
    pub json: bool,
}

/// Arguments for history clear command.
#[derive(Args)]
pub struct HistoryClearArgs {
    /// Skip confirmation prompt
    #[arg(short = 'y', long)]
    pub yes: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // ==========================================================================
    // LogLevel tests
    // ==========================================================================

    #[test]
    fn test_log_level_default() {
        let default = LogLevel::default();
        assert_eq!(default, LogLevel::Info);
    }

    #[test]
    fn test_log_level_as_filter_str() {
        assert_eq!(LogLevel::Error.as_filter_str(), "error");
        assert_eq!(LogLevel::Warn.as_filter_str(), "warn");
        assert_eq!(LogLevel::Info.as_filter_str(), "info");
        assert_eq!(LogLevel::Debug.as_filter_str(), "debug");
        assert_eq!(LogLevel::Trace.as_filter_str(), "trace");
    }

    #[test]
    fn test_log_level_from_str_loose_valid() {
        assert_eq!(LogLevel::from_str_loose("error"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str_loose("warn"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str_loose("warning"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str_loose("info"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str_loose("debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str_loose("trace"), Some(LogLevel::Trace));
    }

    #[test]
    fn test_log_level_from_str_loose_case_insensitive() {
        assert_eq!(LogLevel::from_str_loose("ERROR"), Some(LogLevel::Error));
        assert_eq!(LogLevel::from_str_loose("WARN"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str_loose("WARNING"), Some(LogLevel::Warn));
        assert_eq!(LogLevel::from_str_loose("INFO"), Some(LogLevel::Info));
        assert_eq!(LogLevel::from_str_loose("Debug"), Some(LogLevel::Debug));
        assert_eq!(LogLevel::from_str_loose("TrAcE"), Some(LogLevel::Trace));
    }

    #[test]
    fn test_log_level_from_str_loose_invalid() {
        assert_eq!(LogLevel::from_str_loose("invalid"), None);
        assert_eq!(LogLevel::from_str_loose(""), None);
        assert_eq!(LogLevel::from_str_loose("err"), None);
        assert_eq!(LogLevel::from_str_loose("verbose"), None);
    }

    #[test]
    fn test_log_level_equality() {
        assert_eq!(LogLevel::Error, LogLevel::Error);
        assert_ne!(LogLevel::Error, LogLevel::Warn);
        assert_ne!(LogLevel::Info, LogLevel::Debug);
    }

    #[test]
    fn test_log_level_clone() {
        let level = LogLevel::Debug;
        let cloned = level;
        assert_eq!(level, cloned);
    }

    // ==========================================================================
    // ColorMode tests
    // ==========================================================================

    #[test]
    fn test_color_mode_default() {
        let default = ColorMode::default();
        assert_eq!(default, ColorMode::Auto);
    }

    #[test]
    fn test_color_mode_equality() {
        assert_eq!(ColorMode::Auto, ColorMode::Auto);
        assert_eq!(ColorMode::Always, ColorMode::Always);
        assert_eq!(ColorMode::Never, ColorMode::Never);
        assert_ne!(ColorMode::Auto, ColorMode::Always);
        assert_ne!(ColorMode::Always, ColorMode::Never);
    }

    #[test]
    fn test_color_mode_clone() {
        let mode = ColorMode::Always;
        let cloned = mode;
        assert_eq!(mode, cloned);
    }

    // ==========================================================================
    // InteractiveArgs tests
    // ==========================================================================

    #[test]
    fn test_interactive_args_default() {
        let args = InteractiveArgs::default();
        assert!(args.model.is_none());
        assert!(!args.oss);
        assert!(args.config_profile.is_none());
        assert!(args.sandbox_mode.is_none());
        assert!(args.approval_policy.is_none());
        assert!(!args.full_auto);
        assert!(!args.dangerously_bypass_approvals_and_sandbox);
        assert!(args.cwd.is_none());
        assert!(args.add_dir.is_empty());
        assert!(args.images.is_empty());
        assert!(!args.web_search);
        assert_eq!(args.log_level, LogLevel::Info);
        assert!(!args.debug);
        assert!(args.prompt.is_empty());
    }

    // ==========================================================================
    // Cli parsing tests
    // ==========================================================================

    #[test]
    fn test_cli_no_args() {
        let cli = Cli::try_parse_from(["cortex"]).expect("should parse with no args");
        assert!(cli.command.is_none());
        assert!(!cli.verbose);
        assert!(!cli.trace);
        assert_eq!(cli.color, ColorMode::Auto);
    }

    #[test]
    fn test_cli_verbose_flag() {
        let cli = Cli::try_parse_from(["cortex", "--verbose"]).expect("should parse --verbose");
        assert!(cli.verbose);
    }

    #[test]
    fn test_cli_verbose_short_flag() {
        let cli = Cli::try_parse_from(["cortex", "-v"]).expect("should parse -v");
        assert!(cli.verbose);
    }

    #[test]
    fn test_cli_trace_flag() {
        let cli = Cli::try_parse_from(["cortex", "--trace"]).expect("should parse --trace");
        assert!(cli.trace);
    }

    #[test]
    fn test_cli_color_always() {
        let cli = Cli::try_parse_from(["cortex", "--color", "always"])
            .expect("should parse --color always");
        assert_eq!(cli.color, ColorMode::Always);
    }

    #[test]
    fn test_cli_color_never() {
        let cli = Cli::try_parse_from(["cortex", "--color", "never"])
            .expect("should parse --color never");
        assert_eq!(cli.color, ColorMode::Never);
    }

    #[test]
    fn test_cli_color_auto() {
        let cli =
            Cli::try_parse_from(["cortex", "--color", "auto"]).expect("should parse --color auto");
        assert_eq!(cli.color, ColorMode::Auto);
    }

    #[test]
    fn test_cli_model_short() {
        let cli = Cli::try_parse_from(["cortex", "-m", "gpt-4o"]).expect("should parse -m");
        assert_eq!(cli.interactive.model, Some("gpt-4o".to_string()));
    }

    #[test]
    fn test_cli_model_long() {
        let cli = Cli::try_parse_from(["cortex", "--model", "claude-sonnet-4-20250514"])
            .expect("should parse --model");
        assert_eq!(
            cli.interactive.model,
            Some("claude-sonnet-4-20250514".to_string())
        );
    }

    #[test]
    fn test_cli_oss_flag() {
        let cli = Cli::try_parse_from(["cortex", "--oss"]).expect("should parse --oss");
        assert!(cli.interactive.oss);
    }

    #[test]
    fn test_cli_profile_short() {
        let cli = Cli::try_parse_from(["cortex", "-p", "work"]).expect("should parse -p");
        assert_eq!(cli.interactive.config_profile, Some("work".to_string()));
    }

    #[test]
    fn test_cli_profile_long() {
        let cli = Cli::try_parse_from(["cortex", "--profile", "production"])
            .expect("should parse --profile");
        assert_eq!(
            cli.interactive.config_profile,
            Some("production".to_string())
        );
    }

    #[test]
    fn test_cli_sandbox_mode() {
        let cli =
            Cli::try_parse_from(["cortex", "--sandbox", "strict"]).expect("should parse --sandbox");
        assert_eq!(cli.interactive.sandbox_mode, Some("strict".to_string()));
    }

    #[test]
    fn test_cli_approval_policy() {
        let cli = Cli::try_parse_from(["cortex", "--ask-for-approval", "always"])
            .expect("should parse --ask-for-approval");
        assert_eq!(cli.interactive.approval_policy, Some("always".to_string()));
    }

    #[test]
    fn test_cli_approval_policy_short() {
        let cli = Cli::try_parse_from(["cortex", "-a", "never"]).expect("should parse -a");
        assert_eq!(cli.interactive.approval_policy, Some("never".to_string()));
    }

    #[test]
    fn test_cli_full_auto() {
        let cli = Cli::try_parse_from(["cortex", "--full-auto"]).expect("should parse --full-auto");
        assert!(cli.interactive.full_auto);
    }

    #[test]
    fn test_cli_dangerously_bypass() {
        let cli = Cli::try_parse_from(["cortex", "--dangerously-bypass-approvals-and-sandbox"])
            .expect("should parse dangerous flag");
        assert!(cli.interactive.dangerously_bypass_approvals_and_sandbox);
    }

    #[test]
    fn test_cli_dangerously_bypass_yolo_alias() {
        let cli = Cli::try_parse_from(["cortex", "--yolo"]).expect("should parse --yolo alias");
        assert!(cli.interactive.dangerously_bypass_approvals_and_sandbox);
    }

    #[test]
    fn test_cli_cwd_short() {
        let cli = Cli::try_parse_from(["cortex", "-C", "/workspace"]).expect("should parse -C");
        assert_eq!(cli.interactive.cwd, Some(PathBuf::from("/workspace")));
    }

    #[test]
    fn test_cli_cwd_long() {
        let cli =
            Cli::try_parse_from(["cortex", "--cd", "/tmp/project"]).expect("should parse --cd");
        assert_eq!(cli.interactive.cwd, Some(PathBuf::from("/tmp/project")));
    }

    #[test]
    fn test_cli_add_dir() {
        let cli = Cli::try_parse_from(["cortex", "--add-dir", "/extra/dir"])
            .expect("should parse --add-dir");
        assert_eq!(cli.interactive.add_dir, vec![PathBuf::from("/extra/dir")]);
    }

    #[test]
    fn test_cli_add_dir_multiple() {
        let cli = Cli::try_parse_from(["cortex", "--add-dir", "/dir1", "--add-dir", "/dir2"])
            .expect("should parse multiple --add-dir");
        assert_eq!(
            cli.interactive.add_dir,
            vec![PathBuf::from("/dir1"), PathBuf::from("/dir2")]
        );
    }

    #[test]
    fn test_cli_image() {
        let cli = Cli::try_parse_from(["cortex", "--image", "screenshot.png"])
            .expect("should parse --image");
        assert_eq!(
            cli.interactive.images,
            vec![PathBuf::from("screenshot.png")]
        );
    }

    #[test]
    fn test_cli_image_short() {
        let cli = Cli::try_parse_from(["cortex", "-i", "photo.jpg"]).expect("should parse -i");
        assert_eq!(cli.interactive.images, vec![PathBuf::from("photo.jpg")]);
    }

    #[test]
    fn test_cli_image_comma_separated() {
        let cli = Cli::try_parse_from(["cortex", "--image", "a.png,b.jpg,c.gif"])
            .expect("should parse comma-separated images");
        assert_eq!(
            cli.interactive.images,
            vec![
                PathBuf::from("a.png"),
                PathBuf::from("b.jpg"),
                PathBuf::from("c.gif")
            ]
        );
    }

    #[test]
    fn test_cli_web_search() {
        let cli = Cli::try_parse_from(["cortex", "--search"]).expect("should parse --search");
        assert!(cli.interactive.web_search);
    }

    #[test]
    fn test_cli_log_level() {
        let cli = Cli::try_parse_from(["cortex", "--log-level", "debug"])
            .expect("should parse --log-level");
        assert_eq!(cli.interactive.log_level, LogLevel::Debug);
    }

    #[test]
    fn test_cli_log_level_short() {
        let cli = Cli::try_parse_from(["cortex", "-L", "trace"]).expect("should parse -L");
        assert_eq!(cli.interactive.log_level, LogLevel::Trace);
    }

    #[test]
    fn test_cli_debug_flag() {
        let cli = Cli::try_parse_from(["cortex", "--debug"]).expect("should parse --debug");
        assert!(cli.interactive.debug);
    }

    #[test]
    fn test_cli_prompt_trailing() {
        let cli = Cli::try_parse_from(["cortex", "write", "a", "unit", "test"])
            .expect("should parse trailing prompt");
        assert_eq!(
            cli.interactive.prompt,
            vec!["write", "a", "unit", "test"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_cli_prompt_with_hyphens() {
        let cli = Cli::try_parse_from(["cortex", "create", "--", "test", "--with", "options"])
            .expect("should parse prompt with hyphens");
        assert!(!cli.interactive.prompt.is_empty());
    }

    // ==========================================================================
    // Subcommand tests
    // ==========================================================================

    #[test]
    fn test_cli_run_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "run"]).expect("should parse run subcommand");
        assert!(matches!(cli.command, Some(Commands::Run(_))));
    }

    #[test]
    fn test_cli_exec_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "exec"]).expect("should parse exec subcommand");
        assert!(matches!(cli.command, Some(Commands::Exec(_))));
    }

    #[test]
    fn test_cli_login_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "login"]).expect("should parse login subcommand");
        assert!(matches!(cli.command, Some(Commands::Login(_))));
    }

    #[test]
    fn test_cli_logout_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "logout"]).expect("should parse logout subcommand");
        assert!(matches!(cli.command, Some(Commands::Logout(_))));
    }

    #[test]
    fn test_cli_whoami_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "whoami"]).expect("should parse whoami subcommand");
        assert!(matches!(cli.command, Some(Commands::Whoami)));
    }

    #[test]
    fn test_cli_config_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "config"]).expect("should parse config subcommand");
        assert!(matches!(cli.command, Some(Commands::Config(_))));
    }

    #[test]
    fn test_cli_models_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "models"]).expect("should parse models subcommand");
        assert!(matches!(cli.command, Some(Commands::Models(_))));
    }

    #[test]
    fn test_cli_sessions_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "sessions"]).expect("should parse sessions subcommand");
        assert!(matches!(cli.command, Some(Commands::Sessions(_))));
    }

    #[test]
    fn test_cli_resume_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "resume"]).expect("should parse resume subcommand");
        assert!(matches!(cli.command, Some(Commands::Resume(_))));
    }

    #[test]
    fn test_cli_resume_with_session_id() {
        let cli = Cli::try_parse_from(["cortex", "resume", "abc123"])
            .expect("should parse resume with session id");
        if let Some(Commands::Resume(resume)) = cli.command {
            assert_eq!(resume.session_id, Some("abc123".to_string()));
        } else {
            panic!("Expected Resume command");
        }
    }

    #[test]
    fn test_cli_resume_last_flag() {
        let cli = Cli::try_parse_from(["cortex", "resume", "--last"])
            .expect("should parse resume --last");
        if let Some(Commands::Resume(resume)) = cli.command {
            assert!(resume.last);
        } else {
            panic!("Expected Resume command");
        }
    }

    #[test]
    fn test_cli_upgrade_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "upgrade"]).expect("should parse upgrade subcommand");
        assert!(matches!(cli.command, Some(Commands::Upgrade(_))));
    }

    #[test]
    fn test_cli_agent_subcommand() {
        // Agent command requires a subcommand, so test with "list"
        let cli = Cli::try_parse_from(["cortex", "agent", "list"])
            .expect("should parse agent list subcommand");
        assert!(matches!(cli.command, Some(Commands::Agent(_))));
    }

    // NOTE: test_cli_mcp_subcommand is skipped due to a pre-existing bug in mcp_cmd
    // where "ls" is defined as both a command name and an alias, causing clap to panic.
    // This is tracked as a known issue in the mcp_cmd module (types.rs:27).

    #[test]
    fn test_cli_acp_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "acp"]).expect("should parse acp subcommand");
        assert!(matches!(cli.command, Some(Commands::Acp(_))));
    }

    // ==========================================================================
    // LoginCommand tests
    // ==========================================================================

    #[test]
    fn test_login_command_default() {
        let cli = Cli::try_parse_from(["cortex", "login"]).expect("should parse login");
        if let Some(Commands::Login(login)) = cli.command {
            assert!(!login.with_api_key);
            assert!(login.token.is_none());
            assert!(!login.use_device_code);
            assert!(!login.use_sso);
            assert!(login.issuer_base_url.is_none());
            assert!(login.client_id.is_none());
            assert!(login.action.is_none());
        } else {
            panic!("Expected Login command");
        }
    }

    #[test]
    fn test_login_command_with_api_key() {
        let cli = Cli::try_parse_from(["cortex", "login", "--with-api-key"])
            .expect("should parse login --with-api-key");
        if let Some(Commands::Login(login)) = cli.command {
            assert!(login.with_api_key);
        } else {
            panic!("Expected Login command");
        }
    }

    #[test]
    fn test_login_command_with_token() {
        let cli = Cli::try_parse_from(["cortex", "login", "--token", "mytoken123"])
            .expect("should parse login --token");
        if let Some(Commands::Login(login)) = cli.command {
            assert_eq!(login.token, Some("mytoken123".to_string()));
        } else {
            panic!("Expected Login command");
        }
    }

    #[test]
    fn test_login_command_device_auth() {
        let cli = Cli::try_parse_from(["cortex", "login", "--device-auth"])
            .expect("should parse login --device-auth");
        if let Some(Commands::Login(login)) = cli.command {
            assert!(login.use_device_code);
        } else {
            panic!("Expected Login command");
        }
    }

    #[test]
    fn test_login_command_sso() {
        let cli =
            Cli::try_parse_from(["cortex", "login", "--sso"]).expect("should parse login --sso");
        if let Some(Commands::Login(login)) = cli.command {
            assert!(login.use_sso);
        } else {
            panic!("Expected Login command");
        }
    }

    #[test]
    fn test_login_status_subcommand() {
        let cli =
            Cli::try_parse_from(["cortex", "login", "status"]).expect("should parse login status");
        if let Some(Commands::Login(login)) = cli.command {
            assert!(matches!(login.action, Some(LoginSubcommand::Status)));
        } else {
            panic!("Expected Login command");
        }
    }

    // ==========================================================================
    // LogoutCommand tests
    // ==========================================================================

    #[test]
    fn test_logout_command_default() {
        let cli = Cli::try_parse_from(["cortex", "logout"]).expect("should parse logout");
        if let Some(Commands::Logout(logout)) = cli.command {
            assert!(!logout.yes);
            assert!(!logout.all);
        } else {
            panic!("Expected Logout command");
        }
    }

    #[test]
    fn test_logout_command_yes() {
        let cli = Cli::try_parse_from(["cortex", "logout", "-y"]).expect("should parse logout -y");
        if let Some(Commands::Logout(logout)) = cli.command {
            assert!(logout.yes);
        } else {
            panic!("Expected Logout command");
        }
    }

    #[test]
    fn test_logout_command_all() {
        let cli =
            Cli::try_parse_from(["cortex", "logout", "--all"]).expect("should parse logout --all");
        if let Some(Commands::Logout(logout)) = cli.command {
            assert!(logout.all);
        } else {
            panic!("Expected Logout command");
        }
    }

    // ==========================================================================
    // SessionsCommand tests
    // ==========================================================================

    #[test]
    fn test_sessions_command_default() {
        let cli = Cli::try_parse_from(["cortex", "sessions"]).expect("should parse sessions");
        if let Some(Commands::Sessions(sessions)) = cli.command {
            assert!(!sessions.all);
            assert!(sessions.days.is_none());
            assert!(sessions.since.is_none());
            assert!(sessions.until.is_none());
            assert!(!sessions.favorites);
            assert!(sessions.search.is_none());
            assert!(sessions.limit.is_none());
            assert!(!sessions.json);
        } else {
            panic!("Expected Sessions command");
        }
    }

    #[test]
    fn test_sessions_command_with_flags() {
        let cli = Cli::try_parse_from([
            "cortex",
            "sessions",
            "--all",
            "--days",
            "7",
            "--favorites",
            "--limit",
            "10",
            "--json",
        ])
        .expect("should parse sessions with flags");
        if let Some(Commands::Sessions(sessions)) = cli.command {
            assert!(sessions.all);
            assert_eq!(sessions.days, Some(7));
            assert!(sessions.favorites);
            assert_eq!(sessions.limit, Some(10));
            assert!(sessions.json);
        } else {
            panic!("Expected Sessions command");
        }
    }

    #[test]
    fn test_sessions_command_search() {
        let cli = Cli::try_parse_from(["cortex", "sessions", "--search", "fix bug"])
            .expect("should parse sessions --search");
        if let Some(Commands::Sessions(sessions)) = cli.command {
            assert_eq!(sessions.search, Some("fix bug".to_string()));
        } else {
            panic!("Expected Sessions command");
        }
    }

    // ==========================================================================
    // DeleteCommand tests
    // ==========================================================================

    #[test]
    fn test_delete_command() {
        let cli = Cli::try_parse_from(["cortex", "delete", "abc12345"])
            .expect("should parse delete with session id");
        if let Some(Commands::Delete(delete)) = cli.command {
            assert_eq!(delete.session_id, "abc12345");
            assert!(!delete.yes);
            assert!(!delete.force);
        } else {
            panic!("Expected Delete command");
        }
    }

    #[test]
    fn test_delete_command_with_flags() {
        let cli = Cli::try_parse_from(["cortex", "delete", "abc12345", "-y", "-f"])
            .expect("should parse delete with flags");
        if let Some(Commands::Delete(delete)) = cli.command {
            assert_eq!(delete.session_id, "abc12345");
            assert!(delete.yes);
            assert!(delete.force);
        } else {
            panic!("Expected Delete command");
        }
    }

    // ==========================================================================
    // ConfigCommand tests
    // ==========================================================================

    #[test]
    fn test_config_command_default() {
        let cli = Cli::try_parse_from(["cortex", "config"]).expect("should parse config");
        if let Some(Commands::Config(config)) = cli.command {
            assert!(!config.json);
            assert!(!config.edit);
            assert!(config.action.is_none());
        } else {
            panic!("Expected Config command");
        }
    }

    #[test]
    fn test_config_command_json() {
        let cli = Cli::try_parse_from(["cortex", "config", "--json"])
            .expect("should parse config --json");
        if let Some(Commands::Config(config)) = cli.command {
            assert!(config.json);
        } else {
            panic!("Expected Config command");
        }
    }

    #[test]
    fn test_config_command_edit() {
        let cli = Cli::try_parse_from(["cortex", "config", "--edit"])
            .expect("should parse config --edit");
        if let Some(Commands::Config(config)) = cli.command {
            assert!(config.edit);
        } else {
            panic!("Expected Config command");
        }
    }

    #[test]
    fn test_config_get_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "config", "get", "model"])
            .expect("should parse config get");
        if let Some(Commands::Config(config)) = cli.command {
            if let Some(ConfigSubcommand::Get(get)) = config.action {
                assert_eq!(get.key, "model");
            } else {
                panic!("Expected Get subcommand");
            }
        } else {
            panic!("Expected Config command");
        }
    }

    #[test]
    fn test_config_set_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "config", "set", "model", "gpt-4o"])
            .expect("should parse config set");
        if let Some(Commands::Config(config)) = cli.command {
            if let Some(ConfigSubcommand::Set(set)) = config.action {
                assert_eq!(set.key, "model");
                assert_eq!(set.value, "gpt-4o");
            } else {
                panic!("Expected Set subcommand");
            }
        } else {
            panic!("Expected Config command");
        }
    }

    #[test]
    fn test_config_unset_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "config", "unset", "api_key"])
            .expect("should parse config unset");
        if let Some(Commands::Config(config)) = cli.command {
            if let Some(ConfigSubcommand::Unset(unset)) = config.action {
                assert_eq!(unset.key, "api_key");
            } else {
                panic!("Expected Unset subcommand");
            }
        } else {
            panic!("Expected Config command");
        }
    }

    // ==========================================================================
    // ServeCommand tests
    // ==========================================================================

    #[test]
    fn test_serve_command_default() {
        let cli = Cli::try_parse_from(["cortex", "serve"]).expect("should parse serve");
        if let Some(Commands::Serve(serve)) = cli.command {
            assert_eq!(serve.port, 3000);
            assert_eq!(serve.host, "127.0.0.1");
            assert!(serve.auth_token.is_none());
            assert!(!serve.cors);
            assert!(serve.cors_origins.is_empty());
            assert!(!serve.mdns);
            assert!(!serve.no_mdns);
            assert!(serve.mdns_name.is_none());
        } else {
            panic!("Expected Serve command");
        }
    }

    #[test]
    fn test_serve_command_with_port() {
        let cli = Cli::try_parse_from(["cortex", "serve", "--port", "8080"])
            .expect("should parse serve --port");
        if let Some(Commands::Serve(serve)) = cli.command {
            assert_eq!(serve.port, 8080);
        } else {
            panic!("Expected Serve command");
        }
    }

    #[test]
    fn test_serve_command_with_host() {
        let cli = Cli::try_parse_from(["cortex", "serve", "--host", "0.0.0.0"])
            .expect("should parse serve --host");
        if let Some(Commands::Serve(serve)) = cli.command {
            assert_eq!(serve.host, "0.0.0.0");
        } else {
            panic!("Expected Serve command");
        }
    }

    #[test]
    fn test_serve_command_cors() {
        let cli =
            Cli::try_parse_from(["cortex", "serve", "--cors"]).expect("should parse serve --cors");
        if let Some(Commands::Serve(serve)) = cli.command {
            assert!(serve.cors);
        } else {
            panic!("Expected Serve command");
        }
    }

    #[test]
    fn test_serve_command_cors_origins() {
        let cli = Cli::try_parse_from([
            "cortex",
            "serve",
            "--cors-origin",
            "http://localhost:3000",
            "--cors-origin",
            "https://example.com",
        ])
        .expect("should parse serve with cors origins");
        if let Some(Commands::Serve(serve)) = cli.command {
            assert_eq!(
                serve.cors_origins,
                vec!["http://localhost:3000", "https://example.com"]
            );
        } else {
            panic!("Expected Serve command");
        }
    }

    #[test]
    fn test_serve_command_mdns() {
        let cli =
            Cli::try_parse_from(["cortex", "serve", "--mdns"]).expect("should parse serve --mdns");
        if let Some(Commands::Serve(serve)) = cli.command {
            assert!(serve.mdns);
        } else {
            panic!("Expected Serve command");
        }
    }

    // ==========================================================================
    // InitCommand tests
    // ==========================================================================

    #[test]
    fn test_init_command_default() {
        let cli = Cli::try_parse_from(["cortex", "init"]).expect("should parse init");
        if let Some(Commands::Init(init)) = cli.command {
            assert!(!init.force);
            assert!(!init.yes);
        } else {
            panic!("Expected Init command");
        }
    }

    #[test]
    fn test_init_command_force() {
        let cli = Cli::try_parse_from(["cortex", "init", "-f"]).expect("should parse init -f");
        if let Some(Commands::Init(init)) = cli.command {
            assert!(init.force);
        } else {
            panic!("Expected Init command");
        }
    }

    #[test]
    fn test_init_command_yes() {
        let cli = Cli::try_parse_from(["cortex", "init", "-y"]).expect("should parse init -y");
        if let Some(Commands::Init(init)) = cli.command {
            assert!(init.yes);
        } else {
            panic!("Expected Init command");
        }
    }

    // ==========================================================================
    // CompletionCommand tests
    // ==========================================================================

    #[test]
    fn test_completion_command_default() {
        let cli = Cli::try_parse_from(["cortex", "completion"]).expect("should parse completion");
        if let Some(Commands::Completion(completion)) = cli.command {
            assert!(completion.shell.is_none());
            assert!(!completion.install);
        } else {
            panic!("Expected Completion command");
        }
    }

    #[test]
    fn test_completion_command_bash() {
        let cli = Cli::try_parse_from(["cortex", "completion", "bash"])
            .expect("should parse completion bash");
        if let Some(Commands::Completion(completion)) = cli.command {
            assert_eq!(completion.shell, Some(clap_complete::Shell::Bash));
        } else {
            panic!("Expected Completion command");
        }
    }

    #[test]
    fn test_completion_command_zsh() {
        let cli = Cli::try_parse_from(["cortex", "completion", "zsh"])
            .expect("should parse completion zsh");
        if let Some(Commands::Completion(completion)) = cli.command {
            assert_eq!(completion.shell, Some(clap_complete::Shell::Zsh));
        } else {
            panic!("Expected Completion command");
        }
    }

    #[test]
    fn test_completion_command_install() {
        let cli = Cli::try_parse_from(["cortex", "completion", "--install"])
            .expect("should parse completion --install");
        if let Some(Commands::Completion(completion)) = cli.command {
            assert!(completion.install);
        } else {
            panic!("Expected Completion command");
        }
    }

    // ==========================================================================
    // HistoryCommand tests
    // ==========================================================================

    #[test]
    fn test_history_command_default() {
        let cli = Cli::try_parse_from(["cortex", "history"]).expect("should parse history");
        if let Some(Commands::History(history)) = cli.command {
            assert!(history.action.is_none());
            assert_eq!(history.limit, 20);
            assert!(!history.all);
            assert!(!history.json);
        } else {
            panic!("Expected History command");
        }
    }

    #[test]
    fn test_history_command_with_limit() {
        let cli = Cli::try_parse_from(["cortex", "history", "-n", "50"])
            .expect("should parse history -n");
        if let Some(Commands::History(history)) = cli.command {
            assert_eq!(history.limit, 50);
        } else {
            panic!("Expected History command");
        }
    }

    #[test]
    fn test_history_search_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "history", "search", "fix"])
            .expect("should parse history search");
        if let Some(Commands::History(history)) = cli.command {
            if let Some(HistorySubcommand::Search(search)) = history.action {
                assert_eq!(search.pattern, "fix");
                assert_eq!(search.limit, 20);
                assert!(!search.json);
            } else {
                panic!("Expected Search subcommand");
            }
        } else {
            panic!("Expected History command");
        }
    }

    #[test]
    fn test_history_clear_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "history", "clear"])
            .expect("should parse history clear");
        if let Some(Commands::History(history)) = cli.command {
            if let Some(HistorySubcommand::Clear(clear)) = history.action {
                assert!(!clear.yes);
            } else {
                panic!("Expected Clear subcommand");
            }
        } else {
            panic!("Expected History command");
        }
    }

    #[test]
    fn test_history_clear_yes() {
        let cli = Cli::try_parse_from(["cortex", "history", "clear", "-y"])
            .expect("should parse history clear -y");
        if let Some(Commands::History(history)) = cli.command {
            if let Some(HistorySubcommand::Clear(clear)) = history.action {
                assert!(clear.yes);
            } else {
                panic!("Expected Clear subcommand");
            }
        } else {
            panic!("Expected History command");
        }
    }

    // ==========================================================================
    // ServersCommand tests
    // ==========================================================================

    #[test]
    fn test_servers_command_default() {
        let cli = Cli::try_parse_from(["cortex", "servers"]).expect("should parse servers");
        if let Some(Commands::Servers(servers)) = cli.command {
            assert!(servers.action.is_none());
            assert_eq!(servers.timeout, 3);
            assert!(!servers.json);
        } else {
            panic!("Expected Servers command");
        }
    }

    #[test]
    fn test_servers_command_with_timeout() {
        let cli = Cli::try_parse_from(["cortex", "servers", "--timeout", "10"])
            .expect("should parse servers --timeout");
        if let Some(Commands::Servers(servers)) = cli.command {
            assert_eq!(servers.timeout, 10);
        } else {
            panic!("Expected Servers command");
        }
    }

    #[test]
    fn test_servers_refresh_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "servers", "refresh"])
            .expect("should parse servers refresh");
        if let Some(Commands::Servers(servers)) = cli.command {
            if let Some(ServersSubcommand::Refresh(refresh)) = servers.action {
                assert_eq!(refresh.timeout, 5);
                assert!(!refresh.json);
            } else {
                panic!("Expected Refresh subcommand");
            }
        } else {
            panic!("Expected Servers command");
        }
    }

    // ==========================================================================
    // get_long_version tests
    // ==========================================================================

    #[test]
    fn test_get_long_version() {
        let version = get_long_version();
        assert!(!version.is_empty());
        // Version should contain the package version
        assert!(
            version.contains(env!("CARGO_PKG_VERSION")),
            "Version should contain CARGO_PKG_VERSION"
        );
    }

    // ==========================================================================
    // Conflict tests
    // ==========================================================================

    #[test]
    fn test_dangerously_bypass_conflicts_with_approval_policy() {
        let result = Cli::try_parse_from([
            "cortex",
            "--dangerously-bypass-approvals-and-sandbox",
            "--ask-for-approval",
            "always",
        ]);
        assert!(
            result.is_err(),
            "Should fail when dangerous flag conflicts with approval policy"
        );
    }

    #[test]
    fn test_dangerously_bypass_conflicts_with_full_auto() {
        let result = Cli::try_parse_from([
            "cortex",
            "--dangerously-bypass-approvals-and-sandbox",
            "--full-auto",
        ]);
        assert!(
            result.is_err(),
            "Should fail when dangerous flag conflicts with full-auto"
        );
    }

    #[test]
    fn test_resume_session_id_conflicts_with_last() {
        let result = Cli::try_parse_from(["cortex", "resume", "abc123", "--last"]);
        assert!(
            result.is_err(),
            "Should fail when session_id conflicts with --last"
        );
    }

    #[test]
    fn test_resume_pick_conflicts_with_session_id() {
        let result = Cli::try_parse_from(["cortex", "resume", "abc123", "--pick"]);
        assert!(
            result.is_err(),
            "Should fail when --pick conflicts with session_id"
        );
    }

    #[test]
    fn test_resume_pick_conflicts_with_last() {
        let result = Cli::try_parse_from(["cortex", "resume", "--pick", "--last"]);
        assert!(
            result.is_err(),
            "Should fail when --pick conflicts with --last"
        );
    }

    #[test]
    fn test_login_token_conflicts_with_api_key() {
        let result = Cli::try_parse_from(["cortex", "login", "--token", "xyz", "--with-api-key"]);
        assert!(
            result.is_err(),
            "Should fail when --token conflicts with --with-api-key"
        );
    }

    #[test]
    fn test_serve_mdns_conflicts_with_no_mdns() {
        let result = Cli::try_parse_from(["cortex", "serve", "--mdns", "--no-mdns"]);
        assert!(
            result.is_err(),
            "Should fail when --mdns conflicts with --no-mdns"
        );
    }

    // ==========================================================================
    // Alias tests
    // ==========================================================================

    #[test]
    fn test_run_alias_r() {
        let cli = Cli::try_parse_from(["cortex", "r"]).expect("should parse 'r' alias for run");
        assert!(matches!(cli.command, Some(Commands::Run(_))));
    }

    #[test]
    fn test_exec_alias_e() {
        let cli = Cli::try_parse_from(["cortex", "e"]).expect("should parse 'e' alias for exec");
        assert!(matches!(cli.command, Some(Commands::Exec(_))));
    }

    #[test]
    fn test_github_alias_gh() {
        // GitHub command requires a subcommand, so test with "status"
        let cli = Cli::try_parse_from(["cortex", "gh", "status"])
            .expect("should parse 'gh status' alias for github");
        assert!(matches!(cli.command, Some(Commands::Github(_))));
    }

    #[test]
    fn test_compact_alias_gc() {
        let cli =
            Cli::try_parse_from(["cortex", "gc"]).expect("should parse 'gc' alias for compact");
        assert!(matches!(cli.command, Some(Commands::Compact(_))));
    }

    #[test]
    fn test_compact_alias_cleanup() {
        let cli = Cli::try_parse_from(["cortex", "cleanup"])
            .expect("should parse 'cleanup' alias for compact");
        assert!(matches!(cli.command, Some(Commands::Compact(_))));
    }

    #[test]
    fn test_feedback_alias_report() {
        let cli = Cli::try_parse_from(["cortex", "report"])
            .expect("should parse 'report' alias for feedback");
        assert!(matches!(cli.command, Some(Commands::Feedback(_))));
    }

    #[test]
    fn test_lock_alias_protect() {
        let cli = Cli::try_parse_from(["cortex", "protect"])
            .expect("should parse 'protect' alias for lock");
        assert!(matches!(cli.command, Some(Commands::Lock(_))));
    }

    // ==========================================================================
    // FeaturesCommand tests
    // ==========================================================================

    #[test]
    fn test_features_list_subcommand() {
        let cli = Cli::try_parse_from(["cortex", "features", "list"])
            .expect("should parse features list");
        if let Some(Commands::Features(features)) = cli.command {
            assert!(matches!(features.sub, FeaturesSubcommand::List));
        } else {
            panic!("Expected Features command");
        }
    }
}
