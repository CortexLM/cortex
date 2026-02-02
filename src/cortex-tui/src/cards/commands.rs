//! Commands Card
//!
//! A command palette card (like VS Code Ctrl+K) that displays all available
//! commands with fuzzy search filtering.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::cards::{CancellationEvent, CardAction, CardResult, CardView};
use crate::widgets::{SelectionItem, SelectionList, SelectionResult};

// ============================================================
// COMMAND CATEGORY
// ============================================================

/// Categories for organizing commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

// ============================================================
// COMMAND ENTRY
// ============================================================

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

// ============================================================
// COMMANDS CARD
// ============================================================

/// A command palette card for executing commands.
pub struct CommandsCard {
    /// Available commands.
    commands: Vec<CommandEntry>,
    /// Selection list widget.
    list: SelectionList,
    /// Whether a selection has been made.
    completed: bool,
}

impl CommandsCard {
    /// Create a new CommandsCard with default commands.
    pub fn new() -> Self {
        let commands = Self::default_commands();
        Self::with_commands(commands)
    }

    /// Create a CommandsCard with custom commands.
    pub fn with_commands(commands: Vec<CommandEntry>) -> Self {
        // Convert CommandEntry to SelectionItems
        let items: Vec<SelectionItem> = commands
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
            commands,
            list,
            completed: false,
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
}

impl Default for CommandsCard {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// CARDVIEW IMPLEMENTATION
// ============================================================

impl CardView for CommandsCard {
    fn title(&self) -> &str {
        "Commands"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Base height for list items + search bar + some padding
        let command_count = self.commands.len() as u16;
        let content_height = command_count + 2; // +2 for search bar and padding

        // Clamp between min 5 and max 14, respecting max_height
        content_height.clamp(5, 14).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Render the selection list
        (&self.list).render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> CardResult {
        match key.code {
            KeyCode::Esc => CardResult::Close,
            KeyCode::Enter => {
                // First let the list handle it to get the selection
                if let SelectionResult::Selected(idx) = self.list.handle_key(key)
                    && let Some(cmd) = self.commands.get(idx)
                {
                    self.completed = true;
                    return CardResult::Action(CardAction::ExecuteCommand(cmd.name.clone()));
                }
                CardResult::Continue
            }
            _ => {
                // Let the list handle navigation and search
                match self.list.handle_key(key) {
                    SelectionResult::Selected(idx) => {
                        if let Some(cmd) = self.commands.get(idx) {
                            self.completed = true;
                            CardResult::Action(CardAction::ExecuteCommand(cmd.name.clone()))
                        } else {
                            CardResult::Continue
                        }
                    }
                    SelectionResult::Cancelled => CardResult::Close,
                    SelectionResult::None => CardResult::Continue,
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

    fn on_cancel(&mut self) -> CancellationEvent {
        // If there's an active search, clear it first
        if !self.list.search_query().is_empty() {
            // Clear the search by sending Ctrl+U
            let clear_key =
                KeyEvent::new(KeyCode::Char('u'), crossterm::event::KeyModifiers::CONTROL);
            self.list.handle_key(clear_key);
            CancellationEvent::Handled
        } else {
            CancellationEvent::NotHandled
        }
    }

    fn is_complete(&self) -> bool {
        self.completed
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Type a command...")
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

    #[test]
    fn test_new_creates_default_commands() {
        let card = CommandsCard::new();
        assert_eq!(card.commands.len(), 9);
        assert_eq!(card.commands[0].name, "model");
        assert_eq!(card.commands[0].display, "Change Model");
    }

    #[test]
    fn test_with_commands() {
        let commands = vec![CommandEntry::new(
            "test",
            "Test Command",
            "A test command",
            CommandCategory::Help,
        )];
        let card = CommandsCard::with_commands(commands);
        assert_eq!(card.commands.len(), 1);
        assert_eq!(card.commands[0].name, "test");
    }

    #[test]
    fn test_title() {
        let card = CommandsCard::new();
        assert_eq!(card.title(), "Commands");
    }

    #[test]
    fn test_is_searchable() {
        let card = CommandsCard::new();
        assert!(card.is_searchable());
    }

    #[test]
    fn test_search_placeholder() {
        let card = CommandsCard::new();
        assert_eq!(card.search_placeholder(), Some("Type a command..."));
    }

    #[test]
    fn test_desired_height() {
        let card = CommandsCard::new();

        // With 10 commands + 2 padding = 12, clamped to max 14
        let height = card.desired_height(20, 80);
        assert!(height >= 5);
        assert!(height <= 14);
    }

    #[test]
    fn test_key_hints() {
        let card = CommandsCard::new();
        let hints = card.key_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
        assert!(hints.iter().any(|(_, v)| *v == "filter"));
    }

    #[test]
    fn test_escape_closes() {
        let mut card = CommandsCard::new();

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Close));
    }

    #[test]
    fn test_enter_executes_command() {
        let mut card = CommandsCard::new();

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = card.handle_key(key);

        // Should execute the first command (model)
        if let CardResult::Action(CardAction::ExecuteCommand(name)) = result {
            assert_eq!(name, "model");
        } else {
            panic!("Expected ExecuteCommand action");
        }
    }

    #[test]
    fn test_navigation_and_select() {
        let mut card = CommandsCard::new();

        // Move down once
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        card.handle_key(down);

        // Select (should be "sessions" now, index 1)
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = card.handle_key(enter);

        if let CardResult::Action(CardAction::ExecuteCommand(name)) = result {
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
        let card = CommandsCard::new();
        let cmd = card.selected_command().unwrap();
        assert_eq!(cmd.name, "model");
    }

    #[test]
    fn test_default_impl() {
        let card = CommandsCard::default();
        assert_eq!(card.commands.len(), 9);
    }

    #[test]
    fn test_is_complete() {
        let mut card = CommandsCard::new();
        assert!(!card.is_complete());

        // Execute a command
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        card.handle_key(enter);

        assert!(card.is_complete());
    }
}
