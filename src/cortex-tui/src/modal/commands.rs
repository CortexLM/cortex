//! Commands Modal
//!
//! A command palette modal (like VS Code Ctrl+K) that displays all available
//! commands with fuzzy search filtering. Commands are grouped by category
//! with section headers for easy navigation.

use cortex_core::style::{BORDER, CYAN_PRIMARY, SURFACE_0, TEXT, TEXT_DIM, TEXT_MUTED, VOID};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use std::collections::BTreeMap;

use crate::modal::render_section_header;
use crate::widgets::{ActionBar, SelectionItem, SelectionList, SelectionResult};

use super::{CancelBehavior, Modal, ModalAction, ModalResult};

// ============================================================================
// COMMAND CATEGORY
// ============================================================================

/// Categories for organizing commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CommandCategory {
    /// Session-related commands (new, export, clear, etc.)
    Session,
    /// Model selection and configuration
    Model,
    /// View and display commands
    View,
    /// Settings and configuration
    Settings,
    /// Help and documentation
    Help,
}

impl CommandCategory {
    /// Returns the display name for the category.
    pub fn display_name(&self) -> &'static str {
        match self {
            CommandCategory::Session => "Session",
            CommandCategory::Model => "Model",
            CommandCategory::View => "View",
            CommandCategory::Settings => "Settings",
            CommandCategory::Help => "Help",
        }
    }
}

// ============================================================================
// COMMAND ENTRY
// ============================================================================

/// A single command entry in the command palette.
#[derive(Debug, Clone)]
pub struct CommandEntry {
    /// Command identifier (e.g., "model", "sessions")
    pub name: String,
    /// Display name shown in the palette (e.g., "Change Model")
    pub display: String,
    /// Optional keyboard shortcut (e.g., "Ctrl+M")
    pub shortcut: Option<String>,
    /// Description of what the command does
    pub description: String,
    /// Category for grouping
    pub category: CommandCategory,
}

impl CommandEntry {
    /// Create a new command entry.
    pub fn new(
        name: impl Into<String>,
        display: impl Into<String>,
        description: impl Into<String>,
        category: CommandCategory,
    ) -> Self {
        Self {
            name: name.into(),
            display: display.into(),
            shortcut: None,
            description: description.into(),
            category,
        }
    }

    /// Set the keyboard shortcut for this command.
    pub fn with_shortcut(mut self, shortcut: impl Into<String>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }
}

// ============================================================================
// COMMANDS MODAL
// ============================================================================

/// A command palette modal for executing commands.
pub struct CommandsModal {
    /// Available commands.
    commands: Vec<CommandEntry>,
    /// Selection list widget.
    list: SelectionList,
    /// Category groups for rendering (category -> list of command indices).
    category_groups: Vec<(CommandCategory, Vec<usize>)>,
    /// Whether to show category headers (false if only one category).
    show_category_headers: bool,
}

impl CommandsModal {
    /// Create a new CommandsModal with default commands.
    pub fn new() -> Self {
        let commands = Self::default_commands();
        Self::with_commands(commands)
    }

