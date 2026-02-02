//! MCP Servers Card
//!
//! A card for managing MCP (Model Context Protocol) servers.
//! Shows server status and allows management operations like restart, add, and delete.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::widgets::Widget;

use cortex_core::style::{
    CYAN_PRIMARY, GREEN, ORANGE, RED, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, VOID,
};

use crate::cards::{CancellationEvent, CardAction, CardResult, CardView};
use crate::widgets::{SelectionItem, SelectionList, SelectionResult};

// ============================================================
// MCP STATUS
// ============================================================

/// Status of an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStatus {
    /// Server is running and connected
    Running,
    /// Server is stopped/disconnected
    Stopped,
    /// Server is starting up
    Starting,
    /// Server encountered an error
    Error,
}

impl McpStatus {
    /// Returns the status indicator symbol.
    pub fn symbol(&self) -> &'static str {
        match self {
            McpStatus::Running => "●",
            McpStatus::Stopped => "○",
            McpStatus::Starting => "◐",
            McpStatus::Error => "✗",
        }
    }

    /// Returns the color for this status.
    pub fn color(&self) -> ratatui::style::Color {
        match self {
            McpStatus::Running => GREEN,
            McpStatus::Stopped => TEXT_MUTED,
            McpStatus::Starting => ORANGE,
            McpStatus::Error => RED,
        }
    }

    /// Returns a human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            McpStatus::Running => "Running",
            McpStatus::Stopped => "Stopped",
            McpStatus::Starting => "Starting",
            McpStatus::Error => "Error",
        }
    }
}

// ============================================================
// MCP SERVER INFO
// ============================================================

/// Information about an MCP server.
#[derive(Debug, Clone)]
pub struct McpServerInfo {
    /// Server name/identifier
    pub name: String,
    /// Current status
    pub status: McpStatus,
    /// Number of tools provided by the server
    pub tool_count: usize,
    /// Error message if status is Error
    pub error: Option<String>,
}

impl McpServerInfo {
    /// Creates a new MCP server info.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status: McpStatus::Stopped,
            tool_count: 0,
            error: None,
        }
    }

    /// Sets the server status.
    pub fn with_status(mut self, status: McpStatus) -> Self {
        self.status = status;
        self
    }

    /// Sets the tool count.
    pub fn with_tool_count(mut self, count: usize) -> Self {
        self.tool_count = count;
        self
    }

    /// Sets an error message.
    pub fn with_error(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self.status = McpStatus::Error;
        self
    }

    /// Formats the server info for display (legacy method).
    #[cfg(test)]
    fn format_display(&self) -> String {
        let symbol = self.status.symbol();
        let name = &self.name;

        match self.status {
            McpStatus::Running => {
                if self.tool_count > 0 {
                    format!(
                        "{} {} - {} - {} tools",
                        symbol,
                        name,
                        self.status.label(),
                        self.tool_count
                    )
                } else {
                    format!("{} {} - {}", symbol, name, self.status.label())
                }
            }
            McpStatus::Error => {
                if let Some(ref err) = self.error {
                    format!("{} {} - {}: {}", symbol, name, self.status.label(), err)
                } else {
                    format!("{} {} - {}", symbol, name, self.status.label())
                }
            }
            _ => {
                format!("{} {} - {}", symbol, name, self.status.label())
            }
        }
    }

    /// Formats the status description for the SelectionItem description field.
    fn format_status_description(&self) -> String {
        match self.status {
            McpStatus::Running => {
                if self.tool_count > 0 {
                    format!(
                        "{} {} - {} tools",
                        self.status.symbol(),
                        self.status.label(),
                        self.tool_count
                    )
                } else {
                    format!("{} {}", self.status.symbol(), self.status.label())
                }
            }
            McpStatus::Error => {
                if let Some(ref err) = self.error {
                    format!("{} {}: {}", self.status.symbol(), self.status.label(), err)
                } else {
                    format!("{} {}", self.status.symbol(), self.status.label())
                }
            }
            _ => {
                format!("{} {}", self.status.symbol(), self.status.label())
            }
        }
    }
}

// ============================================================
// MCP CARD MODE
// ============================================================

