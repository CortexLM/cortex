//! Builder for MCP server selection.

use crate::interactive::state::{
    InlineFormField, InlineFormState, InteractiveAction, InteractiveItem, InteractiveState,
};
use crate::modal::mcp_manager::{McpServerInfo, McpStatus};

/// Build an interactive state for MCP server management.
pub fn build_mcp_selector(servers: &[McpServerInfo]) -> InteractiveState {
    let mut items: Vec<InteractiveItem> = Vec::new();

    // Add global actions first (separated from servers)
    items.push(
        InteractiveItem::new("__add__", "Add MCP Server")
            .with_description("Configure a new server (stdio, HTTP, or from registry)")
            .with_shortcut('a'),
    );

    items.push(
        InteractiveItem::new("__tools__", "View All Tools")
            .with_description("List tools from all running servers")
            .with_shortcut('t'),
    );

    items.push(
        InteractiveItem::new("__reload__", "Reload All Servers")
            .with_description("Restart all MCP servers")
            .with_shortcut('r'),
    );

    // Add separator if there are servers
    if !servers.is_empty() {
        items.push(
            InteractiveItem::new("__sep_servers__", "--- Configured Servers ---").as_separator(),
        );
    }

    // Add server entries
    for server in servers {
        let status_text = match server.status {
            McpStatus::Running => "running",
            McpStatus::Starting => "starting",
            McpStatus::Stopped => "stopped",
            McpStatus::Error => "error",
        };

        let description = format!("{} - {} tools", status_text, server.tool_count);

        let mut item =
            InteractiveItem::new(&server.name, &server.name).with_description(description);

        if server.requires_auth {
            item = item.with_metadata("requires_auth".to_string());
        }

        items.push(item);
    }

    let title = if servers.is_empty() {
        "MCP Servers".to_string()
    } else {
        let running = servers
            .iter()
            .filter(|s| matches!(s.status, McpStatus::Running))
            .count();
        format!("MCP Servers ({}/{})", running, servers.len())
    };

    InteractiveState::new(title, items, InteractiveAction::McpServerAction)
        .with_search()
        .with_hints(vec![
            ("Up/Down".to_string(), "navigate".to_string()),
            ("Enter".to_string(), "select".to_string()),
            ("/".to_string(), "search".to_string()),
            ("Esc".to_string(), "close".to_string()),
        ])
}

/// Build a selector for choosing MCP server source (Custom or Registry).
/// This is the first step when adding an MCP server.
pub fn build_mcp_source_selector() -> InteractiveState {
    let items = vec![
        InteractiveItem::new("custom", "Custom Server")
            .with_description("Configure a server with command/URL manually")
            .with_shortcut('c'),
        InteractiveItem::new("registry", "From Registry")
            .with_description("Browse and install from MCP server registry")
            .with_shortcut('r'),
    ];

    InteractiveState::new(
        "Add MCP Server",
        items,
        InteractiveAction::Custom("mcp-source".to_string()),
    )
    .with_hints(vec![
        ("Enter".to_string(), "select".to_string()),
        ("Esc".to_string(), "back".to_string()),
    ])
}

/// Build a selector for choosing MCP transport type (stdio or HTTP).
/// This is shown after selecting "Custom Server".
pub fn build_mcp_transport_selector() -> InteractiveState {
    let items = vec![
        InteractiveItem::new("stdio", "stdio (Local Process)")
            .with_description("Run a local command (npx, uvx, binary)")
            .with_shortcut('s'),
        InteractiveItem::new("http", "HTTP (Remote Server)")
            .with_description("Connect to a remote MCP server via HTTP/SSE")
            .with_shortcut('h'),
    ];

    InteractiveState::new(
        "Transport Type",
        items,
        InteractiveAction::Custom("mcp-transport".to_string()),
    )
    .with_hints(vec![
        ("Enter".to_string(), "select".to_string()),
        ("Esc".to_string(), "back".to_string()),
    ])
}