    /// Create a CommandsModal with custom commands.
    pub fn with_commands(commands: Vec<CommandEntry>) -> Self {
        // Group commands by category using BTreeMap for consistent ordering
        let mut groups_map: BTreeMap<CommandCategory, Vec<usize>> = BTreeMap::new();
        for (idx, cmd) in commands.iter().enumerate() {
            groups_map.entry(cmd.category).or_default().push(idx);
        }

        // Build the sorted command order to match visual category grouping
        // This ensures arrow key navigation follows the same order as visual display
        let sorted_indices: Vec<usize> = groups_map
            .values()
            .flat_map(|indices| indices.iter().copied())
            .collect();

        // Reorder commands to match visual category order
        let sorted_commands: Vec<CommandEntry> = sorted_indices
            .iter()
            .filter_map(|&idx| commands.get(idx).cloned())
            .collect();

        // Rebuild category groups with new sequential indices
        let mut new_groups_map: BTreeMap<CommandCategory, Vec<usize>> = BTreeMap::new();
        for (idx, cmd) in sorted_commands.iter().enumerate() {
            new_groups_map.entry(cmd.category).or_default().push(idx);
        }
        let category_groups: Vec<(CommandCategory, Vec<usize>)> =
            new_groups_map.into_iter().collect();
        let show_category_headers = category_groups.len() > 1;

        // Convert sorted CommandEntry to SelectionItems
        let items: Vec<SelectionItem> = sorted_commands
            .iter()
            .map(|cmd| {
                // Build description: "Description | Ctrl+M"
                let mut desc_parts = vec![cmd.description.clone()];
                if let Some(shortcut) = &cmd.shortcut {
                    desc_parts.push(shortcut.clone());
                }
                let description = desc_parts.join(" | ");

                SelectionItem::new(&cmd.display).with_description(description)
            })
            .collect();

        let list = SelectionList::new(items)
            .with_searchable(true)
            .with_max_visible(12);

        Self {
            commands: sorted_commands,
            list,
            category_groups,
            show_category_headers,
        }
    }

    /// Get the default set of commands.
    fn default_commands() -> Vec<CommandEntry> {
        vec![
            CommandEntry::new(
                "model",
                "Change Model",
                "Change the AI model",
                CommandCategory::Session,
            )
            .with_shortcut("Ctrl+M"),
            CommandEntry::new(
                "sessions",
                "View Sessions",
                "Browse and restore previous sessions",
                CommandCategory::Session,
            )
            .with_shortcut("Ctrl+S"),
            CommandEntry::new(
                "new",
                "New Session",
                "Start a new conversation session",
                CommandCategory::Session,
            )
            .with_shortcut("Ctrl+N"),
            CommandEntry::new(
                "mcp",
                "Manage MCP Servers",
                "Configure MCP server connections",
                CommandCategory::Settings,
            ),
            CommandEntry::new(
                "settings",
                "Open Settings",
                "Open configuration settings",
                CommandCategory::Settings,
            ),
            CommandEntry::new(
                "theme",
                "Change Theme",
                "Switch between color themes",
                CommandCategory::Settings,
            ),
            CommandEntry::new(
                "export",
                "Export Session",
                "Export the current session to a file",
                CommandCategory::Session,
            ),
            CommandEntry::new(
                "clear",
                "Clear Context",
                "Clear the current conversation context",
                CommandCategory::Session,
            ),
            CommandEntry::new(
                "transcript",
                "View Transcript",
                "View the full conversation transcript",
                CommandCategory::View,
            )
            .with_shortcut("Ctrl+T"),
            CommandEntry::new(
                "help",
                "Show Help",
                "Display help and keyboard shortcuts",
                CommandCategory::Help,
            )
            .with_shortcut("?"),
        ]
    }

    /// Get the currently selected command.
    pub fn selected_command(&self) -> Option<&CommandEntry> {
        self.list
            .selected_index()
            .and_then(|idx| self.commands.get(idx))
    }

    /// Get the number of commands.
    pub fn command_count(&self) -> usize {
        self.commands.len()
    }

    /// Builds the action bar for the modal.
    fn build_action_bar(&self) -> ActionBar {
        ActionBar::new().with_standard_hints()
    }

    /// Renders the search bar.
    fn render_search_bar(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let query = self.list.search_query();
        let icon = "> ";
        let bracket_open = "[";
        let bracket_close = "]";

        let display_text = if query.is_empty() {
            "Type a command..."
        } else {
            query
        };

        let text_style = if query.is_empty() {
            Style::default().fg(TEXT_MUTED)
        } else {
            Style::default().fg(CYAN_PRIMARY)
        };

        // Build the search line
        let mut col = area.x + 1;
        buf.set_string(col, area.y, icon, Style::default());
        col += 3; // icon width

        buf.set_string(col, area.y, bracket_open, Style::default().fg(BORDER));
        col += 1;

        buf.set_string(col, area.y, display_text, text_style);
        col += display_text.len() as u16;

        buf.set_string(col, area.y, bracket_close, Style::default().fg(BORDER));
    }