/// Current mode of the MCP card.
#[derive(Debug, Clone)]
enum McpCardMode {
    /// Default list view
    List,
    /// Adding a new server
    AddServer { name: String },
    /// Confirming server deletion
    ConfirmDelete { server: String },
    /// Viewing server logs
    ViewLogs { server: String, logs: Vec<String> },
}

// ============================================================
// MCP CARD
// ============================================================

/// Card for managing MCP servers.
pub struct McpCard {
    /// List of MCP servers
    servers: Vec<McpServerInfo>,
    /// Selection list widget
    list: SelectionList,
    /// Current card mode
    mode: McpCardMode,
}

impl McpCard {
    /// Creates a new MCP card with the given servers.
    pub fn new(servers: Vec<McpServerInfo>) -> Self {
        let items: Vec<SelectionItem> = servers
            .iter()
            .map(|server| {
                let status_text = server.format_status_description();
                SelectionItem::new(&server.name)
                    .with_description(status_text)
                    .with_current(server.status == McpStatus::Running)
            })
            .collect();

        let list = SelectionList::new(items).with_max_visible(10);

        Self {
            servers,
            list,
            mode: McpCardMode::List,
        }
    }

    /// Creates a new MCP card directly in AddServer mode.
    pub fn new_add_mode(servers: Vec<McpServerInfo>) -> Self {
        let items: Vec<SelectionItem> = servers
            .iter()
            .map(|server| {
                let status_text = server.format_status_description();
                SelectionItem::new(&server.name)
                    .with_description(status_text)
                    .with_current(server.status == McpStatus::Running)
            })
            .collect();

        let list = SelectionList::new(items).with_max_visible(10);

        Self {
            servers,
            list,
            mode: McpCardMode::AddServer {
                name: String::new(),
            },
        }
    }

    /// Gets the currently selected server name.
    fn selected_server_name(&self) -> Option<&str> {
        self.list
            .selected_index()
            .and_then(|idx| self.servers.get(idx))
            .map(|s| s.name.as_str())
    }

    /// Handles key events in List mode.
    fn handle_list_key(&mut self, key: KeyEvent) -> CardResult {
        match key.code {
            // Enter: No action in list mode (could expand details in future)
            KeyCode::Enter => CardResult::Continue,

            // Add new server
            KeyCode::Char('a') => {
                self.mode = McpCardMode::AddServer {
                    name: String::new(),
                };
                CardResult::Continue
            }

            // Restart selected server
            KeyCode::Char('r') => {
                if let Some(name) = self.selected_server_name() {
                    CardResult::Action(CardAction::RestartMcpServer(name.to_string()))
                } else {
                    CardResult::Continue
                }
            }

            // Delete selected server
            KeyCode::Char('d') => {
                if let Some(name) = self.selected_server_name() {
                    self.mode = McpCardMode::ConfirmDelete {
                        server: name.to_string(),
                    };
                }
                CardResult::Continue
            }

            // View logs
            KeyCode::Char('l') => {
                if let Some(name) = self.selected_server_name() {
                    self.mode = McpCardMode::ViewLogs {
                        server: name.to_string(),
                        logs: vec![
                            format!("[INFO] Server '{}' started", name),
                            "[INFO] Initializing tools...".to_string(),
                            "[INFO] Ready to accept connections".to_string(),
                        ],
                    };
                }
                CardResult::Continue
            }

            // Close card
            KeyCode::Esc => CardResult::Close,

            // Delegate to selection list
            _ => {
                let result = self.list.handle_key(key);
                match result {
                    SelectionResult::Cancelled => CardResult::Close,
                    _ => CardResult::Continue,
                }
            }
        }
    }

    /// Handles key events in AddServer mode.
    fn handle_add_server_key(&mut self, key: KeyEvent) -> CardResult {
        match &mut self.mode {
            McpCardMode::AddServer { name } => match key.code {
                KeyCode::Enter => {
                    if !name.is_empty() {
                        let server_name = name.clone();
                        self.mode = McpCardMode::List;
                        CardResult::Action(CardAction::AddMcpServer(server_name))
                    } else {
                        self.mode = McpCardMode::List;
                        CardResult::Continue
                    }
                }
                KeyCode::Esc => {
                    self.mode = McpCardMode::List;
                    CardResult::Continue
                }
                KeyCode::Backspace => {
                    name.pop();
                    CardResult::Continue
                }
                KeyCode::Char(c) => {
                    name.push(c);
                    CardResult::Continue
                }
                _ => CardResult::Continue,
            },
            _ => CardResult::Continue,
        }
    }

