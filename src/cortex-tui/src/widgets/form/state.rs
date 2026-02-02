//! Form state management and input handling.

use super::field::FormField;
use super::field_kind::FieldKind;
use super::utils::{grapheme_count, grapheme_to_byte_offset};

/// State for a generic form.
#[derive(Debug, Clone)]
pub struct FormState {
    /// Form title.
    pub title: String,
    /// Command associated with this form.
    pub command: String,
    /// List of fields.
    pub fields: Vec<FormField>,
    /// Index of the currently focused field.
    pub focus_index: usize,
    /// Scroll offset for the form.
    pub scroll_offset: usize,
}

impl FormState {
    /// Creates a new form state.
    pub fn new(
        title: impl Into<String>,
        command: impl Into<String>,
        fields: Vec<FormField>,
    ) -> Self {
        Self {
            title: title.into(),
            command: command.into(),
            fields,
            focus_index: 0,
            scroll_offset: 0,
        }
    }

    /// Moves focus to the next field.
    pub fn focus_next(&mut self) {
        if !self.fields.is_empty() {
            self.focus_index = (self.focus_index + 1) % (self.fields.len() + 1); // +1 for submit button
            self.ensure_visible();
        }
    }

    /// Moves focus to the previous field.
    pub fn focus_prev(&mut self) {
        if !self.fields.is_empty() {
            if self.focus_index == 0 {
                self.focus_index = self.fields.len(); // Submit button
            } else {
                self.focus_index -= 1;
            }
            self.ensure_visible();
        }
    }

    /// Ensures the focused field is visible.
    fn ensure_visible(&mut self) {
        let visible_items = 5;
        if self.focus_index < self.scroll_offset {
            self.scroll_offset = self.focus_index;
        } else if self.focus_index >= self.scroll_offset + visible_items {
            self.scroll_offset = self.focus_index.saturating_sub(visible_items - 1);
        }
    }

    /// Handles a character input for the current field.
    /// Uses grapheme-based cursor positioning for proper Unicode support.
    pub fn handle_char(&mut self, c: char) {
        if self.focus_index < self.fields.len() {
            let field = &mut self.fields[self.focus_index];
            match &field.kind {
                FieldKind::Text | FieldKind::Secret => {
                    let byte_offset = grapheme_to_byte_offset(&field.value, field.cursor_pos);
                    field.value.insert(byte_offset, c);
                    field.cursor_pos += 1;
                }
                FieldKind::Number => {
                    if c.is_ascii_digit() || (c == '-' && field.cursor_pos == 0) || c == '.' {
                        let byte_offset = grapheme_to_byte_offset(&field.value, field.cursor_pos);
                        field.value.insert(byte_offset, c);
                        field.cursor_pos += 1;
                    }
                }
                FieldKind::Toggle | FieldKind::Select(_) => {
                    // These don't accept character input
                }
            }
        }
    }

    /// Handles backspace for the current field.
    /// Uses grapheme-based deletion for proper Unicode/emoji support.
    pub fn handle_backspace(&mut self) {
        if self.focus_index < self.fields.len() {
            let field = &mut self.fields[self.focus_index];
            match &field.kind {
                FieldKind::Text | FieldKind::Secret | FieldKind::Number => {
                    if field.cursor_pos > 0 {
                        let new_cursor_pos = field.cursor_pos - 1;
                        let start_byte = grapheme_to_byte_offset(&field.value, new_cursor_pos);
                        let end_byte = grapheme_to_byte_offset(&field.value, field.cursor_pos);
                        field.value.replace_range(start_byte..end_byte, "");
                        field.cursor_pos = new_cursor_pos;
                    }
                }
                FieldKind::Toggle | FieldKind::Select(_) => {
                    // These don't handle backspace
                }
            }
        }
    }

