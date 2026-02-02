//! Agent management CLI commands.
//!
//! Provides commands for managing Cortex Agents:
//! - `cortex agent list` - List all available agents
//! - `cortex agent create` - Interactive wizard to create a new agent
//! - `cortex agent show <name>` - Show agent details
//! - `cortex agent edit <name>` - Edit an existing agent
//! - `cortex agent remove <name>` - Remove a user-defined agent
//! - `cortex agent install <name>` - Install an agent from the registry
//! - `cortex agent copy <source> <dest>` - Copy/clone an existing agent
//! - `cortex agent export <name>` - Export an agent definition

mod cli;
mod handlers;
mod loader;
mod prompts;
#[cfg(test)]
mod tests;
mod types;
mod utils;

// Re-export public items
pub use cli::{
    AgentCli, AgentSubcommand, CopyArgs, CreateArgs, EditArgs, ExportArgs, InstallArgs, ListArgs,
    RemoveArgs, ShowArgs,
};
pub use loader::load_all_agents;
pub use types::{AgentFrontmatter, AgentInfo, AgentMode, AgentSource};