    /// Renders the commands grouped by category.
    fn render_grouped_commands(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let selected_idx = self.list.selected_index();
        let search_query = self.list.search_query().to_lowercase();
        let mut y = area.y;

        // If searching, use flat list rendering (no grouping)
        if !search_query.is_empty() {
            self.render_flat_filtered_list(area, buf, &search_query, selected_idx);
            return;
        }

        // Render commands grouped by category
        for (category, cmd_indices) in &self.category_groups {
            // Check if we have room
            if y >= area.bottom() {
                break;
            }

            // Render category header if multiple categories
            if self.show_category_headers && y < area.bottom() {
                render_section_header(
                    Rect::new(area.x, y, area.width, 1),
                    buf,
                    category.display_name(),
                );
                y += 1;
            }

            // Render commands in this group
            for &cmd_idx in cmd_indices {
                if y >= area.bottom() {
                    break;
                }

                if let Some(cmd) = self.commands.get(cmd_idx) {
                    let is_selected = selected_idx == Some(cmd_idx);
                    self.render_command_row(area.x, y, area.width, buf, cmd, is_selected);
                    y += 1;
                }
            }
        }

        // Empty state
        if self.commands.is_empty() {
            let msg = "No commands available";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Renders a flat filtered list when searching.
    fn render_flat_filtered_list(
        &self,
        area: Rect,
        buf: &mut Buffer,
        search_query: &str,
        selected_idx: Option<usize>,
    ) {
        let mut y = area.y;

        for (idx, cmd) in self.commands.iter().enumerate() {
            if y >= area.bottom() {
                break;
            }

            // Filter by search query
            if !cmd.display.to_lowercase().contains(search_query)
                && !cmd.name.to_lowercase().contains(search_query)
                && !cmd.description.to_lowercase().contains(search_query)
            {
                continue;
            }

            let is_selected = selected_idx == Some(idx);
            self.render_command_row(area.x, y, area.width, buf, cmd, is_selected);
            y += 1;
        }

        // Empty state for no matches
        if y == area.y {
            let msg = "No matches";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Renders a single command row.
    /// Format: "> /command        Description                  Ctrl+K"
    fn render_command_row(
        &self,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        cmd: &CommandEntry,
        is_selected: bool,
    ) {
        // Determine styles
        let (bg, fg, prefix_fg) = if is_selected {
            (CYAN_PRIMARY, VOID, VOID)
        } else {
            (SURFACE_0, TEXT, CYAN_PRIMARY)
        };

        // Clear the line with background
        for col in x..x.saturating_add(width) {
            buf[(col, y)].set_bg(bg);
        }

        let mut col = x;

        // Selection prefix: ">" for selected, " " for others
        let prefix = if is_selected { ">" } else { " " };
        buf.set_string(col, y, prefix, Style::default().fg(prefix_fg).bg(bg));
        col += 2;

        // Command name with "/" prefix
        let cmd_name = format!("/{}", cmd.name);
        let name_style = Style::default().fg(fg).bg(bg);
        buf.set_string(col, y, &cmd_name, name_style);
        col += cmd_name.len() as u16 + 2;

        // Description
        let desc_style = if is_selected {
            Style::default().fg(VOID).bg(bg)
        } else {
            Style::default().fg(TEXT_DIM).bg(bg)
        };

        // Calculate available space for description
        let shortcut_str = cmd.shortcut.as_deref().unwrap_or("");
        let shortcut_len = shortcut_str.len() as u16;
        let right_padding = if shortcut_len > 0 {
            shortcut_len + 4
        } else {
            2
        };
        let max_desc_len = width.saturating_sub(col - x).saturating_sub(right_padding) as usize;

        // Truncate description if needed
        let truncated_desc = if cmd.description.len() > max_desc_len && max_desc_len > 3 {
            format!("{}...", &cmd.description[..max_desc_len.saturating_sub(3)])
        } else {
            cmd.description.clone()
        };

        buf.set_string(col, y, &truncated_desc, desc_style);

        // Shortcut (right-aligned)
        if !shortcut_str.is_empty() {
            let shortcut_x = x + width.saturating_sub(shortcut_len + 2);
            let shortcut_style = if is_selected {
                Style::default().fg(VOID).bg(bg)
            } else {
                Style::default().fg(TEXT_DIM).bg(bg)
            };
            buf.set_string(shortcut_x, y, shortcut_str, shortcut_style);
        }
    }
}

impl Default for CommandsModal {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// MODAL IMPLEMENTATION
// ============================================================================

impl Modal for CommandsModal {
    fn title(&self) -> &str {
        "Commands"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Base height for list items + search bar + action bar + category headers
        let command_count = self.commands.len() as u16;
        let header_count = if self.show_category_headers {
            self.category_groups.len() as u16
        } else {
            0
        };
        let content_height = command_count + header_count + 3; // +3 for search bar, action bar, padding

        // Clamp between min 6 and max 14, respecting max_height
        content_height.clamp(6, 14).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        // Layout: search bar at top, commands in middle, action bar at bottom
        let search_height = 1u16;
        let action_bar_height = 1u16;
        let content_height = area
            .height
            .saturating_sub(search_height + action_bar_height);

        let search_area = Rect::new(area.x, area.y, area.width, search_height);
        let content_area = Rect::new(area.x, area.y + search_height, area.width, content_height);
        let action_area = Rect::new(
            area.x,
            area.y + search_height + content_height,
            area.width,
            action_bar_height,
        );

        // Render search bar
        self.render_search_bar(search_area, buf);

        // Render grouped commands
        self.render_grouped_commands(content_area, buf);

        // Render action bar
        let action_bar = self.build_action_bar();
        (&action_bar).render(action_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match key.code {
            KeyCode::Esc => {
                // Check if we should handle escape internally (e.g., clear search)
                match self.on_cancel() {
                    CancelBehavior::Handled => ModalResult::Continue,
                    CancelBehavior::Close => ModalResult::Close,
                }
            }
            KeyCode::Enter => {
                // First let the list handle it to get the selection
                if let SelectionResult::Selected(idx) = self.list.handle_key(key)
                    && let Some(cmd) = self.commands.get(idx)
                {
                    return ModalResult::Action(ModalAction::ExecuteCommand(cmd.name.clone()));
                }
                ModalResult::Continue
            }
            _ => {
                // Let the list handle navigation and search
                match self.list.handle_key(key) {
                    SelectionResult::Selected(idx) => {
                        if let Some(cmd) = self.commands.get(idx) {
                            ModalResult::Action(ModalAction::ExecuteCommand(cmd.name.clone()))
                        } else {
                            ModalResult::Continue
                        }
                    }
                    SelectionResult::Cancelled => ModalResult::Close,
                    SelectionResult::None => ModalResult::Continue,
                }
            }
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("\u{2191}\u{2193}", "navigate"),
            ("Enter", "run"),
            ("Esc", "close"),
            ("type", "filter"),
        ]
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        // If there's an active search, clear it first
        if !self.list.search_query().is_empty() {
            // Clear the search by sending Ctrl+U
            let clear_key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
            self.list.handle_key(clear_key);
            CancelBehavior::Handled
        } else {
            CancelBehavior::Close
        }
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Type a command...")
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_default_commands() {
        let modal = CommandsModal::new();
        assert_eq!(modal.commands.len(), 10);
        assert_eq!(modal.commands[0].name, "model");
        assert_eq!(modal.commands[0].display, "Change Model");
    }

    #[test]
    fn test_with_commands() {
        let commands = vec![CommandEntry::new(
            "test",
            "Test Command",
            "A test command",
            CommandCategory::Help,
        )];
        let modal = CommandsModal::with_commands(commands);
        assert_eq!(modal.commands.len(), 1);
        assert_eq!(modal.commands[0].name, "test");
    }

    #[test]
    fn test_title() {
        let modal = CommandsModal::new();
        assert_eq!(modal.title(), "Commands");
    }

    #[test]
    fn test_is_searchable() {
        let modal = CommandsModal::new();
        assert!(modal.is_searchable());
    }

    #[test]
    fn test_search_placeholder() {
        let modal = CommandsModal::new();
        assert_eq!(modal.search_placeholder(), Some("Type a command..."));
    }

    #[test]
    fn test_desired_height() {
        let modal = CommandsModal::new();

        // With 10 commands + 2 padding = 12, clamped to max 14
        let height = modal.desired_height(20, 80);
        assert!(height >= 5);
        assert!(height <= 14);
    }

    #[test]
    fn test_key_hints() {
        let modal = CommandsModal::new();
        let hints = modal.key_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
        assert!(hints.iter().any(|(_, v)| *v == "filter"));
    }

    #[test]
    fn test_escape_closes() {
        let mut modal = CommandsModal::new();

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_enter_executes_command() {
        let mut modal = CommandsModal::new();

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        // Should execute the first command (model)
        if let ModalResult::Action(ModalAction::ExecuteCommand(name)) = result {
            assert_eq!(name, "model");
        } else {
            panic!("Expected ExecuteCommand action");
        }
    }

    #[test]
    fn test_navigation_and_select() {
        let mut modal = CommandsModal::new();

        // Move down once
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        modal.handle_key(down);

        // Select (should be "sessions" now, index 1)
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(enter);

        if let ModalResult::Action(ModalAction::ExecuteCommand(name)) = result {
            assert_eq!(name, "sessions");
        } else {
            panic!("Expected ExecuteCommand action");
        }
    }

    #[test]
    fn test_command_entry_with_shortcut() {
        let cmd = CommandEntry::new("test", "Test", "Description", CommandCategory::Help)
            .with_shortcut("Ctrl+T");

        assert_eq!(cmd.shortcut, Some("Ctrl+T".to_string()));
    }

    #[test]
    fn test_command_category_display_name() {
        assert_eq!(CommandCategory::Session.display_name(), "Session");
        assert_eq!(CommandCategory::Model.display_name(), "Model");
        assert_eq!(CommandCategory::View.display_name(), "View");
        assert_eq!(CommandCategory::Settings.display_name(), "Settings");
        assert_eq!(CommandCategory::Help.display_name(), "Help");
    }

    #[test]
    fn test_selected_command() {
        let modal = CommandsModal::new();
        let cmd = modal.selected_command().unwrap();
        assert_eq!(cmd.name, "model");
    }

    #[test]
    fn test_default_impl() {
        let modal = CommandsModal::default();
        assert_eq!(modal.commands.len(), 10);
    }

    #[test]
    fn test_command_count() {
        let modal = CommandsModal::new();
        assert_eq!(modal.command_count(), 10);
    }

    #[test]
    fn test_on_cancel_clears_search() {
        let mut modal = CommandsModal::new();

        // Type something to set up a search query
        let key_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        modal.handle_key(key_x);
        assert!(!modal.list.search_query().is_empty());

        // on_cancel should clear it
        let behavior = modal.on_cancel();
        assert_eq!(behavior, CancelBehavior::Handled);
        assert!(modal.list.search_query().is_empty());
    }

    #[test]
    fn test_on_cancel_closes_when_no_search() {
        let mut modal = CommandsModal::new();

        // No search query, should return Close
        let behavior = modal.on_cancel();
        assert_eq!(behavior, CancelBehavior::Close);
    }

    #[test]
    fn test_category_grouping_multiple_categories() {
        let modal = CommandsModal::new();

        // Default commands span multiple categories
        assert!(modal.show_category_headers);
        assert!(!modal.category_groups.is_empty());
    }

    #[test]
    fn test_category_grouping_single_category() {
        let commands = vec![
            CommandEntry::new(
                "cmd1",
                "Command 1",
                "Description 1",
                CommandCategory::Session,
            ),
            CommandEntry::new(
                "cmd2",
                "Command 2",
                "Description 2",
                CommandCategory::Session,
            ),
        ];
        let modal = CommandsModal::with_commands(commands);

        // Should not show headers when only one category
        assert!(!modal.show_category_headers);
        assert_eq!(modal.category_groups.len(), 1);
    }

    #[test]
    fn test_category_groups_contain_correct_indices() {
        let modal = CommandsModal::new();

        // Find the Session group
        let session_group = modal
            .category_groups
            .iter()
            .find(|(cat, _)| *cat == CommandCategory::Session);
        assert!(session_group.is_some());
        let (_, indices) = session_group.unwrap();
        // Session should have multiple commands
        assert!(!indices.is_empty());
    }

    #[test]
    fn test_build_action_bar() {
        let modal = CommandsModal::new();

        // Should build action bar with standard hints
        let _action_bar = modal.build_action_bar();
        // ActionBar is created successfully (basic smoke test)
    }

    #[test]
    fn test_escape_clears_search_first() {
        let mut modal = CommandsModal::new();

        // Type something to create a search query
        let char_key = KeyEvent::new(KeyCode::Char('m'), KeyModifiers::NONE);
        modal.handle_key(char_key);

        // First escape should clear search, not close
        let esc_key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = modal.handle_key(esc_key);
        assert!(matches!(result, ModalResult::Continue));

        // Second escape should close
        let result = modal.handle_key(esc_key);
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_desired_height_with_categories() {
        let modal = CommandsModal::new();

        // With categories, height should account for headers
        let height = modal.desired_height(20, 80);
        assert!(height >= 6); // min 6 with new layout
        assert!(height <= 14);
    }

    #[test]
    fn test_navigation_matches_visual_category_order() {
        // Create commands across multiple categories in non-sorted order
        let commands = vec![
            CommandEntry::new(
                "settings",
                "Settings",
                "Open settings",
                CommandCategory::Settings,
            ),
            CommandEntry::new("model", "Model", "Change model", CommandCategory::Model),
            CommandEntry::new("help", "Help", "Show help", CommandCategory::Help),
            CommandEntry::new(
                "new",
                "New Session",
                "Start new session",
                CommandCategory::Session,
            ),
            CommandEntry::new(
                "export",
                "Export",
                "Export session",
                CommandCategory::Session,
            ),
        ];
        let mut modal = CommandsModal::with_commands(commands);

        // After sorting by category (BTreeMap order: Session, Model, View, Settings, Help),
        // the order should be: new, export (Session), model (Model), settings (Settings), help (Help)

        // First command should be "new" (first Session command)
        let first = modal.selected_command().unwrap();
        assert_eq!(
            first.name, "new",
            "First command should be 'new' (Session category)"
        );

        // Navigate down to second command
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        modal.handle_key(down);
        let second = modal.selected_command().unwrap();
        assert_eq!(
            second.name, "export",
            "Second command should be 'export' (Session category)"
        );

        // Navigate down to third command
        modal.handle_key(down);
        let third = modal.selected_command().unwrap();
        assert_eq!(
            third.name, "model",
            "Third command should be 'model' (Model category)"
        );

        // Navigate down to fourth command
        modal.handle_key(down);
        let fourth = modal.selected_command().unwrap();
        assert_eq!(
            fourth.name, "settings",
            "Fourth command should be 'settings' (Settings category)"
        );

        // Navigate down to fifth command
        modal.handle_key(down);
        let fifth = modal.selected_command().unwrap();
        assert_eq!(
            fifth.name, "help",
            "Fifth command should be 'help' (Help category)"
        );
    }
}
