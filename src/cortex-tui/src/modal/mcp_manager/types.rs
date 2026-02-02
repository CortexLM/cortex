//! MCP Manager types
//!
//! Contains all type definitions for the MCP Manager modal.

use cortex_core::style::{ERROR, SUCCESS, TEXT_DIM, WARNING};

// ============================================================================
// MCP SERVER INFO
// ============================================================================

/// Status of an MCP server
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStatus {
    Running,
    Stopped,
    Starting,
    Error,
}

impl McpStatus {
    pub fn icon(&self) -> &'static str {
        match self {
            McpStatus::Running => "[*]",
            McpStatus::Stopped => "[ ]",
            McpStatus::Starting => "[~]",
            McpStatus::Error => "[!]",
        }
    }

    pub fn color(&self) -> ratatui::style::Color {
        match self {
            McpStatus::Running => SUCCESS,
            McpStatus::Stopped => TEXT_DIM,
            McpStatus::Starting => WARNING,
            McpStatus::Error => ERROR,
        }
    }

    pub fn text(&self) -> &'static str {
        match self {
            McpStatus::Running => "Running",
            McpStatus::Stopped => "Stopped",
            McpStatus::Starting => "Starting",
            McpStatus::Error => "Error",
        }
    }
}

/// Information about an MCP server for display
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    pub name: String,
    pub status: McpStatus,
    pub tool_count: usize,
    pub error: Option<String>,
    pub requires_auth: bool,
}

// ============================================================================
// ADD SERVER WIZARD TYPES
// ============================================================================

/// Source of the MCP server configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpServerSource {
    /// Custom configuration entered by user
    Custom,
    /// From MCP Registry (predefined servers)
    Registry,
}

/// Transport type for MCP server communication
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpTransportType {
    /// Stdio transport (subprocess communication)
    Stdio,
    /// HTTP transport (remote server)
    Http,
}

/// Registry loading state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegistryLoadState {
    #[default]
    NotLoaded,
    _Loading,
    Loaded,
    _Error,
}

// ============================================================================
// MODE ENUMS
// ============================================================================

/// Mode/state of the MCP Manager
#[derive(Debug, Clone)]
pub enum McpMode {
    /// Main list view
    List,
    /// Step 1: Choose source (Custom or Registry)
    ChooseSource {
        selected: usize, // 0 = Custom, 1 = Registry
    },
    /// Step 2: Choose transport type (stdio or HTTP)
    ChooseTransport {
        _source: McpServerSource,
        selected: usize, // 0 = Stdio, 1 = HTTP
    },
    /// Step 3a: Adding a stdio server
    AddStdioServer {
        name: String,
        command: String,
        args: String,
        focus: AddStdioServerFocus,
    },
    /// Step 3b: Adding an HTTP server
    AddHttpServer {
        name: String,
        url: String,
        focus: AddHttpServerFocus,
    },
    /// Step 3c: Select from registry
    SelectFromRegistry {
        selected: usize,
        search_query: String,
        /// Cached registry entries (if loaded)
        entries: Vec<super::registry::RegistryEntry>,
        /// Loading state
        _load_state: RegistryLoadState,
    },
    /// Confirming deletion
    ConfirmDelete { server_name: String },
    /// Setting API key
    SetAuth {
        server_name: String,
        api_key: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddStdioServerFocus {
    Name,
    Command,
    Args,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddHttpServerFocus {
    Name,
    Url,
}
