//! Text input component.
//!
//! Single-line text input with cursor, history, and completion support.

use crate::component::{Component, ComponentResult, FocusState};
use cortex_core::style::{CYAN_PRIMARY, SURFACE_1, TEXT, TEXT_MUTED};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use unicode_segmentation::UnicodeSegmentation;

/// State for a text input.
#[derive(Debug, Clone, Default)]
pub struct InputState {
    /// Current text value
    pub value: String,
    /// Cursor position (in graphemes)
    pub cursor: usize,
    /// Placeholder text
    pub placeholder: Option<String>,
    /// Whether the input is masked (password)
    pub masked: bool,
}

impl InputState {
    /// Create new input state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the initial value.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        let v = value.into();
        self.cursor = v.graphemes(true).count();
        self.value = v;
        self
    }

    /// Set placeholder text.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Set as masked (password) input.
    pub fn masked(mut self) -> Self {
        self.masked = true;
        self
    }

    /// Insert a character at the cursor.
    pub fn insert(&mut self, c: char) {
        let byte_offset = self.grapheme_to_byte_offset(self.cursor);
        self.value.insert(byte_offset, c);
        self.cursor += 1;
    }

    /// Delete the character before the cursor.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let new_cursor = self.cursor - 1;
            let start_byte = self.grapheme_to_byte_offset(new_cursor);
            let end_byte = self.grapheme_to_byte_offset(self.cursor);
            self.value.replace_range(start_byte..end_byte, "");
            self.cursor = new_cursor;
        }
    }

    /// Delete the character at the cursor.
    pub fn delete(&mut self) {
        let grapheme_count = self.value.graphemes(true).count();
        if self.cursor < grapheme_count {
            let start_byte = self.grapheme_to_byte_offset(self.cursor);
            let end_byte = self.grapheme_to_byte_offset(self.cursor + 1);
            self.value.replace_range(start_byte..end_byte, "");
        }
    }

    /// Move cursor left.
    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor right.
    pub fn move_right(&mut self) {
        let grapheme_count = self.value.graphemes(true).count();
        if self.cursor < grapheme_count {
            self.cursor += 1;
        }
    }

    /// Move cursor to start.
    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    /// Move cursor to end.
    pub fn move_end(&mut self) {
        self.cursor = self.value.graphemes(true).count();
    }

    /// Clear the input.
    pub fn clear(&mut self) {
        self.value.clear();
        self.cursor = 0;
    }

    /// Get the display value (masked if password).
    pub fn display_value(&self) -> String {
        if self.masked {
            "*".repeat(self.value.graphemes(true).count())
        } else {
            self.value.clone()
        }
    }

    /// Insert text at cursor (for paste).
    pub fn insert_str(&mut self, text: &str) {
        let byte_offset = self.grapheme_to_byte_offset(self.cursor);
        self.value.insert_str(byte_offset, text);
        self.cursor += text.graphemes(true).count();
    }

    fn grapheme_to_byte_offset(&self, grapheme_idx: usize) -> usize {
        self.value
            .grapheme_indices(true)
            .nth(grapheme_idx)
            .map(|(byte_idx, _)| byte_idx)
            .unwrap_or(self.value.len())
    }
}

/// A single-line text input widget.
pub struct TextInput<'a> {
    state: &'a InputState,
    focused: bool,
    label: Option<&'a str>,
}

impl<'a> TextInput<'a> {
    /// Create a new text input widget.
    pub fn new(state: &'a InputState) -> Self {
        Self {
            state,
            focused: true,
            label: None,
        }
    }

    /// Set whether the input is focused.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Set a label for the input.
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }
}

