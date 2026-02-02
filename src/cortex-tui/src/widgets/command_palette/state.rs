//! Command palette state management.
//!
//! Contains the state struct and all methods for managing command palette behavior.

use crate::app::SessionSummary;
use crate::commands::CommandRegistry;

use super::fuzzy::fuzzy_score;
use super::types::PaletteItem;

/// Returns the keyboard shortcut for a command, if known.
fn get_command_shortcut(name: &str) -> Option<String> {
    match name {
        "help" => Some("[?]".to_string()),
        "settings" => Some("[Ctrl+,]".to_string()),
        "model" => Some("[Ctrl+M]".to_string()),
        "palette" => Some("[Ctrl+P]".to_string()),
        "quit" => Some("[Ctrl+Q]".to_string()),
        "new" => Some("[Ctrl+N]".to_string()),
        "clear" => Some("[Ctrl+L]".to_string()),
        _ => None,
    }
}

/// State for the command palette widget.
pub struct CommandPaletteState {
    /// Current search query
    pub query: String,
    /// Cursor position within query
    pub cursor_pos: usize,
    /// Currently selected item index
    pub selected_index: usize,
    /// Scroll offset for long lists
    pub scroll_offset: usize,
    /// All available items
    pub items: Vec<PaletteItem>,
    /// Filtered items with scores: (original_index, score)
    pub filtered_items: Vec<(usize, i32)>,
    /// Recently used items
    pub recent: Vec<PaletteItem>,
    /// Number of visible items in the current view
    pub visible_count: usize,
}

impl Default for CommandPaletteState {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandPaletteState {
    /// Creates a new command palette state.
    pub fn new() -> Self {
        Self {
            query: String::new(),
            cursor_pos: 0,
            selected_index: 0,
            scroll_offset: 0,
            items: Vec::new(),
            filtered_items: Vec::new(),
            recent: Vec::new(),
            visible_count: 10,
        }
    }

    /// Updates filtered items based on the current query.
    pub fn filter(&mut self) {
        self.filtered_items.clear();

        if self.query.is_empty() {
            // Show all items with default score, sorted by category
            let mut items_with_indices: Vec<(usize, i32, u8)> = self
                .items
                .iter()
                .enumerate()
                .map(|(idx, item)| (idx, 0i32, item.sort_key()))
                .collect();

            items_with_indices.sort_by_key(|(_, _, sort_key)| *sort_key);

            self.filtered_items = items_with_indices
                .into_iter()
                .map(|(idx, score, _)| (idx, score))
                .collect();
        } else {
            // Fuzzy match against query
            let query_lower = self.query.to_lowercase();

            for (idx, item) in self.items.iter().enumerate() {
                let text = item.display_text().to_lowercase();
                if let Some(score) = fuzzy_score(&query_lower, &text) {
                    self.filtered_items.push((idx, score));
                }
            }

            // Sort by score descending
            self.filtered_items.sort_by(|a, b| b.1.cmp(&a.1));
        }

        // Reset selection to top
        self.selected_index = 0;
        self.scroll_offset = 0;
    }

    /// Inserts a character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        self.query.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
        self.filter();
    }

    /// Deletes the character before the cursor (backspace).
    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            // Find the previous character boundary
            let prev_pos = self.query[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);