/// Build an inline form for adding a stdio MCP server.
pub fn build_mcp_stdio_form() -> InlineFormState {
    InlineFormState::new("Add stdio Server", "mcp-add-stdio")
        .with_field(
            InlineFormField::new("name", "Name")
                .required()
                .with_placeholder("server-name"),
        )
        .with_field(
            InlineFormField::new("command", "Command")
                .required()
                .with_placeholder("npx, uvx, or path/to/binary"),
        )
        .with_field(InlineFormField::new("args", "Args").with_placeholder("arg1 arg2 ..."))
}

/// Build an inline form for adding an HTTP MCP server.
pub fn build_mcp_http_form() -> InlineFormState {
    InlineFormState::new("Add HTTP Server", "mcp-add-http")
        .with_field(
            InlineFormField::new("name", "Name")
                .required()
                .with_placeholder("server-name"),
        )
        .with_field(
            InlineFormField::new("url", "URL")
                .required()
                .with_placeholder("https://api.example.com/mcp"),
        )
        .with_field(InlineFormField::new("api_key", "API Key").with_placeholder("optional"))
}

/// Build a selector for browsing MCP registry (placeholder).
pub fn build_mcp_registry_browser() -> InteractiveState {
    // Placeholder data - real registry integration planned
    let items = vec![
        InteractiveItem::new("__coming_soon__", "Registry Coming Soon")
            .with_description("MCP registry integration is under development")
            .with_disabled(true),
    ];

    InteractiveState::new(
        "MCP Registry",
        items,
        InteractiveAction::Custom("mcp-registry".to_string()),
    )
    .with_hints(vec![("Esc".to_string(), "back".to_string())])
}

/// Build an inline form for adding a new MCP server (legacy - kept for compatibility).
/// This form is displayed within the MCP panel, not as a separate modal.
pub fn build_mcp_add_server_form() -> InlineFormState {
    build_mcp_stdio_form()
}

/// Build an interactive state for MCP server actions (for a specific server).
pub fn build_mcp_server_actions(server: &McpServerInfo) -> InteractiveState {
    let mut items = Vec::new();

    match server.status {
        McpStatus::Running | McpStatus::Starting => {
            items.push(InteractiveItem::new("stop", "Stop Server").with_shortcut('s'));
            items.push(InteractiveItem::new("restart", "Restart Server").with_shortcut('r'));
        }
        McpStatus::Stopped | McpStatus::Error => {
            items.push(InteractiveItem::new("start", "Start Server").with_shortcut('s'));
        }
    }

    if server.requires_auth {
        items.push(InteractiveItem::new("auth", "Configure Authentication").with_shortcut('a'));
    }

    items.push(InteractiveItem::new("logs", "View Logs").with_shortcut('l'));

    items.push(
        InteractiveItem::new("remove", "Remove Server")
            .with_shortcut('d')
            .with_description("Remove this server from configuration"),
    );

    InteractiveState::new(
        format!("Actions: {}", server.name),
        items,
        InteractiveAction::Custom(format!("mcp:{}", server.name)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_server(name: &str, status: McpStatus) -> McpServerInfo {
        McpServerInfo {
            name: name.to_string(),
            status,
            tool_count: 5,
            error: None,
            requires_auth: false,
        }
    }

    #[test]
    fn test_build_mcp_selector_empty() {
        let state = build_mcp_selector(&[]);
        // Should have global actions: Add, Tools, Reload (no separator when empty)
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.items[0].id, "__add__");
        assert_eq!(state.items[1].id, "__tools__");
        assert_eq!(state.items[2].id, "__reload__");
    }

    #[test]
    fn test_build_mcp_selector_with_servers() {
        let servers = vec![
            create_test_server("test1", McpStatus::Running),
            create_test_server("test2", McpStatus::Stopped),
        ];
        let state = build_mcp_selector(&servers);
        // 3 global actions + 1 separator + 2 servers = 6 items
        assert_eq!(state.items.len(), 6);
    }

    #[test]
    fn test_build_mcp_server_actions() {
        let server = create_test_server("test", McpStatus::Running);
        let state = build_mcp_server_actions(&server);
        // Stop, Restart, Logs, Remove
        assert!(state.items.len() >= 4);
    }
}
