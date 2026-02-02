//! CLI command definitions for agent management.
//!
//! Contains the clap command structures and argument definitions.

use clap::Parser;
use std::path::PathBuf;

use super::handlers;

/// Agent management CLI.
#[derive(Debug, Parser)]
pub struct AgentCli {
    #[command(subcommand)]
    pub subcommand: AgentSubcommand,
}

/// Agent subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum AgentSubcommand {
    /// List all available agents.
    List(ListArgs),

    /// Show details for a specific agent.
    Show(ShowArgs),

    /// Create a new agent interactively.
    Create(CreateArgs),

    /// Edit an existing agent in your default editor.
    Edit(EditArgs),

    /// Remove a user-defined agent.
    Remove(RemoveArgs),

    /// Install an agent from the registry.
    Install(InstallArgs),

    /// Copy/clone an existing agent with a new name.
    #[command(visible_alias = "clone")]
    Copy(CopyArgs),

    /// Export an agent definition to stdout or a file.
    Export(ExportArgs),
}

/// Arguments for list command.
#[derive(Debug, Parser)]
pub struct ListArgs {
    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Show only primary agents.
    #[arg(long)]
    pub primary: bool,

    /// Show only subagents.
    #[arg(long)]
    pub subagents: bool,

    /// Show all agents including hidden ones.
    #[arg(long)]
    pub all: bool,

    /// List agents from remote registry.
    #[arg(long)]
    pub remote: bool,

    /// Filter agents by pattern (glob-style matching).
    /// Example: --filter "python*" or --filter "*test*"
    #[arg(long)]
    pub filter: Option<String>,

    /// Output only agent names (one per line) for shell completion.
    #[arg(long, hide = true)]
    pub names_only: bool,
}

/// Arguments for show command.
#[derive(Debug, Parser)]
pub struct ShowArgs {
    /// Name of the agent to display.
    pub name: String,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,

    /// Show how agent would work with a specific model override.
    #[arg(long)]
    pub model: Option<String>,
}

/// Arguments for create command.
#[derive(Debug, Parser)]
#[command(
    about = "Create a new custom agent",
    long_about = "Create a new custom agent with a specific persona and capabilities.\n\n\
    Agents can be customized with:\n\
    - Custom system prompts that define the agent's personality and expertise\n\
    - Tool restrictions (allowed/disallowed tools) to limit agent capabilities\n\
    - Model preferences for specific use cases\n\
    - Color themes for visual identification in the UI\n\
    - Delegation settings to control sub-agent spawning\n\n\
    Examples:\n\
      cortex agent create my-agent --model gpt-4o --color '#FF5733'\n\
      cortex agent create reviewer --generate \"A code reviewer focused on security\"\n\
      cortex agent create --name helper --non-interactive"
)]
pub struct CreateArgs {
    /// Agent name (if not provided, interactive mode will prompt).
    #[arg(short, long)]
    pub name: Option<String>,

    /// Agent description.
    #[arg(short, long)]
    pub description: Option<String>,

    /// Agent mode: primary, subagent, or all.
    #[arg(short, long)]
    pub mode: Option<String>,

    /// Skip interactive prompts and use defaults.
    #[arg(long)]
    pub non_interactive: bool,

    /// Generate agent using AI from a natural language description.
    /// Example: --generate "A Rust expert that helps with memory safety and performance"
    /// Note: This feature requires authentication. Run 'cortex login' first or set CORTEX_AUTH_TOKEN.
    #[arg(short, long, value_name = "DESCRIPTION")]
    pub generate: Option<String>,

    /// Model to use for AI generation (default: gpt-4o).
    #[arg(long, default_value = "gpt-4o")]
    pub model: String,
}

/// Arguments for edit command.
#[derive(Debug, Parser)]
pub struct EditArgs {
    /// Name of the agent to edit.
    pub name: String,

    /// Editor to use (defaults to $EDITOR or $VISUAL).
    #[arg(short, long)]
    pub editor: Option<String>,
}

/// Arguments for remove command.
#[derive(Debug, Parser)]
pub struct RemoveArgs {
    /// Name of the agent to remove.
    pub name: String,

    /// Force removal without confirmation.
    #[arg(short, long)]
    pub force: bool,
}

/// Arguments for install command.
#[derive(Debug, Parser)]
pub struct InstallArgs {
    /// Name or URL of the agent to install from registry.
    pub name: String,

    /// Force overwrite if agent already exists.
    #[arg(short, long)]
    pub force: bool,

    /// Registry URL to install from (defaults to official registry).
    #[arg(long)]
    pub registry: Option<String>,
}

/// Arguments for copy command.
#[derive(Debug, Parser)]
pub struct CopyArgs {
    /// Name of the agent to copy.
    pub source: String,

    /// Name for the new agent copy.
    pub destination: String,

    /// Force overwrite if destination agent already exists.
    #[arg(short, long)]
    pub force: bool,
}

/// Arguments for export command.
#[derive(Debug, Parser)]
pub struct ExportArgs {
    /// Name of the agent to export.
    pub name: String,

    /// Output file path (defaults to stdout).
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Export as JSON instead of markdown.
    #[arg(long)]
    pub json: bool,
}

impl AgentCli {
    /// Run the agent command.
    pub async fn run(self) -> anyhow::Result<()> {
        match self.subcommand {
            AgentSubcommand::List(args) => handlers::run_list(args).await,
            AgentSubcommand::Show(args) => handlers::run_show(args).await,
            AgentSubcommand::Create(args) => handlers::run_create(args).await,
            AgentSubcommand::Edit(args) => handlers::run_edit(args).await,
            AgentSubcommand::Remove(args) => handlers::run_remove(args).await,
            AgentSubcommand::Install(args) => handlers::run_install(args).await,
            AgentSubcommand::Copy(args) => handlers::run_copy(args).await,
            AgentSubcommand::Export(args) => handlers::run_export(args).await,
        }
    }
}