            self.query.remove(prev_pos);
            self.cursor_pos = prev_pos;
            self.filter();
        }
    }

    /// Deletes the character at the cursor (delete key).
    pub fn delete(&mut self) {
        if self.cursor_pos < self.query.len() {
            self.query.remove(self.cursor_pos);
            self.filter();
        }
    }

    /// Moves the cursor left.
    pub fn cursor_left(&mut self) {
        if self.cursor_pos > 0 {
            // Find the previous character boundary
            self.cursor_pos = self.query[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    /// Moves the cursor right.
    pub fn cursor_right(&mut self) {
        if self.cursor_pos < self.query.len() {
            // Find the next character boundary
            self.cursor_pos = self.query[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor_pos + i)
                .unwrap_or(self.query.len());
        }
    }

    /// Moves the cursor to the start.
    pub fn cursor_home(&mut self) {
        self.cursor_pos = 0;
    }

    /// Moves the cursor to the end.
    pub fn cursor_end(&mut self) {
        self.cursor_pos = self.query.len();
    }

    /// Moves selection up with wrap-around.
    pub fn select_prev(&mut self) {
        let total = self.total_display_count();
        if total == 0 {
            return;
        }
        if self.selected_index > 0 {
            self.selected_index -= 1;
            // Adjust scroll if needed
            if self.selected_index < self.scroll_offset {
                self.scroll_offset = self.selected_index;
            }
        } else {
            // Wrap to last item
            self.selected_index = total - 1;
            // Scroll to show last items
            if total > self.visible_count {
                self.scroll_offset = total - self.visible_count;
            }
        }
    }

    /// Moves selection down with wrap-around.
    pub fn select_next(&mut self) {
        let total = self.total_display_count();
        if total == 0 {
            return;
        }
        if self.selected_index < total.saturating_sub(1) {
            self.selected_index += 1;
            // Adjust scroll if needed
            if self.selected_index >= self.scroll_offset + self.visible_count {
                self.scroll_offset = self.selected_index.saturating_sub(self.visible_count - 1);
            }
        } else {
            // Wrap to first item
            self.selected_index = 0;
            self.scroll_offset = 0;
        }
    }

    /// Returns the total number of displayable items.
    pub(super) fn total_display_count(&self) -> usize {
        if self.query.is_empty() {
            self.recent.len().min(3) + self.filtered_items.len()
        } else {
            self.filtered_items.len()
        }
    }

    /// Returns the currently selected item.
    pub fn selected(&self) -> Option<&PaletteItem> {
        if self.query.is_empty() {
            let recent_count = self.recent.len().min(3);
            if self.selected_index < recent_count {
                return self.recent.get(self.selected_index);
            }

            let filtered_idx = self.selected_index - recent_count;
            self.filtered_items
                .get(filtered_idx)
                .map(|(idx, _)| &self.items[*idx])
        } else {
            self.filtered_items
                .get(self.selected_index)
                .map(|(idx, _)| &self.items[*idx])
        }
    }

    /// Adds an item to the recent list.
    pub fn add_recent(&mut self, item: PaletteItem) {
        // Remove duplicates
        let display_text = item.display_text().to_string();
        self.recent.retain(|r| r.display_text() != display_text);

        // Add to front
        self.recent.insert(0, item);

        // Keep only last 10
        self.recent.truncate(10);
    }

    /// Loads commands from a registry.
    pub fn load_commands(&mut self, registry: &CommandRegistry) {
        // Clear existing command items
        self.items
            .retain(|item| !matches!(item, PaletteItem::Command { .. }));

        // Add all visible commands
        for cmd in registry.all() {
            let shortcut = get_command_shortcut(cmd.name);
            self.items.push(PaletteItem::Command {
                name: cmd.name.to_string(),
                description: cmd.description.to_string(),
                shortcut,
                category: cmd.category,
            });
        }

        // Re-filter
        self.filter();
    }

    /// Loads sessions into the palette.
    pub fn load_sessions(&mut self, sessions: &[SessionSummary]) {
        // Clear existing session items
        self.items
            .retain(|item| !matches!(item, PaletteItem::Session { .. }));

        // Add sessions
        for session in sessions {
            self.items.push(PaletteItem::Session {
                id: session.id.to_string(),
                title: session.title.clone(),
                relative_time: session.relative_time(),
            });
        }

        // Re-filter
        self.filter();
    }

    /// Adds a file to the palette.
    pub fn add_file(&mut self, path: String, line: Option<usize>) {
        self.items.push(PaletteItem::File { path, line });
        self.filter();
    }

    /// Clears the query and resets selection.
    pub fn clear(&mut self) {
        self.query.clear();
        self.cursor_pos = 0;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.filter();
    }

    /// Resets the entire state.
    pub fn reset(&mut self) {
        self.clear();
        self.items.clear();
        self.filtered_items.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::CommandCategory;
    use crate::widgets::command_palette::types::RecentType;

    #[test]
    fn test_state_new() {
        let state = CommandPaletteState::new();
        assert!(state.query.is_empty());
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.selected_index, 0);
        assert!(state.items.is_empty());
        assert!(state.filtered_items.is_empty());
        assert!(state.recent.is_empty());
    }

    #[test]
    fn test_state_insert_char() {
        let mut state = CommandPaletteState::new();
        state.insert_char('h');
        state.insert_char('e');
        state.insert_char('l');
        state.insert_char('p');

        assert_eq!(state.query, "help");
        assert_eq!(state.cursor_pos, 4);
    }

    #[test]
    fn test_state_backspace() {
        let mut state = CommandPaletteState::new();
        state.query = "help".to_string();
        state.cursor_pos = 4;

        state.backspace();
        assert_eq!(state.query, "hel");
        assert_eq!(state.cursor_pos, 3);

        state.backspace();
        assert_eq!(state.query, "he");
        assert_eq!(state.cursor_pos, 2);
    }

    #[test]
    fn test_state_backspace_at_start() {
        let mut state = CommandPaletteState::new();
        state.query = "help".to_string();
        state.cursor_pos = 0;

        state.backspace();
        assert_eq!(state.query, "help");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_state_delete() {
        let mut state = CommandPaletteState::new();
        state.query = "help".to_string();
        state.cursor_pos = 0;

        state.delete();
        assert_eq!(state.query, "elp");
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_state_cursor_movement() {
        let mut state = CommandPaletteState::new();
        state.query = "help".to_string();
        state.cursor_pos = 2;

        state.cursor_left();
        assert_eq!(state.cursor_pos, 1);

        state.cursor_left();
        assert_eq!(state.cursor_pos, 0);

        state.cursor_left();
        assert_eq!(state.cursor_pos, 0); // Can't go below 0

        state.cursor_right();
        assert_eq!(state.cursor_pos, 1);

        state.cursor_end();
        assert_eq!(state.cursor_pos, 4);

        state.cursor_home();
        assert_eq!(state.cursor_pos, 0);
    }

    #[test]
    fn test_state_select_navigation() {
        let mut state = CommandPaletteState::new();
        state.items = vec![
            PaletteItem::Command {
                name: "help".to_string(),
                description: "Help".to_string(),
                shortcut: None,
                category: CommandCategory::General,
            },
            PaletteItem::Command {
                name: "quit".to_string(),
                description: "Quit".to_string(),
                shortcut: None,
                category: CommandCategory::General,
            },
            PaletteItem::Command {
                name: "model".to_string(),
                description: "Model".to_string(),
                shortcut: None,
                category: CommandCategory::Model,
            },
        ];
        state.filter();
        let total = state.filtered_items.len();

        assert_eq!(state.selected_index, 0);

        state.select_next();
        assert_eq!(state.selected_index, 1);

        state.select_next();
        assert_eq!(state.selected_index, 2);

        // Wrap around to first when going beyond last
        state.select_next();
        assert_eq!(state.selected_index, 0);

        state.select_prev();
        assert_eq!(state.selected_index, total - 1); // Wrap to last

        state.select_prev();
        assert_eq!(state.selected_index, total - 2);
    }

    #[test]
    fn test_state_filter() {
        let mut state = CommandPaletteState::new();
        state.items = vec![
            PaletteItem::Command {
                name: "help".to_string(),
                description: "Show help".to_string(),
                shortcut: None,
                category: CommandCategory::General,
            },
            PaletteItem::Command {
                name: "history".to_string(),
                description: "Show history".to_string(),
                shortcut: None,
                category: CommandCategory::Navigation,
            },
            PaletteItem::Command {
                name: "model".to_string(),
                description: "Switch model".to_string(),
                shortcut: None,
                category: CommandCategory::Model,
            },
        ];

        // Empty query shows all
        state.filter();
        assert_eq!(state.filtered_items.len(), 3);

        // Filter by "h" matches "help" and "history"
        state.query = "h".to_string();
        state.filter();
        assert_eq!(state.filtered_items.len(), 2);

        // Filter by "mod" matches only "model"
        state.query = "mod".to_string();
        state.filter();
        assert_eq!(state.filtered_items.len(), 1);

        // Filter by "xyz" matches nothing
        state.query = "xyz".to_string();
        state.filter();
        assert_eq!(state.filtered_items.len(), 0);
    }

    #[test]
    fn test_state_add_recent() {
        let mut state = CommandPaletteState::new();

        let item1 = PaletteItem::Recent {
            text: "help".to_string(),
            item_type: RecentType::Command,
        };
        state.add_recent(item1);
        assert_eq!(state.recent.len(), 1);

        let item2 = PaletteItem::Recent {
            text: "model".to_string(),
            item_type: RecentType::Command,
        };
        state.add_recent(item2);
        assert_eq!(state.recent.len(), 2);

        // Most recent should be first
        assert_eq!(state.recent[0].display_text(), "model");
        assert_eq!(state.recent[1].display_text(), "help");

        // Adding duplicate should move to front
        let item3 = PaletteItem::Recent {
            text: "help".to_string(),
            item_type: RecentType::Command,
        };
        state.add_recent(item3);
        assert_eq!(state.recent.len(), 2);
        assert_eq!(state.recent[0].display_text(), "help");
    }

    #[test]
    fn test_state_clear() {
        let mut state = CommandPaletteState::new();
        state.query = "test".to_string();
        state.cursor_pos = 4;
        state.selected_index = 5;
        state.scroll_offset = 2;

        state.clear();

        assert!(state.query.is_empty());
        assert_eq!(state.cursor_pos, 0);
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.scroll_offset, 0);
    }

    #[test]
    fn test_state_selected() {
        let mut state = CommandPaletteState::new();
        state.items = vec![
            PaletteItem::Command {
                name: "help".to_string(),
                description: "Help".to_string(),
                shortcut: None,
                category: CommandCategory::General,
            },
            PaletteItem::Command {
                name: "quit".to_string(),
                description: "Quit".to_string(),
                shortcut: None,
                category: CommandCategory::General,
            },
        ];
        state.filter();

        assert_eq!(state.selected().unwrap().display_text(), "help");

        state.selected_index = 1;
        assert_eq!(state.selected().unwrap().display_text(), "quit");
    }

    #[test]
    fn test_command_shortcuts() {
        assert_eq!(get_command_shortcut("help"), Some("[?]".to_string()));
        assert_eq!(
            get_command_shortcut("settings"),
            Some("[Ctrl+,]".to_string())
        );
        assert_eq!(get_command_shortcut("model"), Some("[Ctrl+M]".to_string()));
        assert_eq!(get_command_shortcut("unknown"), None);
    }
}