impl Widget for TextInput<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 5 {
            return;
        }

        let mut x = area.x;

        // Label
        if let Some(label) = self.label {
            buf.set_string(x, area.y, label, Style::default().fg(TEXT));
            x += label.len() as u16 + 1;
        }

        // Background
        let bg_style = if self.focused {
            Style::default().bg(SURFACE_1)
        } else {
            Style::default()
        };
        for col in x..area.right() {
            if let Some(cell) = buf.cell_mut((col, area.y)) {
                cell.set_style(bg_style);
            }
        }

        // Value or placeholder
        let display = self.state.display_value();
        let (text, style) = if display.is_empty() {
            let placeholder = self.state.placeholder.as_deref().unwrap_or("");
            (placeholder.to_string(), Style::default().fg(TEXT_MUTED))
        } else {
            (display, Style::default().fg(TEXT))
        };

        buf.set_string(x, area.y, &text, style.bg(SURFACE_1));

        // Cursor
        if self.focused {
            let cursor_x = x + self.state.cursor as u16;
            if cursor_x < area.right()
                && let Some(cell) = buf.cell_mut((cursor_x, area.y))
            {
                cell.set_bg(CYAN_PRIMARY).set_fg(SURFACE_1);
            }
        }
    }
}

/// An interactive text input component.
pub struct TextInputComponent {
    /// State
    pub state: InputState,
    /// Whether focused
    focused: bool,
    /// Label
    label: Option<String>,
}

impl TextInputComponent {
    /// Create a new text input component.
    pub fn new() -> Self {
        Self {
            state: InputState::new(),
            focused: true,
            label: None,
        }
    }

    /// Set the initial value.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.state = self.state.with_value(value);
        self
    }

    /// Set placeholder text.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.state = self.state.with_placeholder(placeholder);
        self
    }

    /// Set as masked (password) input.
    pub fn masked(mut self) -> Self {
        self.state = self.state.masked();
        self
    }

    /// Set a label.
    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Get the current value.
    pub fn value(&self) -> &str {
        &self.state.value
    }
}

impl Default for TextInputComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for TextInputComponent {
    type Output = String;

    fn render(&self, area: Rect, buf: &mut Buffer) {
        let widget = TextInput::new(&self.state).focused(self.focused);
        widget.render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ComponentResult<Self::Output> {
        match key.code {
            KeyCode::Enter => ComponentResult::Done(self.state.value.clone()),
            KeyCode::Esc => ComponentResult::Cancelled,
            KeyCode::Backspace => {
                self.state.backspace();
                ComponentResult::Handled
            }
            KeyCode::Delete => {
                self.state.delete();
                ComponentResult::Handled
            }
            KeyCode::Left => {
                self.state.move_left();
                ComponentResult::Handled
            }
            KeyCode::Right => {
                self.state.move_right();
                ComponentResult::Handled
            }
            KeyCode::Home => {
                self.state.move_home();
                ComponentResult::Handled
            }
            KeyCode::End => {
                self.state.move_end();
                ComponentResult::Handled
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.state.clear();
                ComponentResult::Handled
            }
            KeyCode::Char(c) => {
                self.state.insert(c);
                ComponentResult::Handled
            }
            _ => ComponentResult::NotHandled,
        }
    }

    fn focus_state(&self) -> FocusState {
        if self.focused {
            FocusState::Editing
        } else {
            FocusState::Unfocused
        }
    }

    fn set_focus(&mut self, focused: bool) {
        self.focused = focused;
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Enter", "Submit"), ("Esc", "Cancel")]
    }

    fn handle_paste(&mut self, text: &str) -> bool {
        self.state.insert_str(text);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_state_basic() {
        let mut state = InputState::new();

        state.insert('H');
        state.insert('i');
        assert_eq!(state.value, "Hi");
        assert_eq!(state.cursor, 2);

        state.backspace();
        assert_eq!(state.value, "H");
        assert_eq!(state.cursor, 1);
    }

    #[test]
    fn test_input_state_navigation() {
        let mut state = InputState::new().with_value("Hello");
        assert_eq!(state.cursor, 5);

        state.move_left();
        assert_eq!(state.cursor, 4);

        state.move_home();
        assert_eq!(state.cursor, 0);

        state.move_end();
        assert_eq!(state.cursor, 5);
    }

    #[test]
    fn test_input_state_masked() {
        let state = InputState::new().with_value("secret").masked();
        assert_eq!(state.display_value(), "******");
    }

    #[test]
    fn test_input_state_paste() {
        let mut state = InputState::new().with_value("Hello");
        state.move_home();
        state.insert_str("Oh ");
        assert_eq!(state.value, "Oh Hello");
    }
}
