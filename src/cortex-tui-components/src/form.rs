//! Form component with multiple fields.

use crate::component::{Component, ComponentResult, FocusState};
use crate::focus::FocusManager;
use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_segmentation::UnicodeSegmentation;

/// Kind of form field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormFieldKind {
    /// Plain text input
    Text,
    /// Masked password input
    Secret,
    /// Number input
    Number,
    /// Boolean toggle
    Toggle,
    /// Selection from options
    Select(Vec<String>),
}

/// A form field.
#[derive(Debug, Clone)]
pub struct FormField {
    /// Field key/identifier
    pub key: String,
    /// Display label
    pub label: String,
    /// Field kind
    pub kind: FormFieldKind,
    /// Current value
    pub value: String,
    /// Whether required
    pub required: bool,
    /// Placeholder text
    pub placeholder: Option<String>,
    /// Cursor position (for text inputs)
    pub cursor: usize,
    /// Selected index (for Select/Toggle)
    pub selected_index: usize,
    /// Toggle state
    pub toggled: bool,
}

impl FormField {
    /// Create a text field.
    pub fn text(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Text,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor: 0,
            selected_index: 0,
            toggled: false,
        }
    }

    /// Create a secret (password) field.
    pub fn secret(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Secret,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor: 0,
            selected_index: 0,
            toggled: false,
        }
    }

    /// Create a number field.
    pub fn number(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Number,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor: 0,
            selected_index: 0,
            toggled: false,
        }
    }

    /// Create a toggle field.
    pub fn toggle(key: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Toggle,
            value: String::new(),
            required: false,
            placeholder: None,
            cursor: 0,
            selected_index: 0,
            toggled: false,
        }
    }

    /// Create a select field.
    pub fn select(key: impl Into<String>, label: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            kind: FormFieldKind::Select(options),
            value: String::new(),
            required: false,
            placeholder: None,
            cursor: 0,
            selected_index: 0,
            toggled: false,
        }
    }

    /// Mark as required.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    /// Set placeholder text.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Set initial value.
    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        let v = value.into();
        self.cursor = v.graphemes(true).count();
        self.value = v;
        self
    }

    /// Get display value.
    pub fn display_value(&self) -> String {
        match &self.kind {
            FormFieldKind::Text | FormFieldKind::Number => self.value.clone(),
            FormFieldKind::Secret => "*".repeat(self.value.graphemes(true).count()),
            FormFieldKind::Toggle => {
                if self.toggled {
                    "ON".to_string()
                } else {
                    "OFF".to_string()
                }
            }
            FormFieldKind::Select(options) => options
                .get(self.selected_index)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// Result of form submission.
#[derive(Debug, Clone)]
pub struct FormResult {
    /// Field values as key-value pairs
    pub values: Vec<(String, String)>,
}

impl FormResult {
    /// Get a value by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Form state.
#[derive(Debug, Clone)]
pub struct FormState {
    /// Form title
    pub title: String,
    /// Fields
    pub fields: Vec<FormField>,
    /// Focus manager
    pub focus: FocusManager,
}

impl FormState {
    /// Create new form state.
    pub fn new(title: impl Into<String>, fields: Vec<FormField>) -> Self {
        let count = fields.len() + 1; // +1 for submit button
        Self {
            title: title.into(),
            fields,
            focus: FocusManager::new(count),
        }
    }

    /// Check if submit button is focused.
    pub fn is_submit_focused(&self) -> bool {
        self.focus.current() == self.fields.len()
    }

    /// Check if form can be submitted.
    pub fn can_submit(&self) -> bool {
        self.fields.iter().all(|f| {
            if !f.required {
                return true;
            }
            match &f.kind {
                FormFieldKind::Text | FormFieldKind::Secret | FormFieldKind::Number => {
                    !f.value.trim().is_empty()
                }
                FormFieldKind::Toggle | FormFieldKind::Select(_) => true,
            }
        })
    }

    /// Get form result.
    pub fn result(&self) -> FormResult {
        FormResult {
            values: self
                .fields
                .iter()
                .map(|f| {
                    let value = match &f.kind {
                        FormFieldKind::Toggle => f.toggled.to_string(),
                        FormFieldKind::Select(opts) => {
                            opts.get(f.selected_index).cloned().unwrap_or_default()
                        }
                        _ => f.value.clone(),
                    };
                    (f.key.clone(), value)
                })
                .collect(),
        }
    }
}

/// A form component.
pub struct Form {
    /// Form state
    pub state: FormState,
    /// Whether focused
    focused: bool,
}

impl Form {
    /// Create a new form.
    pub fn new(title: impl Into<String>, fields: Vec<FormField>) -> Self {
        Self {
            state: FormState::new(title, fields),
            focused: true,
        }
    }
}

impl Component for Form {
    type Output = FormResult;

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 5 || area.width < 20 {
            return;
        }

        let mut y = area.y;
        let field_width = area.width.saturating_sub(4);

        for (i, field) in self.state.fields.iter().enumerate() {
            if y + 2 > area.bottom().saturating_sub(2) {
                break;
            }

            let is_focused = self.state.focus.current() == i;

            // Label
            let label_style = if is_focused {
                Style::default()
                    .fg(CYAN_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };

            let label = if field.required {
                format!("{} *", field.label)
            } else {
                field.label.clone()
            };
            buf.set_string(area.x + 2, y, &label, label_style);
            y += 1;

            // Value
            let display = field.display_value();
            let (text, style) = if display.is_empty() {
                let ph = field.placeholder.as_deref().unwrap_or("");
                (ph.to_string(), Style::default().fg(TEXT_MUTED))
            } else {
                (display, Style::default().fg(TEXT))
            };

            // Background
            let bg = if is_focused { SURFACE_1 } else { SURFACE_0 };
            for x in area.x + 2..area.x + 2 + field_width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_bg(bg);
                }
            }

            buf.set_string(area.x + 2, y, &text, style.bg(bg));

            // Cursor for text fields
            if is_focused {
                match &field.kind {
                    FormFieldKind::Text | FormFieldKind::Secret | FormFieldKind::Number => {
                        let cursor_x = area.x + 2 + field.cursor as u16;
                        if cursor_x < area.x + 2 + field_width
                            && let Some(cell) = buf.cell_mut((cursor_x, y))
                        {
                            cell.set_bg(CYAN_PRIMARY).set_fg(SURFACE_0);
                        }
                    }
                    _ => {}
                }
            }

            y += 2;
        }

        // Submit button
        let submit_y = area.bottom().saturating_sub(2);
        let submit_text = "[ Submit ]";
        let submit_x = area.x + (area.width.saturating_sub(submit_text.len() as u16)) / 2;

        let submit_style = if self.state.is_submit_focused() {
            Style::default()
                .fg(SURFACE_0)
                .bg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(TEXT_DIM)
        };

        buf.set_string(submit_x, submit_y, submit_text, submit_style);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ComponentResult<Self::Output> {
        let focus_idx = self.state.focus.current();

        match key.code {
            // Navigation
            KeyCode::Tab | KeyCode::Down => {
                self.state.focus.next();
                ComponentResult::Handled
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.state.focus.prev();
                ComponentResult::Handled
            }

            // Submit
            KeyCode::Enter => {
                if self.state.is_submit_focused() && self.state.can_submit() {
                    ComponentResult::Done(self.state.result())
                } else if self.state.is_submit_focused() {
                    ComponentResult::Handled
                } else {
                    self.state.focus.next();
                    ComponentResult::Handled
                }
            }

            // Cancel
            KeyCode::Esc => ComponentResult::Cancelled,

            // Field input
            _ if focus_idx < self.state.fields.len() => {
                let field = &mut self.state.fields[focus_idx];
                match &field.kind {
                    FormFieldKind::Text | FormFieldKind::Secret => match key.code {
                        KeyCode::Char(c) => {
                            let byte_offset = field
                                .value
                                .grapheme_indices(true)
                                .nth(field.cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(field.value.len());
                            field.value.insert(byte_offset, c);
                            field.cursor += 1;
                            ComponentResult::Handled
                        }
                        KeyCode::Backspace if field.cursor > 0 => {
                            let new_cursor = field.cursor - 1;
                            let start = field
                                .value
                                .grapheme_indices(true)
                                .nth(new_cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            let end = field
                                .value
                                .grapheme_indices(true)
                                .nth(field.cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(field.value.len());
                            field.value.replace_range(start..end, "");
                            field.cursor = new_cursor;
                            ComponentResult::Handled
                        }
                        KeyCode::Left if field.cursor > 0 => {
                            field.cursor -= 1;
                            ComponentResult::Handled
                        }
                        KeyCode::Right if field.cursor < field.value.graphemes(true).count() => {
                            field.cursor += 1;
                            ComponentResult::Handled
                        }
                        _ => ComponentResult::NotHandled,
                    },
                    FormFieldKind::Number => match key.code {
                        KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == '-' => {
                            let byte_offset = field
                                .value
                                .grapheme_indices(true)
                                .nth(field.cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(field.value.len());
                            field.value.insert(byte_offset, c);
                            field.cursor += 1;
                            ComponentResult::Handled
                        }
                        KeyCode::Backspace if field.cursor > 0 => {
                            let new_cursor = field.cursor - 1;
                            let start = field
                                .value
                                .grapheme_indices(true)
                                .nth(new_cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            let end = field
                                .value
                                .grapheme_indices(true)
                                .nth(field.cursor)
                                .map(|(i, _)| i)
                                .unwrap_or(field.value.len());
                            field.value.replace_range(start..end, "");
                            field.cursor = new_cursor;
                            ComponentResult::Handled
                        }
                        _ => ComponentResult::NotHandled,
                    },
                    FormFieldKind::Toggle => match key.code {
                        KeyCode::Char(' ') | KeyCode::Left | KeyCode::Right => {
                            field.toggled = !field.toggled;
                            ComponentResult::Handled
                        }
                        _ => ComponentResult::NotHandled,
                    },
                    FormFieldKind::Select(options) => match key.code {
                        KeyCode::Left if field.selected_index > 0 => {
                            field.selected_index -= 1;
                            ComponentResult::Handled
                        }
                        KeyCode::Right if field.selected_index + 1 < options.len() => {
                            field.selected_index += 1;
                            ComponentResult::Handled
                        }
                        KeyCode::Char(' ') => {
                            field.selected_index = (field.selected_index + 1) % options.len();
                            ComponentResult::Handled
                        }
                        _ => ComponentResult::NotHandled,
                    },
                }
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
        vec![("Tab", "Next"), ("Enter", "Submit"), ("Esc", "Cancel")]
    }
}

/// Builder for forms.
pub struct FormBuilder {
    title: String,
    fields: Vec<FormField>,
}

impl FormBuilder {
    /// Create a new form builder.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            fields: Vec::new(),
        }
    }

    /// Add a field.
    pub fn field(mut self, field: FormField) -> Self {
        self.fields.push(field);
        self
    }

    /// Build the form.
    pub fn build(self) -> Form {
        Form::new(self.title, self.fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_form_field_builders() {
        let text = FormField::text("name", "Name")
            .required()
            .with_placeholder("Enter name");
        assert_eq!(text.key, "name");
        assert!(text.required);

        let toggle = FormField::toggle("enabled", "Enabled");
        assert!(!toggle.toggled);
    }

    #[test]
    fn test_form_can_submit() {
        let form = Form::new(
            "Test",
            vec![
                FormField::text("name", "Name")
                    .required()
                    .with_value("John"),
                FormField::text("email", "Email"),
            ],
        );

        assert!(form.state.can_submit());
    }

    #[test]
    fn test_form_cannot_submit_empty_required() {
        let form = Form::new("Test", vec![FormField::text("name", "Name").required()]);

        assert!(!form.state.can_submit());
    }

    #[test]
    fn test_form_result() {
        let form = Form::new(
            "Test",
            vec![
                FormField::text("name", "Name").with_value("John"),
                FormField::toggle("enabled", "Enabled"),
            ],
        );

        let result = form.state.result();
        assert_eq!(result.get("name"), Some("John"));
        assert_eq!(result.get("enabled"), Some("false"));
    }
}
