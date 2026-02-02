//! MCP (Model Context Protocol) management commands.
//!
//! This module provides CLI commands for managing MCP servers, including:
//! - Listing configured servers
//! - Adding/removing servers
//! - Enabling/disabling servers
//! - OAuth authentication
//! - Debug and connection testing

mod auth;
mod config;
mod debug;
mod handlers;
mod macros;
mod types;
mod validation;

use anyhow::Result;

// Re-export public types
pub use types::{McpCli, McpSubcommand};

impl McpCli {
    /// Run the MCP command.
    pub async fn run(self) -> Result<()> {
        let McpCli {
            config_overrides: _,
            subcommand,
        } = self;

        match subcommand {
            McpSubcommand::List(args) | McpSubcommand::Ls(args) => handlers::run_list(args).await,
            McpSubcommand::Get(args) => handlers::run_get(args).await,
            McpSubcommand::Add(args) => handlers::run_add(args).await,
            McpSubcommand::Remove(args) => handlers::run_remove(args).await,
            McpSubcommand::Enable(args) => handlers::run_enable(args).await,
            McpSubcommand::Disable(args) => handlers::run_disable(args).await,
            McpSubcommand::Rename(args) => handlers::run_rename(args).await,
            McpSubcommand::Auth(cmd) => auth::run_auth_command(cmd).await,
            McpSubcommand::Logout(args) => auth::run_logout(args).await,
            McpSubcommand::Debug(args) => debug::run_debug(args).await,
        }
    }
}