    /// Handles key events in ConfirmDelete mode.
    fn handle_confirm_delete_key(&mut self, key: KeyEvent) -> CardResult {
        match &self.mode {
            McpCardMode::ConfirmDelete { server } => match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let server_name = server.clone();
                    self.mode = McpCardMode::List;
                    CardResult::Action(CardAction::RemoveMcpServer(server_name))
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.mode = McpCardMode::List;
                    CardResult::Continue
                }
                _ => CardResult::Continue,
            },
            _ => CardResult::Continue,
        }
    }

    /// Handles key events in ViewLogs mode.
    fn handle_view_logs_key(&mut self, key: KeyEvent) -> CardResult {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.mode = McpCardMode::List;
                CardResult::Continue
            }
            _ => CardResult::Continue,
        }
    }

    /// Renders the list view.
    fn render_list(&self, area: Rect, buf: &mut Buffer) {
        <&SelectionList as Widget>::render(&self.list, area, buf);
    }

    /// Renders the add server input.
    fn render_add_server(&self, name: &str, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 20 {
            return;
        }

        let y = area.y;

        // Prompt
        let prompt = "Enter server name:";
        buf.set_string(area.x, y, prompt, Style::default().fg(TEXT));

        // Input field background
        let input_y = y + 1;
        for x in area.x..area.x.saturating_add(area.width) {
            buf[(x, input_y)].set_bg(SURFACE_1);
        }

        // Input text
        let input_style = Style::default().fg(TEXT).bg(SURFACE_1);
        buf.set_string(area.x + 1, input_y, name, input_style);

        // Cursor
        let cursor_x = area.x + 1 + name.len() as u16;
        if cursor_x < area.x + area.width - 1 {
            buf[(cursor_x, input_y)].set_bg(CYAN_PRIMARY);
            buf[(cursor_x, input_y)].set_fg(VOID);
        }

        // Hint
        let hint_y = y + 2;
        if hint_y < area.y + area.height {
            buf.set_string(
                area.x,
                hint_y,
                "Press Enter to add, Esc to cancel",
                Style::default().fg(TEXT_DIM),
            );
        }
    }

    /// Renders the delete confirmation dialog.
    fn render_confirm_delete(&self, server: &str, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 20 {
            return;
        }

        let y = area.y;

        // Warning message
        let warning = format!("Delete server '{}'?", server);
        buf.set_string(
            area.x,
            y,
            &warning,
            Style::default().fg(RED).add_modifier(Modifier::BOLD),
        );

        // Confirmation prompt
        let prompt_y = y + 1;
        if prompt_y < area.y + area.height {
            buf.set_string(
                area.x,
                prompt_y,
                "Press Y to confirm, N or Esc to cancel",
                Style::default().fg(TEXT_DIM),
            );
        }
    }

    /// Renders the logs view.
    fn render_view_logs(&self, server: &str, logs: &[String], area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 20 {
            return;
        }

        // Header
        let header = format!("Logs: {}", server);
        buf.set_string(
            area.x,
            area.y,
            &header,
            Style::default()
                .fg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD),
        );

        // Log entries
        for (i, log) in logs.iter().enumerate() {
            let y = area.y + 2 + i as u16;
            if y >= area.y + area.height {
                break;
            }

            // Truncate log if too long
            let max_len = area.width as usize;
            let display_log = if log.len() > max_len {
                format!("{}...", &log[..max_len.saturating_sub(3)])
            } else {
                log.clone()
            };

            buf.set_string(area.x, y, &display_log, Style::default().fg(TEXT_DIM));
        }

        // Footer hint
        let hint_y = area.y + area.height.saturating_sub(1);
        buf.set_string(
            area.x,
            hint_y,
            "Press Esc or Q to go back",
            Style::default().fg(TEXT_MUTED),
        );
    }
}

