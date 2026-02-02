//! MCP (Model Context Protocol) command handlers.

use super::CommandExecutor;
use crate::commands::types::{CommandResult, ModalType, ParsedCommand};

impl CommandExecutor {
    pub(super) fn cmd_mcp(&self, _cmd: &ParsedCommand) -> CommandResult {
        // Always open the interactive MCP panel - all management is centralized there
        CommandResult::OpenModal(ModalType::McpManager)
    }

    #[allow(dead_code)]
    pub(super) fn cmd_mcp_auth(&self, _cmd: &ParsedCommand) -> CommandResult {
        // Deprecated: redirect to interactive MCP panel
        CommandResult::OpenModal(ModalType::McpManager)
    }

    #[allow(dead_code)]
    pub(super) fn cmd_mcp_logs(&self, _cmd: &ParsedCommand) -> CommandResult {
        // Deprecated: redirect to interactive MCP panel
        CommandResult::OpenModal(ModalType::McpManager)
    }
}
