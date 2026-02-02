//! MCP Server Manager Modal
//!
//! A comprehensive modal for managing MCP (Model Context Protocol) servers.
//! Supports listing, adding, removing, restarting, and authenticating servers.
//! Features a multi-step wizard for adding servers:
//! 1. Choose source: Custom configuration or MCP Registry
//! 2. Choose transport type: stdio or HTTP
//! 3. Enter server details based on transport type

mod handlers;
mod registry;
mod rendering;
mod state;
mod types;

// Re-export public types
pub use state::McpManagerModal;
pub use types::{McpServerInfo, McpServerSource, McpStatus, McpTransportType};

use crate::modal::{CancelBehavior, Modal, ModalResult};
use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};
use types::{AddHttpServerFocus, AddStdioServerFocus, McpMode, McpServerSource as Source};

impl Modal for McpManagerModal {
    fn title(&self) -> &str {
        "MCP Servers"
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        match &mut self.mode {
            McpMode::AddStdioServer {
                name,
                command,
                args,
                focus,
            } => {
                match focus {
                    AddStdioServerFocus::Name => name.push_str(text),
                    AddStdioServerFocus::Command => command.push_str(text),
                    AddStdioServerFocus::Args => args.push_str(text),
                }
                true
            }
            McpMode::AddHttpServer { name, url, focus } => {
                match focus {
                    AddHttpServerFocus::Name => name.push_str(text),
                    AddHttpServerFocus::Url => url.push_str(text),
                }
                true
            }
            McpMode::SetAuth {
                server_name: _,
                api_key,
            } => {
                api_key.push_str(text);
                true
            }
            McpMode::SelectFromRegistry {
                selected: _,
                search_query,
                entries: _,
                _load_state: _,
            } => {
                search_query.push_str(text);
                true
            }
            _ => false,
        }
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        match &self.mode {
            McpMode::List => {
                let items = self.servers.len() as u16;
                // 2 (border) + 1 (search) + 1 (sep) + items + 1 (sep) + 1 (action bar)
                (items + 6).min(max_height).max(10) // min 10 rows for better UX
            }
            McpMode::ChooseSource { .. } => 8,
            McpMode::ChooseTransport { .. } => 8,
            McpMode::AddStdioServer { .. } => 11,
            McpMode::AddHttpServer { .. } => 9,
            McpMode::SelectFromRegistry { .. } => 16,
            McpMode::ConfirmDelete { .. } => 7,
            McpMode::SetAuth { .. } => 6,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        match &self.mode {
            McpMode::List => self.render_list(area, buf),
            McpMode::ChooseSource { .. } => self.render_choose_source(area, buf),
            McpMode::ChooseTransport { .. } => self.render_choose_transport(area, buf),
            McpMode::AddStdioServer { .. } => self.render_add_stdio_server(area, buf),
            McpMode::AddHttpServer { .. } => self.render_add_http_server(area, buf),
            McpMode::SelectFromRegistry { .. } => self.render_select_from_registry(area, buf),
            McpMode::ConfirmDelete { .. } => self.render_confirm_delete(area, buf),
            McpMode::SetAuth { .. } => self.render_set_auth(area, buf),
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match &self.mode {
            McpMode::List => self.handle_list_key(key),
            McpMode::ChooseSource { .. } => self.handle_choose_source_key(key),
            McpMode::ChooseTransport { .. } => self.handle_choose_transport_key(key),
            McpMode::AddStdioServer { .. } => self.handle_add_stdio_server_key(key),
            McpMode::AddHttpServer { .. } => self.handle_add_http_server_key(key),
            McpMode::SelectFromRegistry { .. } => self.handle_select_from_registry_key(key),
            McpMode::ConfirmDelete { .. } => self.handle_confirm_delete_key(key),
            McpMode::SetAuth { .. } => self.handle_set_auth_key(key),
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        match &self.mode {
            McpMode::List => vec![
                ("↑↓", "navigate"),
                ("a", "add"),
                ("d", "delete"),
                ("r", "restart"),
                ("s", "start/stop"),
                ("k", "api key"),
                ("Esc", "close"),
            ],
            McpMode::ChooseSource { .. } => {
                vec![("↑↓", "select"), ("Enter", "confirm"), ("Esc", "cancel")]
            }
            McpMode::ChooseTransport { .. } => {
                vec![("↑↓", "select"), ("Enter", "confirm"), ("Esc", "back")]
            }
            McpMode::AddStdioServer { .. } => {
                vec![("Tab", "next field"), ("Enter", "save"), ("Esc", "back")]
            }
            McpMode::AddHttpServer { .. } => {
                vec![("Tab", "next field"), ("Enter", "save"), ("Esc", "back")]
            }
            McpMode::SelectFromRegistry { .. } => {
                vec![("↑↓", "select"), ("Enter", "add"), ("Esc", "back")]
            }
            McpMode::ConfirmDelete { .. } => vec![("Y", "confirm"), ("N", "cancel")],
            McpMode::SetAuth { .. } => vec![("Enter", "save"), ("Esc", "cancel")],
        }
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        match &self.mode {
            McpMode::List => CancelBehavior::Close,
            McpMode::ChooseSource { .. } => {
                self.mode = McpMode::List;
                CancelBehavior::Handled
            }
            McpMode::ChooseTransport { .. } => {
                self.mode = McpMode::ChooseSource { selected: 0 };
                CancelBehavior::Handled
            }
            McpMode::AddStdioServer { .. } => {
                self.mode = McpMode::ChooseTransport {
                    _source: Source::Custom,
                    selected: 0,
                };
                CancelBehavior::Handled
            }
            McpMode::AddHttpServer { .. } => {
                self.mode = McpMode::ChooseTransport {
                    _source: Source::Custom,
                    selected: 1,
                };
                CancelBehavior::Handled
            }
            McpMode::SelectFromRegistry { .. } => {
                self.mode = McpMode::ChooseSource { selected: 1 };
                CancelBehavior::Handled
            }
            _ => {
                self.mode = McpMode::List;
                CancelBehavior::Handled
            }
        }
    }

    fn is_searchable(&self) -> bool {
        matches!(
            self.mode,
            McpMode::List | McpMode::SelectFromRegistry { .. }
        )
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Search servers...")
    }
}
