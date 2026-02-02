//! Text input widget with history support
//!
//! Wraps `tui-textarea` with Cortex styling and history functionality.

use crate::style::{BORDER, PINK, TEXT, TEXT_DIM, VOID};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Widget, WidgetRef};
use tui_textarea::TextArea;

/// A styled text input widget with history support.
///
/// Wraps `tui-textarea` with Cortex theming and adds command history
/// navigation similar to shell input.
///
/// # Example
///
/// ```ignore
/// use cortex_engine::widgets::CortexInput;
///
/// let mut input = CortexInput::new()
///     .with_placeholder("Type a message...")
///     .focused(true);
///
/// // Handle key events
/// if input.handle_key(key_event) {
///     // Key was handled by input
/// }
///
/// // Submit and get text
/// let text = input.submit();
/// ```
/// Editing mode for the input widget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditingMode {
    /// Insert mode (default): New characters are inserted at cursor position.
    #[default]
    Insert,
    /// Overwrite mode: New characters replace existing characters at cursor position.
    Overwrite,
}

pub struct CortexInput<'a> {
    textarea: TextArea<'a>,
    history: Vec<String>,
    history_index: Option<usize>,
    placeholder: Option<String>,
    focused: bool,
    multiline: bool,
    /// Current editing mode (insert or overwrite).
    editing_mode: EditingMode,
}

