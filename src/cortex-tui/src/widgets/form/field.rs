//! Form field definitions and implementations.

use super::field_kind::FieldKind;
use super::utils::grapheme_count;

/// A single field in the form.
#[derive(Debug, Clone)]
pub struct FormField {
    /// Unique key for the field.
    pub key: String,
    /// Display label.
    pub label: String,
    /// Field type and configuration.
    pub kind: FieldKind,
    /// Current value as a string.
    pub value: String,
    /// Whether the field is required.
    pub required: bool,
    /// Placeholder text.
    pub placeholder: Option<String>,
    /// Cursor position (for text fields).
    pub cursor_pos: usize,
    /// Selected index (for Select fields).
    pub select_index: usize,
    /// Toggle state (for Toggle fields).
    pub toggle_state: bool,
}

impl FormField {
    /// Creates a new text field.
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Text,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor_pos: 0,
            select_index: 0,
            toggle_state: false,
        }
    }

    /// Creates a new secret (password) field.
    pub fn secret(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Secret,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor_pos: 0,
            select_index: 0,
            toggle_state: false,
        }
    }

    /// Creates a new number field.
    pub fn number(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Number,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor_pos: 0,
            select_index: 0,
            toggle_state: false,
        }
    }

    /// Creates a new toggle field.
    pub fn toggle(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Toggle,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor_pos: 0,
            select_index: 0,
            toggle_state: false,
        }
    }

    /// Creates a new select field with options.
    pub fn select(key: impl Into<String>, label: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FieldKind::Select(options),
            value: String::new(),
            required: false,
            placeholder: None,
            cursor_pos: 0,
            select_index: 0,
            toggle_state: false,
        }
    }

    /// Sets the field as required.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Sets the placeholder text.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Sets the initial value.
    /// Cursor is positioned at the end (in grapheme units).
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        let val = value.into();
        self.cursor_pos = grapheme_count(&val);
        self.value = val;
        self
    }

    /// Gets the display value for this field.
    pub fn display_value(&self) -> String {
        match &self.kind {
            FieldKind::Text | FieldKind::Number => self.value.clone(),
            FieldKind::Secret => "*".repeat(self.value.len()),
            FieldKind::Toggle => {
                if self.toggle_state {
                    "ON".to_string()
                } else {
                    "OFF".to_string()
                }
            }
            FieldKind::Select(options) => {
                if self.select_index < options.len() {
                    options[self.select_index].clone()
                } else {
                    String::new()
                }
            }
        }
    }
}
