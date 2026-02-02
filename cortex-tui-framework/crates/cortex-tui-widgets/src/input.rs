//! Single-line text input widget.
//!
//! Provides a text input field with:
//! - Cursor movement and text editing
//! - Text selection
//! - Placeholder text
//! - Password mode (character masking)
//! - Horizontal scrolling when text exceeds width
//! - Change callbacks

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::cursor::{grapheme_to_byte_offset, CursorMove, LineCursor, Selection};

/// Character used for masking password input.
const PASSWORD_MASK_CHAR: char = '•';

/// Default placeholder text color (gray).
const PLACEHOLDER_STYLE: InputStyle = InputStyle {
    text_color: None,
    placeholder_color: Some((128, 128, 128)),
    cursor_color: None,
    selection_bg: None,
};

/// Styling options for the Input widget.
#[derive(Debug, Clone, Copy)]
pub struct InputStyle {
    /// Text color as RGB tuple.
    pub text_color: Option<(u8, u8, u8)>,
    /// Placeholder text color as RGB tuple.
    pub placeholder_color: Option<(u8, u8, u8)>,
    /// Cursor color as RGB tuple.
    pub cursor_color: Option<(u8, u8, u8)>,
    /// Selection background color as RGB tuple.
    pub selection_bg: Option<(u8, u8, u8)>,
}

impl Default for InputStyle {
    fn default() -> Self {
        PLACEHOLDER_STYLE
    }
}

/// Change event data for the Input widget.
#[derive(Debug, Clone)]
pub struct InputChange {
    /// The new text value.
    pub value: String,
    /// The cursor position after the change.
    pub cursor_position: usize,
}

/// Key event representation for handling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKey {
    /// A character to insert.
    Char(char),
    /// Backspace key.
    Backspace,
    /// Delete key.
    Delete,
    /// Left arrow.
    Left,
    /// Right arrow.
    Right,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Ctrl+Left (word left).
    CtrlLeft,
    /// Ctrl+Right (word right).
    CtrlRight,
    /// Ctrl+A (select all).
    SelectAll,
    /// Ctrl+C (copy).
    Copy,
    /// Ctrl+V (paste).
    Paste,
    /// Ctrl+X (cut).
    Cut,
    /// Ctrl+Backspace (delete word left).
    DeleteWordLeft,
    /// Ctrl+Delete (delete word right).
    DeleteWordRight,
    /// Enter/Return key.
    Enter,
    /// Escape key.
    Escape,
    /// Tab key.
    Tab,
}

/// Modifier keys for input handling.
#[derive(Debug, Clone, Copy, Default)]
pub struct InputModifiers {
    /// Shift key is held.
    pub shift: bool,
    /// Control key is held.
    pub ctrl: bool,
    /// Alt/Option key is held.
    pub alt: bool,
}

impl InputModifiers {
    /// Creates new modifiers.
    pub const fn new(shift: bool, ctrl: bool, alt: bool) -> Self {
        Self { shift, ctrl, alt }
    }

    /// Returns true if only shift is held.
    pub const fn shift_only(&self) -> bool {
        self.shift && !self.ctrl && !self.alt
    }

    /// Returns true if only ctrl is held.
    pub const fn ctrl_only(&self) -> bool {
        self.ctrl && !self.shift && !self.alt
    }
}