    /// Handles left arrow key.
    /// Cursor position is in grapheme units for proper Unicode support.
    pub fn handle_left(&mut self) {
        if self.focus_index < self.fields.len() {
            let field = &mut self.fields[self.focus_index];
            match &field.kind {
                FieldKind::Text | FieldKind::Secret | FieldKind::Number => {
                    if field.cursor_pos > 0 {
                        field.cursor_pos -= 1;
                    }
                }
                FieldKind::Select(options) => {
                    if field.select_index > 0 {
                        field.select_index -= 1;
                    } else if !options.is_empty() {
                        field.select_index = options.len() - 1;
                    }
                }
                FieldKind::Toggle => {
                    field.toggle_state = !field.toggle_state;
                }
            }
        }
    }

    /// Handles right arrow key.
    /// Cursor position is in grapheme units for proper Unicode support.
    pub fn handle_right(&mut self) {
        if self.focus_index < self.fields.len() {
            let field = &mut self.fields[self.focus_index];
            match &field.kind {
                FieldKind::Text | FieldKind::Secret | FieldKind::Number => {
                    let total_graphemes = grapheme_count(&field.value);
                    if field.cursor_pos < total_graphemes {
                        field.cursor_pos += 1;
                    }
                }
                FieldKind::Select(options) => {
                    if !options.is_empty() {
                        field.select_index = (field.select_index + 1) % options.len();
                    }
                }
                FieldKind::Toggle => {
                    field.toggle_state = !field.toggle_state;
                }
            }
        }
    }

    /// Toggles the current field (for Toggle and Select types).
    pub fn toggle_current(&mut self) {
        if self.focus_index < self.fields.len() {
            let field = &mut self.fields[self.focus_index];
            match &field.kind {
                FieldKind::Toggle => {
                    field.toggle_state = !field.toggle_state;
                }
                FieldKind::Select(options) => {
                    if !options.is_empty() {
                        field.select_index = (field.select_index + 1) % options.len();
                    }
                }
                _ => {}
            }
        }
    }

    /// Returns true if submit button is focused.
    pub fn is_submit_focused(&self) -> bool {
        self.focus_index == self.fields.len()
    }

    /// Returns true if all required fields have values and the form can be submitted.
    ///
    /// For text, number, and secret fields, checks that the value is non-empty.
    /// For toggle fields, always returns true (they have a default state).
    /// For select fields, always returns true (they have a default selection).
    pub fn can_submit(&self) -> bool {
        self.fields.iter().all(|field| {
            if !field.required {
                return true;
            }
            match &field.kind {
                FieldKind::Text | FieldKind::Secret | FieldKind::Number => {
                    !field.value.trim().is_empty()
                }
                FieldKind::Toggle | FieldKind::Select(_) => true,
            }
        })
    }

    /// Handles pasted text for the current field.
    /// Uses grapheme-based cursor positioning for proper Unicode support.
    pub fn handle_paste(&mut self, text: &str) {
        if self.focus_index < self.fields.len() {
            let field = &mut self.fields[self.focus_index];
            match &field.kind {
                FieldKind::Text | FieldKind::Secret => {
                    let byte_offset = grapheme_to_byte_offset(&field.value, field.cursor_pos);
                    field.value.insert_str(byte_offset, text);
                    field.cursor_pos += grapheme_count(text);
                }
                FieldKind::Number => {
                    // Only insert valid numeric characters from pasted text
                    let filtered: String = text
                        .chars()
                        .filter(|c| {
                            c.is_ascii_digit()
                                || (*c == '-' && field.cursor_pos == 0 && field.value.is_empty())
                                || *c == '.'
                        })
                        .collect();
                    if !filtered.is_empty() {
                        let byte_offset = grapheme_to_byte_offset(&field.value, field.cursor_pos);
                        field.value.insert_str(byte_offset, &filtered);
                        field.cursor_pos += grapheme_count(&filtered);
                    }
                }
                FieldKind::Toggle | FieldKind::Select(_) => {
                    // These don't accept paste input
                }
            }
        }
    }
}