impl CardView for McpCard {
    fn title(&self) -> &str {
        "MCP Servers"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        let base_height = match &self.mode {
            McpCardMode::List => {
                // Number of servers + some padding
                (self.servers.len() as u16 + 2).min(12)
            }
            McpCardMode::AddServer { .. } => 4,
            McpCardMode::ConfirmDelete { .. } => 3,
            McpCardMode::ViewLogs { logs, .. } => {
                // Header + logs + footer
                (logs.len() as u16 + 4).min(15)
            }
        };

        base_height.min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        match &self.mode {
            McpCardMode::List => self.render_list(area, buf),
            McpCardMode::AddServer { name } => self.render_add_server(name, area, buf),
            McpCardMode::ConfirmDelete { server } => self.render_confirm_delete(server, area, buf),
            McpCardMode::ViewLogs { server, logs } => {
                self.render_view_logs(server, logs, area, buf)
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> CardResult {
        match &self.mode {
            McpCardMode::List => self.handle_list_key(key),
            McpCardMode::AddServer { .. } => self.handle_add_server_key(key),
            McpCardMode::ConfirmDelete { .. } => self.handle_confirm_delete_key(key),
            McpCardMode::ViewLogs { .. } => self.handle_view_logs_key(key),
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.mode {
            McpCardMode::List => vec![
                ("a", "Add"),
                ("r", "Restart"),
                ("d", "Delete"),
                ("l", "Logs"),
                ("Esc", "Close"),
            ],
            McpCardMode::AddServer { .. } => vec![("Enter", "Add"), ("Esc", "Cancel")],
            McpCardMode::ConfirmDelete { .. } => vec![("Y", "Confirm"), ("N", "Cancel")],
            McpCardMode::ViewLogs { .. } => vec![("Esc", "Back")],
        }
    }

    fn on_cancel(&mut self) -> CancellationEvent {
        match &self.mode {
            McpCardMode::List => CancellationEvent::NotHandled,
            _ => {
                self.mode = McpCardMode::List;
                CancellationEvent::Handled
            }
        }
    }

    fn is_complete(&self) -> bool {
        false
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    fn create_test_servers() -> Vec<McpServerInfo> {
        vec![
            McpServerInfo::new("filesystem")
                .with_status(McpStatus::Running)
                .with_tool_count(3),
            McpServerInfo::new("github").with_status(McpStatus::Stopped),
            McpServerInfo::new("postgres").with_error("Connection refused"),
        ]
    }

    #[test]
    fn test_mcp_status_symbol() {
        assert_eq!(McpStatus::Running.symbol(), "●");
        assert_eq!(McpStatus::Stopped.symbol(), "○");
        assert_eq!(McpStatus::Starting.symbol(), "◐");
        assert_eq!(McpStatus::Error.symbol(), "✗");
    }

    #[test]
    fn test_mcp_status_label() {
        assert_eq!(McpStatus::Running.label(), "Running");
        assert_eq!(McpStatus::Stopped.label(), "Stopped");
        assert_eq!(McpStatus::Starting.label(), "Starting");
        assert_eq!(McpStatus::Error.label(), "Error");
    }

    #[test]
    fn test_mcp_server_info_new() {
        let server = McpServerInfo::new("test");
        assert_eq!(server.name, "test");
        assert_eq!(server.status, McpStatus::Stopped);
        assert_eq!(server.tool_count, 0);
        assert!(server.error.is_none());
    }

    #[test]
    fn test_mcp_server_info_builder() {
        let server = McpServerInfo::new("filesystem")
            .with_status(McpStatus::Running)
            .with_tool_count(5);

        assert_eq!(server.name, "filesystem");
        assert_eq!(server.status, McpStatus::Running);
        assert_eq!(server.tool_count, 5);
    }

    #[test]
    fn test_mcp_server_info_with_error() {
        let server = McpServerInfo::new("broken").with_error("Connection refused");

        assert_eq!(server.status, McpStatus::Error);
        assert_eq!(server.error, Some("Connection refused".to_string()));
    }

    #[test]
    fn test_mcp_card_new() {
        let servers = create_test_servers();
        let card = McpCard::new(servers);

        assert_eq!(card.title(), "MCP Servers");
        assert_eq!(card.servers.len(), 3);
    }

    #[test]
    fn test_mcp_card_key_hints_list_mode() {
        let card = McpCard::new(create_test_servers());
        let hints = card.key_hints();

        assert!(hints.iter().any(|(k, _)| *k == "a"));
        assert!(hints.iter().any(|(k, _)| *k == "r"));
        assert!(hints.iter().any(|(k, _)| *k == "d"));
        assert!(hints.iter().any(|(k, _)| *k == "l"));
    }

    #[test]
    fn test_mcp_card_switch_to_add_mode() {
        let mut card = McpCard::new(create_test_servers());

        let key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Continue));
        assert!(matches!(card.mode, McpCardMode::AddServer { .. }));
    }

    #[test]
    fn test_mcp_card_switch_to_delete_mode() {
        let mut card = McpCard::new(create_test_servers());

        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Continue));
        assert!(matches!(card.mode, McpCardMode::ConfirmDelete { .. }));
    }

    #[test]
    fn test_mcp_card_switch_to_logs_mode() {
        let mut card = McpCard::new(create_test_servers());

        let key = KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Continue));
        assert!(matches!(card.mode, McpCardMode::ViewLogs { .. }));
    }

    #[test]
    fn test_mcp_card_restart_action() {
        let mut card = McpCard::new(create_test_servers());

        let key = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(
            result,
            CardResult::Action(CardAction::RestartMcpServer(_))
        ));
    }

    #[test]
    fn test_mcp_card_escape_closes() {
        let mut card = McpCard::new(create_test_servers());

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Close));
    }

    #[test]
    fn test_mcp_card_add_server_confirm() {
        let mut card = McpCard::new(create_test_servers());

        // Switch to add mode
        card.mode = McpCardMode::AddServer {
            name: "newserver".to_string(),
        };

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(
            matches!(result, CardResult::Action(CardAction::AddMcpServer(name)) if name == "newserver")
        );
    }

    #[test]
    fn test_mcp_card_delete_confirm() {
        let mut card = McpCard::new(create_test_servers());

        // Switch to confirm delete mode
        card.mode = McpCardMode::ConfirmDelete {
            server: "filesystem".to_string(),
        };

        let key = KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(
            matches!(result, CardResult::Action(CardAction::RemoveMcpServer(name)) if name == "filesystem")
        );
    }

    #[test]
    fn test_mcp_card_delete_cancel() {
        let mut card = McpCard::new(create_test_servers());

        // Switch to confirm delete mode
        card.mode = McpCardMode::ConfirmDelete {
            server: "filesystem".to_string(),
        };

        let key = KeyEvent::new(KeyCode::Char('n'), KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Continue));
        assert!(matches!(card.mode, McpCardMode::List));
    }

    #[test]
    fn test_mcp_card_on_cancel_in_submode() {
        let mut card = McpCard::new(create_test_servers());
        card.mode = McpCardMode::AddServer {
            name: String::new(),
        };

        let result = card.on_cancel();

        assert!(matches!(result, CancellationEvent::Handled));
        assert!(matches!(card.mode, McpCardMode::List));
    }

    #[test]
    fn test_mcp_card_on_cancel_in_list_mode() {
        let mut card = McpCard::new(create_test_servers());

        let result = card.on_cancel();

        assert!(matches!(result, CancellationEvent::NotHandled));
    }

    #[test]
    fn test_mcp_card_desired_height() {
        let card = McpCard::new(create_test_servers());

        let height = card.desired_height(20, 80);
        assert!(height >= 3); // At least 3 servers
        assert!(height <= 12); // Capped
    }

    #[test]
    fn test_format_display_running_with_tools() {
        let server = McpServerInfo::new("filesystem")
            .with_status(McpStatus::Running)
            .with_tool_count(3);

        let display = server.format_display();
        assert!(display.contains("●"));
        assert!(display.contains("filesystem"));
        assert!(display.contains("Running"));
        assert!(display.contains("3 tools"));
    }

    #[test]
    fn test_format_display_error() {
        let server = McpServerInfo::new("broken").with_error("Connection refused");

        let display = server.format_display();
        assert!(display.contains("✗"));
        assert!(display.contains("broken"));
        assert!(display.contains("Error"));
        assert!(display.contains("Connection refused"));
    }
}