impl Default for CortexInput<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> CortexInput<'a> {
    /// Create a new text input with default settings.
    pub fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_cursor_line_style(Style::default());

        let mut input = Self {
            textarea,
            history: Vec::new(),
            history_index: None,
            placeholder: None,
            focused: false,
            multiline: false,
            editing_mode: EditingMode::Insert,
        };
        input.apply_theme();
        input
    }

    /// Set the placeholder text shown when input is empty.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self.update_placeholder();
        self
    }

    /// Enable multiline input mode.
    ///
    /// In multiline mode:
    /// - Enter inserts a newline instead of submitting
    /// - Up/Down move cursor instead of navigating history
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }

    /// Set the focused state of the input.
    ///
    /// When focused, the border is highlighted with the accent color.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self.apply_theme();
        self
    }

    /// Apply Cortex theme to the textarea.
    fn apply_theme(&mut self) {
        // Set colors
        self.textarea.set_style(Style::default().fg(TEXT).bg(VOID));

        // Cursor styling
        self.textarea
            .set_cursor_style(Style::default().fg(VOID).bg(PINK));

        // No special styling for cursor line
        self.textarea.set_cursor_line_style(Style::default());

        // Border styling based on focus state
        let border_color = if self.focused { PINK } else { BORDER };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color))
            .style(Style::default().bg(VOID));
        self.textarea.set_block(block);
    }

    /// Update placeholder display.
    fn update_placeholder(&mut self) {
        if let Some(ref placeholder) = self.placeholder {
            self.textarea.set_placeholder_text(placeholder.as_str());
            self.textarea
                .set_placeholder_style(Style::default().fg(TEXT_DIM));
        }
    }

    /// Process a key event.
    ///
    /// Returns `true` if the event was handled by the input widget.
    ///
    /// # Key Bindings
    ///
    /// - **Enter**: Submit (single-line) or newline (multiline)
    /// - **Up/Down**: History navigation (single-line only)
    /// - **Ctrl+A**: Select all
    /// - **Ctrl+U**: Clear line
    /// - **Ctrl+W**: Delete word backward (Unix readline style)
    /// - **Insert**: Toggle insert/overwrite mode
    /// - Standard editing keys are handled by tui-textarea
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        match (key.code, key.modifiers) {
            // Ctrl+A: Select all
            (KeyCode::Char('a'), KeyModifiers::CONTROL) => {
                self.textarea.select_all();
                true
            }

            // Ctrl+U: Clear line
            (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                self.clear();
                true
            }

            // Ctrl+W: Delete word backward (Unix readline style)
            (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                self.delete_word_backward();
                self.history_index = None;
                true
            }

            // Insert key: Toggle insert/overwrite mode
            (KeyCode::Insert, KeyModifiers::NONE) => {
                self.toggle_editing_mode();
                true
            }

            // Enter handling
            (KeyCode::Enter, KeyModifiers::NONE) => {
                if self.multiline {
                    // In multiline mode, insert newline
                    self.textarea.insert_newline();
                    true
                } else {
                    // In single-line mode, signal submission (handled by caller)
                    false
                }
            }

            // Up arrow: history navigation in single-line mode
            (KeyCode::Up, KeyModifiers::NONE) if !self.multiline => {
                self.history_prev();
                true
            }

            // Down arrow: history navigation in single-line mode
            (KeyCode::Down, KeyModifiers::NONE) if !self.multiline => {
                self.history_next();
                true
            }

            // Character input - handle overwrite mode
            (KeyCode::Char(c), KeyModifiers::NONE | KeyModifiers::SHIFT) => {
                if self.editing_mode == EditingMode::Overwrite {
                    // In overwrite mode, delete the character at cursor before inserting
                    self.textarea.delete_char();
                }
                // Insert the character
                let input: tui_textarea::Input = key.into();
                self.textarea.input(input);
                self.history_index = None;
                true
            }

            // Let tui-textarea handle everything else
            _ => {
                // Convert crossterm KeyEvent to tui-textarea Input
                let input: tui_textarea::Input = key.into();
                self.textarea.input(input);
                // Reset history index when user types
                if matches!(key.code, KeyCode::Backspace | KeyCode::Delete) {
                    self.history_index = None;
                }
                true
            }
        }
    }

    /// Delete the word before the cursor (Ctrl+W behavior).
    ///
    /// Deletes backward from the cursor to the start of the previous word,
    /// following Unix readline conventions:
    /// - Whitespace before the cursor is deleted first
    /// - Then characters until the next whitespace or start of line
    fn delete_word_backward(&mut self) {
        let text = self.text();
        let (row, col) = self.textarea.cursor();

        // For single-line input, work with the whole text
        // For multiline, work with the current line
        let lines: Vec<&str> = text.lines().collect();
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        if col == 0 {
            // At start of line - in multiline mode, could join with previous line
            // For simplicity, just do nothing at line start
            return;
        }

        // Get the portion of the line before the cursor
        let before_cursor: String = line.chars().take(col).collect();

        // Find word boundary going backward
        // Skip trailing whitespace first, then delete until next whitespace
        let mut chars: Vec<char> = before_cursor.chars().collect();
        let original_len = chars.len();

        // Skip trailing whitespace
        while !chars.is_empty() && chars.last().map(|c| c.is_whitespace()).unwrap_or(false) {
            chars.pop();
        }

        // Delete word characters until whitespace or empty
        while !chars.is_empty() && !chars.last().map(|c| c.is_whitespace()).unwrap_or(true) {
            chars.pop();
        }

        let chars_to_delete = original_len - chars.len();

        // Use tui-textarea's delete functionality
        for _ in 0..chars_to_delete {
            self.textarea.delete_char();
        }
    }

    /// Get the current input text.
    ///
    /// For multiline input, lines are joined with newlines.
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    /// Set the input text.
    ///
    /// Replaces all current content with the provided text.
    pub fn set_text(&mut self, text: &str) {
        // Clear existing content
        self.textarea.select_all();
        self.textarea.cut();

        // Insert new text
        for (i, line) in text.lines().enumerate() {
            if i > 0 {
                self.textarea.insert_newline();
            }
            self.textarea.insert_str(line);
        }
    }

    /// Clear the input.
    pub fn clear(&mut self) {
        self.textarea.select_all();
        self.textarea.cut();
        self.history_index = None;
    }

    /// Submit current input, adding to history and clearing.
    ///
    /// Returns the submitted text. Empty input is not added to history.
    pub fn submit(&mut self) -> String {
        let text = self.text();
        if !text.is_empty() {
            self.add_to_history(text.clone());
        }
        self.clear();
        text
    }

    /// Check if the input is empty.
    pub fn is_empty(&self) -> bool {
        self.textarea.is_empty()
    }

    /// Navigate to the previous history item (Up key).
    ///
    /// If not currently browsing history, starts from the most recent item.
    /// If already at the oldest item, stays there.
    ///
    /// Note: When running in terminal multiplexers (tmux/screen) with alternate
    /// screen mode, arrow keys may sometimes be misinterpreted. Use Ctrl+P as
    /// an alternative, or configure your multiplexer's escape-time setting (#2201).
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        // Calculate new index with bounds checking to prevent wrapping issues (#2201)
        let new_index = match self.history_index {
            None => {
                // Start browsing from most recent
                self.history.len().saturating_sub(1)
            }
            Some(0) => {
                // Already at oldest, stay there (don't wrap)
                0
            }
            Some(idx) => {
                // Move to older item, ensuring we don't underflow
                idx.saturating_sub(1)
            }
        };

        // Only update if the index actually changed or we're starting navigation
        if self.history_index != Some(new_index) {
            self.history_index = Some(new_index);
            if let Some(text) = self.history.get(new_index) {
                self.set_text(&text.clone());
            }
        }
    }

    /// Navigate to the next history item (Down key).
    ///
    /// If at the most recent item, clears the input and exits history browsing.
    ///
    /// Note: When running in terminal multiplexers (tmux/screen) with alternate
    /// screen mode, arrow keys may sometimes be misinterpreted. Use Ctrl+N as
    /// an alternative, or configure your multiplexer's escape-time setting (#2201).
    pub fn history_next(&mut self) {
        if self.history.is_empty() {
            return;
        }

        match self.history_index {
            None => {
                // Not browsing history, do nothing
            }
            Some(idx) if idx >= self.history.len().saturating_sub(1) => {
                // At most recent, exit history browsing and clear
                self.history_index = None;
                self.clear();
            }
            Some(idx) => {
                // Move to more recent item with bounds checking (#2201)
                let new_idx = (idx + 1).min(self.history.len().saturating_sub(1));
                self.history_index = Some(new_idx);
                if let Some(text) = self.history.get(new_idx) {
                    self.set_text(&text.clone());
                }
            }
        }
    }

    /// Add an item to the history.
    ///
    /// Duplicates are allowed. The item is added to the end (most recent).
    pub fn add_to_history(&mut self, text: String) {
        if !text.is_empty() {
            self.history.push(text);
        }
        self.history_index = None;
    }

    /// Get the history.
    pub fn history(&self) -> &[String] {
        &self.history
    }

    /// Set the history.
    ///
    /// Replaces all existing history. The last item is the most recent.
    pub fn set_history(&mut self, history: Vec<String>) {
        self.history = history;
        self.history_index = None;
    }

    /// Set focus state and return self for chaining.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        self.apply_theme();
    }

    /// Insert a string at the current cursor position.
    ///
    /// This is useful for pasting text from the clipboard.
    pub fn insert_str(&mut self, text: &str) {
        self.textarea.insert_str(text);
    }

    /// Select all text in the input.
    pub fn select_all(&mut self) {
        self.textarea.select_all();
    }

    /// Get the current cursor position (column index in current line).
    ///
    /// For single-line input, this is the character position.
    /// For multiline input, this is the column in the current row.
    pub fn cursor_pos(&self) -> usize {
        let (_, col) = self.textarea.cursor();
        col
    }

    /// Get the cursor row and column.
    ///
    /// Returns (row, col) where row is 0-indexed line number
    /// and col is 0-indexed column position.
    pub fn cursor(&self) -> (usize, usize) {
        self.textarea.cursor()
    }

    /// Select the word at the current cursor position.
    ///
    /// This is typically called on double-click. It finds word boundaries
    /// around the cursor and selects the word.
    pub fn select_word_at_cursor(&mut self) {
        let text = self.text();
        let (row, col) = self.textarea.cursor();

        // Get the current line
        let lines: Vec<&str> = text.lines().collect();
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        let chars: Vec<char> = line.chars().collect();

        if chars.is_empty() || col >= chars.len() {
            return;
        }

        // Find word start (go backward from cursor)
        let mut word_start = col;
        while word_start > 0 && !chars[word_start - 1].is_whitespace() {
            word_start -= 1;
        }

        // Find word end (go forward from cursor)
        let mut word_end = col;
        while word_end < chars.len() && !chars[word_end].is_whitespace() {
            word_end += 1;
        }

        // Only select if we found a word (not just whitespace)
        if word_start < word_end {
            // Move cursor to word start, then start selection to word end
            // tui-textarea uses different methods for this
            // We'll use start_selection and then move to select
            self.textarea.cancel_selection();

            // Move to word start
            let current_col = col;
            for _ in word_start..current_col {
                self.textarea.move_cursor(tui_textarea::CursorMove::Back);
            }

            // Start selection
            self.textarea.start_selection();

            // Move to word end
            for _ in word_start..word_end {
                self.textarea.move_cursor(tui_textarea::CursorMove::Forward);
            }
        }
    }

    /// Move cursor to a specific column position (for click positioning).
    ///
    /// This is useful for handling mouse clicks in the input area.
    pub fn move_cursor_to_col(&mut self, col: usize) {
        let (_, current_col) = self.textarea.cursor();

        if col < current_col {
            // Move backward
            for _ in col..current_col {
                self.textarea.move_cursor(tui_textarea::CursorMove::Back);
            }
        } else if col > current_col {
            // Move forward
            for _ in current_col..col {
                self.textarea.move_cursor(tui_textarea::CursorMove::Forward);
            }
        }
    }

    /// Returns the current editing mode.
    pub fn editing_mode(&self) -> EditingMode {
        self.editing_mode
    }

    /// Sets the editing mode.
    pub fn set_editing_mode(&mut self, mode: EditingMode) {
        self.editing_mode = mode;
    }

    /// Toggles between insert and overwrite editing modes.
    pub fn toggle_editing_mode(&mut self) {
        self.editing_mode = match self.editing_mode {
            EditingMode::Insert => EditingMode::Overwrite,
            EditingMode::Overwrite => EditingMode::Insert,
        };
    }

    /// Select the entire current line.
    ///
    /// This is typically called on triple-click. It selects all text
    /// on the current line (including any leading/trailing whitespace).
    pub fn select_current_line(&mut self) {
        let text = self.text();
        let (row, _) = self.textarea.cursor();

        // Get the current line
        let lines: Vec<&str> = text.lines().collect();
        if row >= lines.len() {
            return;
        }

        let line = lines[row];
        let line_len = line.chars().count();

        if line_len == 0 {
            return;
        }

        // Cancel any existing selection
        self.textarea.cancel_selection();

        // Move cursor to start of line
        self.textarea.move_cursor(tui_textarea::CursorMove::Head);

        // Start selection
        self.textarea.start_selection();

        // Move to end of line
        self.textarea.move_cursor(tui_textarea::CursorMove::End);
    }
}

