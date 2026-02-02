//! MCP Manager state
//!
//! Contains the McpManagerModal struct and its construction.

use super::types::{McpMode, McpServerInfo, McpStatus};
use crate::widgets::action_bar::ActionBar;
use crate::widgets::selection_list::{SelectionItem, SelectionList};

/// MCP Server Manager Modal
pub struct McpManagerModal {
    pub(crate) servers: Vec<McpServerInfo>,
    pub(crate) list: SelectionList,
    pub(crate) mode: McpMode,
}

impl McpManagerModal {
    /// Create a new MCP Manager modal with the given servers
    pub fn new(servers: Vec<McpServerInfo>) -> Self {
        let items: Vec<SelectionItem> = servers
            .iter()
            .map(|s| {
                let mut item = SelectionItem::new(&s.name).with_description(format!(
                    "{} - {} tools",
                    s.status.text(),
                    s.tool_count
                ));

                if s.requires_auth {
                    item = item.with_disabled(true, Some("Auth required".to_string()));
                }
                if s.status == McpStatus::Error
                    && let Some(ref err) = s.error
                {
                    item = item.with_description(err.clone());
                }
                item
            })
            .collect();

        Self {
            servers,
            list: SelectionList::new(items).with_searchable(true),
            mode: McpMode::List,
        }
    }

    /// Get the currently selected server
    pub(crate) fn selected_server(&self) -> Option<&McpServerInfo> {
        self.list
            .selected_index()
            .and_then(|idx| self.servers.get(idx))
    }

    /// Build the contextual action bar based on current state
    pub(crate) fn build_action_bar(&self) -> ActionBar {
        let mut bar = ActionBar::new()
            .action('a', "Add")
            .danger('d', "Delete")
            .action('r', "Restart");

        // Context-sensitive Start/Stop
        if let Some(server) = self.selected_server() {
            match server.status {
                McpStatus::Running => bar = bar.action('s', "Stop"),
                McpStatus::Stopped | McpStatus::Error => bar = bar.action('s', "Start"),
                McpStatus::Starting => bar = bar.secondary('s', "Starting..."),
            }
        } else {
            bar = bar.secondary('s', "Start/Stop");
        }

        bar = bar.action('k', "Key").with_standard_hints();
        bar
    }
}