/// A single-line text input widget.
///
/// # Example
///
/// ```ignore
/// let input = Input::builder()
///     .placeholder("Enter your name")
///     .value("John")
///     .on_change(|change| println!("Value: {}", change.value))
///     .build();
/// ```
pub struct Input {
    /// The current text content.
    value: String,
    /// Placeholder text shown when empty.
    placeholder: String,
    /// Whether this is a password field.
    password_mode: bool,
    /// Cursor and selection state.
    cursor: LineCursor,
    /// Horizontal scroll offset (in display columns).
    scroll_offset: usize,
    /// Maximum length (in graphemes), 0 for unlimited.
    max_length: usize,
    /// Whether the input is focused.
    focused: bool,
    /// Whether the input is disabled.
    disabled: bool,
    /// Whether the input is read-only.
    readonly: bool,
    /// Visual width of the input field.
    width: usize,
    /// Styling options.
    style: InputStyle,
    /// Change callback.
    on_change: Option<Box<dyn Fn(&InputChange) + Send + Sync>>,
    /// Submit callback (on Enter).
    on_submit: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Input")
            .field("value", &self.value)
            .field("placeholder", &self.placeholder)
            .field("password_mode", &self.password_mode)
            .field("cursor", &self.cursor)
            .field("scroll_offset", &self.scroll_offset)
            .field("max_length", &self.max_length)
            .field("focused", &self.focused)
            .field("disabled", &self.disabled)
            .field("readonly", &self.readonly)
            .field("width", &self.width)
            .field("style", &self.style)
            .field("on_change", &self.on_change.as_ref().map(|_| "<callback>"))
            .field("on_submit", &self.on_submit.as_ref().map(|_| "<callback>"))
            .finish()
    }
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    /// Creates a new empty input widget.
    pub fn new() -> Self {
        Self {
            value: String::new(),
            placeholder: String::new(),
            password_mode: false,
            cursor: LineCursor::new(),
            scroll_offset: 0,
            max_length: 0,
            focused: false,
            disabled: false,
            readonly: false,
            width: 20,
            style: InputStyle::default(),
            on_change: None,
            on_submit: None,
        }
    }

    /// Creates a builder for constructing an Input widget.
    pub fn builder() -> InputBuilder {
        InputBuilder::new()
    }

    /// Returns the current text value.
    #[inline]
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Sets the text value.
    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor.update_text(&self.value);
        self.ensure_cursor_visible();
    }

    /// Returns the placeholder text.
    #[inline]
    pub fn placeholder(&self) -> &str {
        &self.placeholder
    }

    /// Sets the placeholder text.
    pub fn set_placeholder(&mut self, placeholder: impl Into<String>) {
        self.placeholder = placeholder.into();
    }

    /// Returns true if password mode is enabled.
    #[inline]
    pub fn is_password_mode(&self) -> bool {
        self.password_mode
    }

    /// Sets password mode.
    pub fn set_password_mode(&mut self, enabled: bool) {
        self.password_mode = enabled;
    }

    /// Returns the cursor position (grapheme index).
    #[inline]
    pub fn cursor_position(&self) -> usize {
        self.cursor.col()
    }

    /// Sets the cursor position.
    pub fn set_cursor_position(&mut self, pos: usize) {
        self.cursor.set_position(pos, false);
        self.ensure_cursor_visible();
    }

    /// Returns true if the input is focused.
    #[inline]
    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// Sets the focused state.
    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            self.ensure_cursor_visible();
        }
    }

    /// Returns true if the input is disabled.
    #[inline]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Sets the disabled state.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    /// Returns true if the input is read-only.
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Sets the read-only state.
    pub fn set_readonly(&mut self, readonly: bool) {
        self.readonly = readonly;
    }

    /// Returns the visual width of the input field.
    #[inline]
    pub fn width(&self) -> usize {
        self.width
    }

    /// Sets the visual width of the input field.
    pub fn set_width(&mut self, width: usize) {
        self.width = width.max(1);
        self.ensure_cursor_visible();
    }

    /// Returns the current selection.
    #[inline]
    pub fn selection(&self) -> &Selection {
        self.cursor.selection()
    }

    /// Returns true if there is an active selection.
    #[inline]
    pub fn has_selection(&self) -> bool {
        self.cursor.has_selection()
    }

    /// Returns the selected text, if any.
    pub fn selected_text(&self) -> Option<&str> {
        self.cursor.get_selected_text(&self.value)
    }

    /// Selects all text.
    pub fn select_all(&mut self) {
        self.cursor.select_all();
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self) {
        self.cursor.clear_selection();
    }

    /// Returns the text to display (masked if password mode).
    pub fn display_text(&self) -> String {
        if self.value.is_empty() {
            return String::new();
        }

        if self.password_mode {
            PASSWORD_MASK_CHAR
                .to_string()
                .repeat(self.value.graphemes(true).count())
        } else {
            self.value.clone()
        }
    }

    /// Returns the visible portion of the text based on scroll offset.
    pub fn visible_text(&self) -> VisibleText {
        let display = self.display_text();

        if display.is_empty() {
            return VisibleText {
                text: String::new(),
                cursor_col: 0,
                selection_start: None,
                selection_end: None,
            };
        }

        let graphemes: Vec<&str> = display.graphemes(true).collect();
        let total_graphemes = graphemes.len();

        // Calculate visible range
        let visible_start = self.scroll_offset;
        let mut visible_width = 0;
        let mut visible_end = visible_start;

        for i in visible_start..total_graphemes {
            let g_width = graphemes[i].width();
            if visible_width + g_width > self.width {
                break;
            }
            visible_width += g_width;
            visible_end = i + 1;
        }

        // Build visible text
        let text: String = graphemes[visible_start..visible_end].concat();

        // Calculate cursor position within visible area
        let cursor_col = if self.cursor.col() >= visible_start && self.cursor.col() <= visible_end {
            let cursor_offset = self.cursor.col() - visible_start;
            graphemes[visible_start..visible_start + cursor_offset]
                .iter()
                .map(|g| g.width())
                .sum()
        } else if self.cursor.col() < visible_start {
            0
        } else {
            visible_width
        };

        // Calculate selection range within visible area
        let (selection_start, selection_end) = if self.has_selection() {
            let sel = self.cursor.selection();
            let sel_start = sel.start().col;
            let sel_end = sel.end().col;

            let vis_sel_start = if sel_start >= visible_start && sel_start <= visible_end {
                let offset = sel_start - visible_start;
                Some(
                    graphemes[visible_start..visible_start + offset]
                        .iter()
                        .map(|g| g.width())
                        .sum(),
                )
            } else if sel_start < visible_start {
                Some(0)
            } else {
                None
            };

            let vis_sel_end = if sel_end >= visible_start && sel_end <= visible_end {
                let offset = sel_end - visible_start;
                Some(
                    graphemes[visible_start..visible_start + offset]
                        .iter()
                        .map(|g| g.width())
                        .sum(),
                )
            } else if sel_end > visible_end {
                Some(visible_width)
            } else {
                None
            };

            (vis_sel_start, vis_sel_end)
        } else {
            (None, None)
        };

        VisibleText {
            text,
            cursor_col,
            selection_start,
            selection_end,
        }
    }

    /// Handles a key event and returns true if the event was handled.
    pub fn handle_key(&mut self, key: InputKey, modifiers: InputModifiers) -> bool {
        if self.disabled {
            return false;
        }

        match key {
            InputKey::Char(c) => {
                if !self.readonly {
                    self.insert_char(c);
                    return true;
                }
            }
            InputKey::Backspace => {
                if !self.readonly {
                    self.delete_backward();
                    return true;
                }
            }
            InputKey::Delete => {
                if !self.readonly {
                    self.delete_forward();
                    return true;
                }
            }
            InputKey::Left => {
                self.move_cursor(CursorMove::Left, modifiers.shift);
                return true;
            }
            InputKey::Right => {
                self.move_cursor(CursorMove::Right, modifiers.shift);
                return true;
            }
            InputKey::Home => {
                self.move_cursor(CursorMove::Home, modifiers.shift);
                return true;
            }
            InputKey::End => {
                self.move_cursor(CursorMove::End, modifiers.shift);
                return true;
            }
            InputKey::CtrlLeft => {
                self.move_cursor(CursorMove::WordLeft, modifiers.shift);
                return true;
            }
            InputKey::CtrlRight => {
                self.move_cursor(CursorMove::WordRight, modifiers.shift);
                return true;
            }
            InputKey::SelectAll => {
                self.select_all();
                return true;
            }
            InputKey::Copy => {
                // Copy is handled externally; just return true to indicate handled
                return self.has_selection();
            }
            InputKey::Paste => {
                // Paste data should be provided via insert_text
                return !self.readonly;
            }
            InputKey::Cut => {
                if !self.readonly && self.has_selection() {
                    self.delete_selection();
                    return true;
                }
            }
            InputKey::DeleteWordLeft => {
                if !self.readonly {
                    self.delete_word_backward();
                    return true;
                }
            }
            InputKey::DeleteWordRight => {
                if !self.readonly {
                    self.delete_word_forward();
                    return true;
                }
            }
            InputKey::Enter => {
                if let Some(ref callback) = self.on_submit {
                    callback(&self.value);
                }
                return true;
            }
            InputKey::Escape | InputKey::Tab => {
                // These are typically handled by the parent container
                return false;
            }
        }

        false
    }

    /// Inserts a single character at the cursor position.
    pub fn insert_char(&mut self, c: char) {
        if self.readonly || self.disabled {
            return;
        }

        // Check max length
        let current_len = self.value.graphemes(true).count();
        let selection_len = if self.has_selection() {
            let sel = self.cursor.selection();
            sel.end().col - sel.start().col
        } else {
            0
        };

        if self.max_length > 0 && current_len - selection_len >= self.max_length {
            return;
        }

        // Delete selection if any
        if self.has_selection() {
            self.delete_selection();
        }

        // Insert character
        let byte_offset = grapheme_to_byte_offset(&self.value, self.cursor.col());
        self.value.insert(byte_offset, c);
        self.cursor.update_text(&self.value);
        self.cursor.set_position(self.cursor.col() + 1, false);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Inserts text at the cursor position (for paste operations).
    pub fn insert_text(&mut self, text: &str) {
        if self.readonly || self.disabled || text.is_empty() {
            return;
        }

        // Filter to single line
        let text = text.lines().next().unwrap_or("");
        if text.is_empty() {
            return;
        }

        // Check max length
        let text_len = text.graphemes(true).count();
        let current_len = self.value.graphemes(true).count();
        let selection_len = if self.has_selection() {
            let sel = self.cursor.selection();
            sel.end().col - sel.start().col
        } else {
            0
        };

        let available = if self.max_length > 0 {
            self.max_length.saturating_sub(current_len - selection_len)
        } else {
            text_len
        };

        if available == 0 {
            return;
        }

        // Truncate if needed
        let insert_text: String = text.graphemes(true).take(available).collect();
        let insert_len = insert_text.graphemes(true).count();

        // Delete selection if any
        if self.has_selection() {
            self.delete_selection();
        }

        // Insert text
        let byte_offset = grapheme_to_byte_offset(&self.value, self.cursor.col());
        self.value.insert_str(byte_offset, &insert_text);
        self.cursor.update_text(&self.value);
        self.cursor
            .set_position(self.cursor.col() + insert_len, false);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Deletes the character before the cursor (backspace).
    pub fn delete_backward(&mut self) {
        if self.readonly || self.disabled {
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }

        if self.cursor.col() == 0 {
            return;
        }

        let new_cursor_pos = self.cursor.col() - 1;
        let start_byte = grapheme_to_byte_offset(&self.value, new_cursor_pos);
        let end_byte = grapheme_to_byte_offset(&self.value, self.cursor.col());

        self.value.replace_range(start_byte..end_byte, "");
        self.cursor.update_text(&self.value);
        self.cursor.set_position(new_cursor_pos, false);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Deletes the character after the cursor (delete).
    pub fn delete_forward(&mut self) {
        if self.readonly || self.disabled {
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }

        let len = self.value.graphemes(true).count();
        if self.cursor.col() >= len {
            return;
        }

        let start_byte = grapheme_to_byte_offset(&self.value, self.cursor.col());
        let end_byte = grapheme_to_byte_offset(&self.value, self.cursor.col() + 1);

        self.value.replace_range(start_byte..end_byte, "");
        self.cursor.update_text(&self.value);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Deletes the word before the cursor.
    pub fn delete_word_backward(&mut self) {
        if self.readonly || self.disabled {
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }

        if self.cursor.col() == 0 {
            return;
        }

        // Find word start
        let cursor_col = self.cursor.col();
        self.cursor
            .move_cursor(&self.value, CursorMove::WordLeft, false);
        let word_start = self.cursor.col();

        // Delete from word start to original cursor
        let start_byte = grapheme_to_byte_offset(&self.value, word_start);
        let end_byte = grapheme_to_byte_offset(&self.value, cursor_col);

        self.value.replace_range(start_byte..end_byte, "");
        self.cursor.update_text(&self.value);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Deletes the word after the cursor.
    pub fn delete_word_forward(&mut self) {
        if self.readonly || self.disabled {
            return;
        }

        if self.has_selection() {
            self.delete_selection();
            return;
        }

        let len = self.value.graphemes(true).count();
        if self.cursor.col() >= len {
            return;
        }

        // Find word end
        let cursor_col = self.cursor.col();
        self.cursor
            .move_cursor(&self.value, CursorMove::WordRight, false);
        let word_end = self.cursor.col();

        // Restore cursor position
        self.cursor.set_position(cursor_col, false);

        // Delete from cursor to word end
        let start_byte = grapheme_to_byte_offset(&self.value, cursor_col);
        let end_byte = grapheme_to_byte_offset(&self.value, word_end);

        self.value.replace_range(start_byte..end_byte, "");
        self.cursor.update_text(&self.value);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Deletes the selected text.
    fn delete_selection(&mut self) {
        if !self.has_selection() {
            return;
        }

        let sel = self.cursor.selection();
        let start = sel.start().col;
        let end = sel.end().col;

        let start_byte = grapheme_to_byte_offset(&self.value, start);
        let end_byte = grapheme_to_byte_offset(&self.value, end);

        self.value.replace_range(start_byte..end_byte, "");
        self.cursor.update_text(&self.value);
        self.cursor.set_position(start, false);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Moves the cursor.
    fn move_cursor(&mut self, movement: CursorMove, extend_selection: bool) {
        // If not extending and there's a selection, collapse it first
        if !extend_selection && self.has_selection() {
            let sel = self.cursor.selection();
            let pos = match movement {
                CursorMove::Left | CursorMove::Home | CursorMove::WordLeft => sel.start().col,
                _ => sel.end().col,
            };
            self.cursor.set_position(pos, false);
            if !matches!(movement, CursorMove::Left | CursorMove::Right) {
                self.cursor.move_cursor(&self.value, movement, false);
            }
        } else {
            self.cursor
                .move_cursor(&self.value, movement, extend_selection);
        }
        self.ensure_cursor_visible();
    }

    /// Ensures the cursor is visible by adjusting scroll offset.
    fn ensure_cursor_visible(&mut self) {
        if self.width == 0 {
            return;
        }

        let display = self.display_text();
        let graphemes: Vec<&str> = display.graphemes(true).collect();

        // Calculate cursor column position in display width
        let cursor_display_col: usize = graphemes[..self.cursor.col().min(graphemes.len())]
            .iter()
            .map(|g| g.width())
            .sum();

        // Calculate visible range width
        let visible_start_width: usize = graphemes[..self.scroll_offset.min(graphemes.len())]
            .iter()
            .map(|g| g.width())
            .sum();

        // Scroll left if cursor is before visible area
        if cursor_display_col < visible_start_width {
            // Find the scroll offset that puts cursor at start
            self.scroll_offset = 0;
            let mut width = 0;
            for (i, g) in graphemes.iter().enumerate() {
                if width >= cursor_display_col {
                    self.scroll_offset = i;
                    break;
                }
                width += g.width();
            }
        }

        // Scroll right if cursor is after visible area
        if cursor_display_col >= visible_start_width + self.width {
            // Find the scroll offset that puts cursor at end
            let target = cursor_display_col.saturating_sub(self.width) + 1;
            self.scroll_offset = 0;
            let mut width = 0;
            for (i, g) in graphemes.iter().enumerate() {
                if width >= target {
                    self.scroll_offset = i;
                    break;
                }
                width += g.width();
                self.scroll_offset = i + 1;
            }
        }
    }

    /// Emits a change event.
    fn emit_change(&self) {
        if let Some(ref callback) = self.on_change {
            callback(&InputChange {
                value: self.value.clone(),
                cursor_position: self.cursor.col(),
            });
        }
    }
}

/// Visible text data for rendering.
#[derive(Debug, Clone)]
pub struct VisibleText {
    /// The visible text content.
    pub text: String,
    /// Cursor column position within visible area (in display columns).
    pub cursor_col: usize,
    /// Selection start column within visible area, if any.
    pub selection_start: Option<usize>,
    /// Selection end column within visible area, if any.
    pub selection_end: Option<usize>,
}

/// Builder for creating Input widgets.
#[derive(Default)]
pub struct InputBuilder {
    value: String,
    placeholder: String,
    password_mode: bool,
    max_length: usize,
    width: usize,
    disabled: bool,
    readonly: bool,
    style: InputStyle,
    on_change: Option<Box<dyn Fn(&InputChange) + Send + Sync>>,
    on_submit: Option<Box<dyn Fn(&str) + Send + Sync>>,
}

impl InputBuilder {
    /// Creates a new input builder.
    pub fn new() -> Self {
        Self {
            width: 20,
            style: InputStyle::default(),
            ..Default::default()
        }
    }

    /// Sets the initial value.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Sets the placeholder text.
    pub fn placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Enables password mode.
    pub fn password(mut self) -> Self {
        self.password_mode = true;
        self
    }

    /// Sets password mode.
    pub fn password_mode(mut self, enabled: bool) -> Self {
        self.password_mode = enabled;
        self
    }

    /// Sets the maximum length.
    pub fn max_length(mut self, max: usize) -> Self {
        self.max_length = max;
        self
    }

    /// Sets the visual width.
    pub fn width(mut self, width: usize) -> Self {
        self.width = width.max(1);
        self
    }

    /// Sets the disabled state.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Sets the read-only state.
    pub fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    /// Sets the text color.
    pub fn text_color(mut self, r: u8, g: u8, b: u8) -> Self {
        self.style.text_color = Some((r, g, b));
        self
    }

    /// Sets the placeholder color.
    pub fn placeholder_color(mut self, r: u8, g: u8, b: u8) -> Self {
        self.style.placeholder_color = Some((r, g, b));
        self
    }

    /// Sets the cursor color.
    pub fn cursor_color(mut self, r: u8, g: u8, b: u8) -> Self {
        self.style.cursor_color = Some((r, g, b));
        self
    }

    /// Sets the selection background color.
    pub fn selection_bg(mut self, r: u8, g: u8, b: u8) -> Self {
        self.style.selection_bg = Some((r, g, b));
        self
    }

    /// Sets the style.
    pub fn style(mut self, style: InputStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the change callback.
    pub fn on_change<F>(mut self, callback: F) -> Self
    where
        F: Fn(&InputChange) + Send + Sync + 'static,
    {
        self.on_change = Some(Box::new(callback));
        self
    }

    /// Sets the submit callback (triggered on Enter).
    pub fn on_submit<F>(mut self, callback: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_submit = Some(Box::new(callback));
        self
    }

    /// Builds the Input widget.
    pub fn build(self) -> Input {
        let mut input = Input {
            value: self.value,
            placeholder: self.placeholder,
            password_mode: self.password_mode,
            cursor: LineCursor::new(),
            scroll_offset: 0,
            max_length: self.max_length,
            focused: false,
            disabled: self.disabled,
            readonly: self.readonly,
            width: self.width,
            style: self.style,
            on_change: self.on_change,
            on_submit: self.on_submit,
        };

        // Position cursor at end of initial value
        input.cursor.update_text(&input.value);
        input
            .cursor
            .set_position(input.value.graphemes(true).count(), false);

        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_insert() {
        let mut input = Input::new();
        input.insert_char('h');
        input.insert_char('e');
        input.insert_char('l');
        input.insert_char('l');
        input.insert_char('o');

        assert_eq!(input.value(), "hello");
        assert_eq!(input.cursor_position(), 5);
    }

    #[test]
    fn test_input_backspace() {
        let mut input = Input::builder().value("hello").build();

        input.delete_backward();
        assert_eq!(input.value(), "hell");
        assert_eq!(input.cursor_position(), 4);

        input.delete_backward();
        input.delete_backward();
        assert_eq!(input.value(), "he");
    }

    #[test]
    fn test_input_delete() {
        let mut input = Input::builder().value("hello").build();
        input.set_cursor_position(2);

        input.delete_forward();
        assert_eq!(input.value(), "helo");
        assert_eq!(input.cursor_position(), 2);
    }

    #[test]
    fn test_input_selection() {
        let mut input = Input::builder().value("hello world").build();

        input.select_all();
        assert!(input.has_selection());
        assert_eq!(input.selected_text(), Some("hello world"));

        input.insert_char('x');
        assert_eq!(input.value(), "x");
    }

    #[test]
    fn test_input_max_length() {
        let mut input = Input::builder().max_length(5).build();

        input.insert_text("hello world");
        assert_eq!(input.value(), "hello");
        assert_eq!(input.value().graphemes(true).count(), 5);
    }

    #[test]
    fn test_input_password_mode() {
        let input = Input::builder().value("secret").password().build();

        let display = input.display_text();
        assert_eq!(display, "••••••");
        assert_eq!(input.value(), "secret");
    }

    #[test]
    fn test_input_unicode() {
        let mut input = Input::new();
        input.insert_text("héllo 🌍");

        assert_eq!(input.value(), "héllo 🌍");
        // h + é + l + l + o + space + 🌍 = 7 graphemes
        assert_eq!(input.cursor_position(), 7);
    }

    #[test]
    fn test_input_cursor_movement() {
        let mut input = Input::builder().value("hello world").build();

        input.handle_key(InputKey::Home, InputModifiers::default());
        assert_eq!(input.cursor_position(), 0);

        input.handle_key(InputKey::End, InputModifiers::default());
        assert_eq!(input.cursor_position(), 11);

        input.handle_key(InputKey::Left, InputModifiers::default());
        assert_eq!(input.cursor_position(), 10);

        input.handle_key(InputKey::Right, InputModifiers::default());
        assert_eq!(input.cursor_position(), 11);
    }

    #[test]
    fn test_input_word_movement() {
        let mut input = Input::builder().value("hello world test").build();

        input.set_cursor_position(11); // End of "world"
        input.handle_key(InputKey::CtrlLeft, InputModifiers::default());
        assert_eq!(input.cursor_position(), 6); // Start of "world"

        input.handle_key(InputKey::CtrlRight, InputModifiers::default());
        assert_eq!(input.cursor_position(), 12); // After "world "
    }
}