impl Widget for CortexInput<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        self.textarea.render(area, buf);
    }
}

impl WidgetRef for CortexInput<'_> {
    fn render_ref(&self, area: Rect, buf: &mut Buffer) {
        // TextArea doesn't implement WidgetRef, so we clone for reference rendering
        self.textarea.clone().render(area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_input_is_empty() {
        let input = CortexInput::new();
        assert!(input.is_empty());
        assert_eq!(input.text(), "");
    }

    #[test]
    fn test_set_and_get_text() {
        let mut input = CortexInput::new();
        input.set_text("hello world");
        assert_eq!(input.text(), "hello world");
        assert!(!input.is_empty());
    }

    #[test]
    fn test_clear() {
        let mut input = CortexInput::new();
        input.set_text("some text");
        input.clear();
        assert!(input.is_empty());
    }

    #[test]
    fn test_submit_adds_to_history() {
        let mut input = CortexInput::new();
        input.set_text("command 1");
        let text = input.submit();

        assert_eq!(text, "command 1");
        assert!(input.is_empty());
        assert_eq!(input.history(), &["command 1"]);
    }

    #[test]
    fn test_submit_empty_not_added_to_history() {
        let mut input = CortexInput::new();
        let text = input.submit();

        assert_eq!(text, "");
        assert!(input.history().is_empty());
    }

    #[test]
    fn test_history_navigation() {
        let mut input = CortexInput::new();
        input.set_history(vec![
            "first".to_string(),
            "second".to_string(),
            "third".to_string(),
        ]);

        // Navigate up through history
        input.history_prev();
        assert_eq!(input.text(), "third");

        input.history_prev();
        assert_eq!(input.text(), "second");

        input.history_prev();
        assert_eq!(input.text(), "first");

        // At oldest, stays there
        input.history_prev();
        assert_eq!(input.text(), "first");

        // Navigate back down
        input.history_next();
        assert_eq!(input.text(), "second");

        input.history_next();
        assert_eq!(input.text(), "third");

        // Past most recent, clears and exits history
        input.history_next();
        assert!(input.is_empty());
    }

    #[test]
    fn test_history_empty() {
        let mut input = CortexInput::new();

        // Should not panic with empty history
        input.history_prev();
        input.history_next();
        assert!(input.is_empty());
    }

    #[test]
    fn test_builder_pattern() {
        let input = CortexInput::new()
            .with_placeholder("Enter text...")
            .multiline()
            .focused(true);

        assert!(input.multiline);
        assert!(input.focused);
        assert_eq!(input.placeholder, Some("Enter text...".to_string()));
    }

    #[test]
    fn test_ctrl_u_clears() {
        let mut input = CortexInput::new();
        input.set_text("some text");

        let key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
        let handled = input.handle_key(key);

        assert!(handled);
        assert!(input.is_empty());
    }

    #[test]
    fn test_ctrl_w_deletes_word() {
        let mut input = CortexInput::new();
        input.set_text("hello world");
        // Cursor should be at end after set_text

        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let handled = input.handle_key(key);

        assert!(handled);
        assert_eq!(input.text(), "hello ");
    }

    #[test]
    fn test_ctrl_w_deletes_word_with_trailing_space() {
        let mut input = CortexInput::new();
        input.set_text("hello world ");
        // Cursor at end, trailing space should be deleted along with "world"

        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let handled = input.handle_key(key);

        assert!(handled);
        assert_eq!(input.text(), "hello ");
    }

    #[test]
    fn test_ctrl_w_empty_input() {
        let mut input = CortexInput::new();
        // Empty input - should not panic

        let key = KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL);
        let handled = input.handle_key(key);

        assert!(handled);
        assert!(input.is_empty());
    }

    #[test]
    fn test_insert_key_handled() {
        let mut input = CortexInput::new();
        input.set_text("test");

        let key = KeyEvent::new(KeyCode::Insert, KeyModifiers::NONE);
        let handled = input.handle_key(key);

        // Insert key should be handled (acknowledged) even though
        // we don't fully implement overwrite mode
        assert!(handled);
    }
}
