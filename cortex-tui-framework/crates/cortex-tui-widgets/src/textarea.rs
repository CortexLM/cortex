//! Multi-line text area widget.
//!
//! Provides a text editing area with:
//! - Multiple lines of text
//! - Cursor movement across lines
//! - Text selection (single and multi-line)
//! - Vertical scrolling
//! - Optional line numbers
//! - Optional word wrap
//! - Change callbacks

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::cursor::{grapheme_to_byte_offset, CursorMove, CursorPosition, Selection, TextCursor};

/// Wrap mode for text area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WrapMode {
    /// No wrapping - lines extend horizontally.
    #[default]
    None,
    /// Wrap at any character boundary.
    Char,
    /// Wrap at word boundaries when possible.
    Word,
}

/// Styling options for the TextArea widget.
#[derive(Debug, Clone, Copy)]
pub struct TextAreaStyle {
    /// Text color as RGB tuple.
    pub text_color: Option<(u8, u8, u8)>,
    /// Line number color as RGB tuple.
    pub line_number_color: Option<(u8, u8, u8)>,
    /// Cursor color as RGB tuple.
    pub cursor_color: Option<(u8, u8, u8)>,
    /// Selection background color as RGB tuple.
    pub selection_bg: Option<(u8, u8, u8)>,
    /// Current line highlight color as RGB tuple.
    pub current_line_bg: Option<(u8, u8, u8)>,
}

impl Default for TextAreaStyle {
    fn default() -> Self {
        Self {
            text_color: None,
            line_number_color: Some((100, 100, 100)),
            cursor_color: None,
            selection_bg: Some((60, 80, 120)),
            current_line_bg: Some((40, 40, 40)),
        }
    }
}

/// Change event data for the TextArea widget.
#[derive(Debug, Clone)]
pub struct TextAreaChange {
    /// The new text value (all lines joined).
    pub value: String,
    /// The cursor position after the change.
    pub cursor: CursorPosition,
    /// Total number of lines.
    pub line_count: usize,
}

/// Key event representation for handling input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAreaKey {
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
    /// Up arrow.
    Up,
    /// Down arrow.
    Down,
    /// Home key.
    Home,
    /// End key.
    End,
    /// Page Up.
    PageUp,
    /// Page Down.
    PageDown,
    /// Ctrl+Home (document start).
    DocumentStart,
    /// Ctrl+End (document end).
    DocumentEnd,
    /// Ctrl+Left (word left).
    WordLeft,
    /// Ctrl+Right (word right).
    WordRight,
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
    /// Tab key.
    Tab,
    /// Escape key.
    Escape,
}

/// Modifier keys for input handling.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextAreaModifiers {
    /// Shift key is held.
    pub shift: bool,
    /// Control key is held.
    pub ctrl: bool,
    /// Alt/Option key is held.
    pub alt: bool,
}

impl TextAreaModifiers {
    /// Creates new modifiers.
    pub const fn new(shift: bool, ctrl: bool, alt: bool) -> Self {
        Self { shift, ctrl, alt }
    }
}

/// A multi-line text editing widget.
///
/// # Example
///
/// ```ignore
/// let textarea = TextArea::builder()
///     .value("Hello\nWorld")
///     .line_numbers(true)
///     .wrap_mode(WrapMode::Word)
///     .on_change(|change| println!("Lines: {}", change.line_count))
///     .build();
/// ```
#[allow(dead_code)]
pub struct TextArea {
    /// Lines of text content.
    lines: Vec<String>,
    /// Cursor and selection state.
    cursor: TextCursor,
    /// Vertical scroll offset (in lines).
    scroll_row: usize,
    /// Horizontal scroll offset (in columns) - only used when wrap is None.
    scroll_col: usize,
    /// Whether line numbers are shown.
    show_line_numbers: bool,
    /// Line number gutter width.
    line_number_width: usize,
    /// Wrap mode.
    wrap_mode: WrapMode,
    /// Tab width in spaces.
    tab_width: usize,
    /// Whether the textarea is focused.
    focused: bool,
    /// Whether the textarea is disabled.
    disabled: bool,
    /// Whether the textarea is read-only.
    readonly: bool,
    /// Visual width of the text area.
    width: usize,
    /// Visual height of the text area.
    height: usize,
    /// Styling options.
    style: TextAreaStyle,
    /// Change callback.
    on_change: Option<Box<dyn Fn(&TextAreaChange) + Send + Sync>>,
}

impl Default for TextArea {
    fn default() -> Self {
        Self::new()
    }
}

impl TextArea {
    /// Creates a new empty text area.
    pub fn new() -> Self {
        Self {
            lines: vec![String::new()],
            cursor: TextCursor::new(),
            scroll_row: 0,
            scroll_col: 0,
            show_line_numbers: false,
            line_number_width: 0,
            wrap_mode: WrapMode::None,
            tab_width: 4,
            focused: false,
            disabled: false,
            readonly: false,
            width: 80,
            height: 24,
            style: TextAreaStyle::default(),
            on_change: None,
        }
    }

    /// Creates a builder for constructing a TextArea.
    pub fn builder() -> TextAreaBuilder {
        TextAreaBuilder::new()
    }

    /// Returns the text content as a single string with newlines.
    pub fn value(&self) -> String {
        self.lines.join("\n")
    }

    /// Returns a reference to the lines.
    #[inline]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Returns the number of lines.
    #[inline]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    /// Returns a specific line, if it exists.
    pub fn line(&self, row: usize) -> Option<&str> {
        self.lines.get(row).map(|s| s.as_str())
    }

    /// Sets the text content.
    pub fn set_value(&mut self, value: &str) {
        self.lines = value.lines().map(String::from).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.cursor.update_lines(&self.lines);
        self.update_line_number_width();
        self.ensure_cursor_visible();
    }

    /// Returns the cursor position.
    #[inline]
    pub fn cursor_position(&self) -> CursorPosition {
        self.cursor.position()
    }

    /// Sets the cursor position.
    pub fn set_cursor_position(&mut self, pos: CursorPosition) {
        self.cursor.set_position(pos, false);
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
    pub fn selected_text(&self) -> Option<String> {
        self.cursor.get_selected_text(&self.lines)
    }

    /// Selects all text.
    pub fn select_all(&mut self) {
        self.cursor.select_all();
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self) {
        self.cursor.clear_selection();
    }

    /// Returns true if line numbers are shown.
    #[inline]
    pub fn show_line_numbers(&self) -> bool {
        self.show_line_numbers
    }

    /// Sets whether line numbers are shown.
    pub fn set_show_line_numbers(&mut self, show: bool) {
        self.show_line_numbers = show;
        self.update_line_number_width();
    }

    /// Returns the wrap mode.
    #[inline]
    pub fn wrap_mode(&self) -> WrapMode {
        self.wrap_mode
    }

    /// Sets the wrap mode.
    pub fn set_wrap_mode(&mut self, mode: WrapMode) {
        self.wrap_mode = mode;
    }

    /// Returns true if the textarea is focused.
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

    /// Returns true if the textarea is disabled.
    #[inline]
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Sets the disabled state.
    pub fn set_disabled(&mut self, disabled: bool) {
        self.disabled = disabled;
    }

    /// Returns true if the textarea is read-only.
    #[inline]
    pub fn is_readonly(&self) -> bool {
        self.readonly
    }

    /// Sets the read-only state.
    pub fn set_readonly(&mut self, readonly: bool) {
        self.readonly = readonly;
    }

    /// Returns the visual dimensions.
    #[inline]
    pub fn dimensions(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    /// Sets the visual dimensions.
    pub fn set_dimensions(&mut self, width: usize, height: usize) {
        self.width = width.max(1);
        self.height = height.max(1);
        self.update_line_number_width();
        self.ensure_cursor_visible();
    }

    /// Returns the scroll position.
    #[inline]
    pub fn scroll_position(&self) -> (usize, usize) {
        (self.scroll_row, self.scroll_col)
    }

    /// Sets the scroll position.
    pub fn set_scroll_position(&mut self, row: usize, col: usize) {
        self.scroll_row = row.min(self.lines.len().saturating_sub(1));
        self.scroll_col = col;
    }

    /// Returns the content width (excluding line numbers).
    fn content_width(&self) -> usize {
        if self.show_line_numbers {
            self.width.saturating_sub(self.line_number_width + 1) // +1 for separator
        } else {
            self.width
        }
    }

    /// Updates the line number gutter width based on line count.
    fn update_line_number_width(&mut self) {
        if self.show_line_numbers {
            let digits = (self.lines.len() as f64).log10().floor() as usize + 1;
            self.line_number_width = digits.max(2) + 1; // padding
        } else {
            self.line_number_width = 0;
        }
    }

    /// Returns visible lines information for rendering.
    pub fn visible_lines(&self) -> VisibleLines {
        let content_width = self.content_width();
        let visible_end = (self.scroll_row + self.height).min(self.lines.len());

        let mut visible = Vec::with_capacity(self.height);

        for row in self.scroll_row..visible_end {
            let line = &self.lines[row];
            let line_number = if self.show_line_numbers {
                Some(row + 1)
            } else {
                None
            };

            // Handle wrapping if enabled
            let wrapped_lines = match self.wrap_mode {
                WrapMode::None => {
                    // No wrapping - just truncate/scroll
                    let display = if self.scroll_col > 0 {
                        let graphemes: Vec<&str> = line.graphemes(true).collect();
                        let start = self.scroll_col.min(graphemes.len());
                        graphemes[start..].concat()
                    } else {
                        line.clone()
                    };
                    vec![display]
                }
                WrapMode::Char => wrap_line_char(line, content_width),
                WrapMode::Word => wrap_line_word(line, content_width),
            };

            // Calculate cursor position within this line
            let cursor_info = if row == self.cursor.row() {
                let cursor_col = self.cursor.col();
                Some(CursorInfo {
                    col: cursor_col.saturating_sub(self.scroll_col),
                    visible: cursor_col >= self.scroll_col
                        && cursor_col < self.scroll_col + content_width,
                })
            } else {
                None
            };

            // Calculate selection ranges for this line
            let selection_range = if self.has_selection() {
                let sel = self.cursor.selection();
                let start = sel.start();
                let end = sel.end();

                if row >= start.row && row <= end.row {
                    let line_len = line.graphemes(true).count();
                    let sel_start = if row == start.row { start.col } else { 0 };
                    let sel_end = if row == end.row { end.col } else { line_len };
                    Some((sel_start, sel_end))
                } else {
                    None
                }
            } else {
                None
            };

            visible.push(VisibleLine {
                row,
                line_number,
                content: wrapped_lines,
                cursor: cursor_info,
                selection: selection_range,
                is_current_line: row == self.cursor.row(),
            });
        }

        VisibleLines {
            lines: visible,
            line_number_width: self.line_number_width,
            scroll_row: self.scroll_row,
            scroll_col: self.scroll_col,
            total_lines: self.lines.len(),
            content_width,
        }
    }

    /// Handles a key event and returns true if the event was handled.
    pub fn handle_key(&mut self, key: TextAreaKey, modifiers: TextAreaModifiers) -> bool {
        if self.disabled {
            return false;
        }

        match key {
            TextAreaKey::Char(c) => {
                if !self.readonly {
                    self.insert_char(c);
                    return true;
                }
            }
            TextAreaKey::Tab => {
                if !self.readonly {
                    self.insert_tab();
                    return true;
                }
            }
            TextAreaKey::Enter => {
                if !self.readonly {
                    self.insert_newline();
                    return true;
                }
            }
            TextAreaKey::Backspace => {
                if !self.readonly {
                    self.delete_backward();
                    return true;
                }
            }
            TextAreaKey::Delete => {
                if !self.readonly {
                    self.delete_forward();
                    return true;
                }
            }
            TextAreaKey::Left => {
                self.move_cursor(CursorMove::Left, modifiers.shift);
                return true;
            }
            TextAreaKey::Right => {
                self.move_cursor(CursorMove::Right, modifiers.shift);
                return true;
            }
            TextAreaKey::Up => {
                self.move_cursor(CursorMove::Up, modifiers.shift);
                return true;
            }
            TextAreaKey::Down => {
                self.move_cursor(CursorMove::Down, modifiers.shift);
                return true;
            }
            TextAreaKey::Home => {
                self.move_cursor(CursorMove::Home, modifiers.shift);
                return true;
            }
            TextAreaKey::End => {
                self.move_cursor(CursorMove::End, modifiers.shift);
                return true;
            }
            TextAreaKey::PageUp => {
                self.move_cursor(CursorMove::PageUp(self.height), modifiers.shift);
                return true;
            }
            TextAreaKey::PageDown => {
                self.move_cursor(CursorMove::PageDown(self.height), modifiers.shift);
                return true;
            }
            TextAreaKey::DocumentStart => {
                self.move_cursor(CursorMove::DocumentStart, modifiers.shift);
                return true;
            }
            TextAreaKey::DocumentEnd => {
                self.move_cursor(CursorMove::DocumentEnd, modifiers.shift);
                return true;
            }
            TextAreaKey::WordLeft => {
                self.move_cursor(CursorMove::WordLeft, modifiers.shift);
                return true;
            }
            TextAreaKey::WordRight => {
                self.move_cursor(CursorMove::WordRight, modifiers.shift);
                return true;
            }
            TextAreaKey::SelectAll => {
                self.select_all();
                return true;
            }
            TextAreaKey::Copy => {
                return self.has_selection();
            }
            TextAreaKey::Paste => {
                return !self.readonly;
            }
            TextAreaKey::Cut => {
                if !self.readonly && self.has_selection() {
                    self.delete_selection();
                    return true;
                }
            }
            TextAreaKey::DeleteWordLeft => {
                if !self.readonly {
                    self.delete_word_backward();
                    return true;
                }
            }
            TextAreaKey::DeleteWordRight => {
                if !self.readonly {
                    self.delete_word_forward();
                    return true;
                }
            }
            TextAreaKey::Escape => {
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

        // Delete selection if any
        if self.has_selection() {
            self.delete_selection();
        }

        let row = self.cursor.row();
        let col = self.cursor.col();

        if row < self.lines.len() {
            let byte_offset = grapheme_to_byte_offset(&self.lines[row], col);
            self.lines[row].insert(byte_offset, c);
        }

        self.cursor.update_lines(&self.lines);
        self.cursor
            .set_position(CursorPosition::new(row, col + 1), false);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Inserts a tab character or spaces.
    pub fn insert_tab(&mut self) {
        if self.readonly || self.disabled {
            return;
        }

        // Delete selection if any
        if self.has_selection() {
            self.delete_selection();
        }

        let row = self.cursor.row();
        let col = self.cursor.col();

        // Calculate spaces to next tab stop
        let spaces_to_tab = self.tab_width - (col % self.tab_width);
        let spaces: String = " ".repeat(spaces_to_tab);

        if row < self.lines.len() {
            let byte_offset = grapheme_to_byte_offset(&self.lines[row], col);
            self.lines[row].insert_str(byte_offset, &spaces);
        }

        self.cursor.update_lines(&self.lines);
        self.cursor
            .set_position(CursorPosition::new(row, col + spaces_to_tab), false);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Inserts a newline at the cursor position.
    pub fn insert_newline(&mut self) {
        if self.readonly || self.disabled {
            return;
        }

        // Delete selection if any
        if self.has_selection() {
            self.delete_selection();
        }

        let row = self.cursor.row();
        let col = self.cursor.col();

        if row < self.lines.len() {
            let byte_offset = grapheme_to_byte_offset(&self.lines[row], col);
            let rest = self.lines[row].split_off(byte_offset);
            self.lines.insert(row + 1, rest);
        } else {
            self.lines.push(String::new());
        }

        self.cursor.update_lines(&self.lines);
        self.cursor
            .set_position(CursorPosition::new(row + 1, 0), false);
        self.update_line_number_width();
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Inserts text at the cursor position (for paste operations).
    pub fn insert_text(&mut self, text: &str) {
        if self.readonly || self.disabled || text.is_empty() {
            return;
        }

        // Delete selection if any
        if self.has_selection() {
            self.delete_selection();
        }

        let lines_to_insert: Vec<&str> = text.lines().collect();
        if lines_to_insert.is_empty() {
            return;
        }

        let row = self.cursor.row();
        let col = self.cursor.col();

        if row >= self.lines.len() {
            self.lines.push(String::new());
        }

        let byte_offset = grapheme_to_byte_offset(&self.lines[row], col);
        let rest = self.lines[row].split_off(byte_offset);

        // Insert first line at cursor
        self.lines[row].push_str(lines_to_insert[0]);

        // Insert middle lines
        for (i, line) in lines_to_insert.iter().enumerate().skip(1) {
            self.lines.insert(row + i, line.to_string());
        }

        // Append rest to last inserted line
        let last_row = row + lines_to_insert.len() - 1;
        let last_col = self.lines[last_row].graphemes(true).count();
        self.lines[last_row].push_str(&rest);

        self.cursor.update_lines(&self.lines);
        self.cursor
            .set_position(CursorPosition::new(last_row, last_col), false);
        self.update_line_number_width();
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

        let row = self.cursor.row();
        let col = self.cursor.col();

        if col > 0 {
            // Delete character on current line
            let start_byte = grapheme_to_byte_offset(&self.lines[row], col - 1);
            let end_byte = grapheme_to_byte_offset(&self.lines[row], col);
            self.lines[row].replace_range(start_byte..end_byte, "");

            self.cursor.update_lines(&self.lines);
            self.cursor
                .set_position(CursorPosition::new(row, col - 1), false);
        } else if row > 0 {
            // Join with previous line
            let current_line = self.lines.remove(row);
            let prev_col = self.lines[row - 1].graphemes(true).count();
            self.lines[row - 1].push_str(&current_line);

            self.cursor.update_lines(&self.lines);
            self.cursor
                .set_position(CursorPosition::new(row - 1, prev_col), false);
            self.update_line_number_width();
        }

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

        let row = self.cursor.row();
        let col = self.cursor.col();
        let line_len = self
            .lines
            .get(row)
            .map(|l| l.graphemes(true).count())
            .unwrap_or(0);

        if col < line_len {
            // Delete character on current line
            let start_byte = grapheme_to_byte_offset(&self.lines[row], col);
            let end_byte = grapheme_to_byte_offset(&self.lines[row], col + 1);
            self.lines[row].replace_range(start_byte..end_byte, "");

            self.cursor.update_lines(&self.lines);
        } else if row < self.lines.len() - 1 {
            // Join with next line
            let next_line = self.lines.remove(row + 1);
            self.lines[row].push_str(&next_line);

            self.cursor.update_lines(&self.lines);
            self.update_line_number_width();
        }

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

        let row = self.cursor.row();
        let col = self.cursor.col();

        if col == 0 && row > 0 {
            // At start of line - join with previous
            self.delete_backward();
            return;
        }

        if col == 0 {
            return;
        }

        // Find word start
        self.cursor
            .move_cursor(&self.lines, CursorMove::WordLeft, false);
        let word_start = self.cursor.col();

        // Delete from word start to original position
        let start_byte = grapheme_to_byte_offset(&self.lines[row], word_start);
        let end_byte = grapheme_to_byte_offset(&self.lines[row], col);
        self.lines[row].replace_range(start_byte..end_byte, "");

        self.cursor.update_lines(&self.lines);
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

        let row = self.cursor.row();
        let col = self.cursor.col();
        let line_len = self
            .lines
            .get(row)
            .map(|l| l.graphemes(true).count())
            .unwrap_or(0);

        if col >= line_len && row < self.lines.len() - 1 {
            // At end of line - join with next
            self.delete_forward();
            return;
        }

        if col >= line_len {
            return;
        }

        // Find word end
        let original_col = col;
        self.cursor
            .move_cursor(&self.lines, CursorMove::WordRight, false);
        let word_end = self.cursor.col();

        // Restore cursor
        self.cursor
            .set_position(CursorPosition::new(row, original_col), false);

        // Delete from cursor to word end
        let start_byte = grapheme_to_byte_offset(&self.lines[row], col);
        let end_byte = grapheme_to_byte_offset(&self.lines[row], word_end);
        self.lines[row].replace_range(start_byte..end_byte, "");

        self.cursor.update_lines(&self.lines);
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Deletes the selected text.
    pub fn delete_selection(&mut self) {
        if !self.has_selection() {
            return;
        }

        let sel = self.cursor.selection();
        let start = sel.start();
        let end = sel.end();

        if start.row == end.row {
            // Single line selection
            let start_byte = grapheme_to_byte_offset(&self.lines[start.row], start.col);
            let end_byte = grapheme_to_byte_offset(&self.lines[start.row], end.col);
            self.lines[start.row].replace_range(start_byte..end_byte, "");
        } else {
            // Multi-line selection
            // Truncate first line
            let start_byte = grapheme_to_byte_offset(&self.lines[start.row], start.col);
            self.lines[start.row].truncate(start_byte);

            // Get the end of the last line
            let end_byte = grapheme_to_byte_offset(&self.lines[end.row], end.col);
            let rest = self.lines[end.row][end_byte..].to_string();

            // Append rest to first line
            self.lines[start.row].push_str(&rest);

            // Remove lines in between
            for _ in start.row + 1..=end.row {
                self.lines.remove(start.row + 1);
            }
        }

        self.cursor.update_lines(&self.lines);
        self.cursor.set_position(start, false);
        self.update_line_number_width();
        self.ensure_cursor_visible();
        self.emit_change();
    }

    /// Moves the cursor.
    fn move_cursor(&mut self, movement: CursorMove, extend_selection: bool) {
        // If not extending and there's a selection, collapse it first for horizontal movement
        if !extend_selection && self.has_selection() {
            let sel = self.cursor.selection();
            let pos = match movement {
                CursorMove::Left
                | CursorMove::Home
                | CursorMove::WordLeft
                | CursorMove::DocumentStart => sel.start(),
                CursorMove::Right
                | CursorMove::End
                | CursorMove::WordRight
                | CursorMove::DocumentEnd => sel.end(),
                _ => sel.start(), // For vertical movement, use start
            };
            self.cursor.set_position(pos, false);

            // For simple left/right, we're done after collapsing
            if matches!(movement, CursorMove::Left | CursorMove::Right) {
                self.ensure_cursor_visible();
                return;
            }
        }

        self.cursor
            .move_cursor(&self.lines, movement, extend_selection);
        self.ensure_cursor_visible();
    }

    /// Ensures the cursor is visible by adjusting scroll offset.
    fn ensure_cursor_visible(&mut self) {
        let row = self.cursor.row();
        let col = self.cursor.col();

        // Vertical scrolling
        if row < self.scroll_row {
            self.scroll_row = row;
        } else if row >= self.scroll_row + self.height {
            self.scroll_row = row.saturating_sub(self.height - 1);
        }

        // Horizontal scrolling (only when wrap is None)
        if self.wrap_mode == WrapMode::None {
            let content_width = self.content_width();

            if col < self.scroll_col {
                self.scroll_col = col;
            } else if col >= self.scroll_col + content_width {
                self.scroll_col = col.saturating_sub(content_width - 1);
            }
        }
    }

    /// Emits a change event.
    fn emit_change(&self) {
        if let Some(ref callback) = self.on_change {
            callback(&TextAreaChange {
                value: self.value(),
                cursor: self.cursor.position(),
                line_count: self.lines.len(),
            });
        }
    }

    /// Scrolls the view by the given number of lines.
    pub fn scroll(&mut self, delta: isize) {
        if delta < 0 {
            self.scroll_row = self.scroll_row.saturating_sub((-delta) as usize);
        } else {
            let max_scroll = self.lines.len().saturating_sub(1);
            self.scroll_row = (self.scroll_row + delta as usize).min(max_scroll);
        }
    }
}

/// Information about a visible line for rendering.
#[derive(Debug, Clone)]
pub struct VisibleLine {
    /// Original line index (0-based).
    pub row: usize,
    /// Line number to display (1-based), if line numbers are enabled.
    pub line_number: Option<usize>,
    /// Content lines (may be multiple if wrapped).
    pub content: Vec<String>,
    /// Cursor information, if cursor is on this line.
    pub cursor: Option<CursorInfo>,
    /// Selection range on this line (start_col, end_col), if any.
    pub selection: Option<(usize, usize)>,
    /// Whether this is the current line (cursor's line).
    pub is_current_line: bool,
}

/// Cursor information for rendering.
#[derive(Debug, Clone, Copy)]
pub struct CursorInfo {
    /// Column position (grapheme index).
    pub col: usize,
    /// Whether the cursor is visible in the current viewport.
    pub visible: bool,
}

/// Collection of visible lines for rendering.
#[derive(Debug, Clone)]
pub struct VisibleLines {
    /// The visible lines.
    pub lines: Vec<VisibleLine>,
    /// Width of the line number gutter.
    pub line_number_width: usize,
    /// Current vertical scroll offset.
    pub scroll_row: usize,
    /// Current horizontal scroll offset.
    pub scroll_col: usize,
    /// Total number of lines in the document.
    pub total_lines: usize,
    /// Width available for content.
    pub content_width: usize,
}

/// Builder for creating TextArea widgets.
pub struct TextAreaBuilder {
    value: String,
    show_line_numbers: bool,
    wrap_mode: WrapMode,
    tab_width: usize,
    width: usize,
    height: usize,
    disabled: bool,
    readonly: bool,
    style: TextAreaStyle,
    on_change: Option<Box<dyn Fn(&TextAreaChange) + Send + Sync>>,
}

impl Default for TextAreaBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TextAreaBuilder {
    /// Creates a new text area builder.
    pub fn new() -> Self {
        Self {
            value: String::new(),
            show_line_numbers: false,
            wrap_mode: WrapMode::None,
            tab_width: 4,
            width: 80,
            height: 24,
            disabled: false,
            readonly: false,
            style: TextAreaStyle::default(),
            on_change: None,
        }
    }

    /// Sets the initial value.
    pub fn value(mut self, value: impl Into<String>) -> Self {
        self.value = value.into();
        self
    }

    /// Enables or disables line numbers.
    pub fn line_numbers(mut self, show: bool) -> Self {
        self.show_line_numbers = show;
        self
    }

    /// Sets the wrap mode.
    pub fn wrap_mode(mut self, mode: WrapMode) -> Self {
        self.wrap_mode = mode;
        self
    }

    /// Sets the tab width.
    pub fn tab_width(mut self, width: usize) -> Self {
        self.tab_width = width.max(1);
        self
    }

    /// Sets the visual width.
    pub fn width(mut self, width: usize) -> Self {
        self.width = width.max(1);
        self
    }

    /// Sets the visual height.
    pub fn height(mut self, height: usize) -> Self {
        self.height = height.max(1);
        self
    }

    /// Sets the visual dimensions.
    pub fn dimensions(mut self, width: usize, height: usize) -> Self {
        self.width = width.max(1);
        self.height = height.max(1);
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

    /// Sets the line number color.
    pub fn line_number_color(mut self, r: u8, g: u8, b: u8) -> Self {
        self.style.line_number_color = Some((r, g, b));
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

    /// Sets the current line highlight color.
    pub fn current_line_bg(mut self, r: u8, g: u8, b: u8) -> Self {
        self.style.current_line_bg = Some((r, g, b));
        self
    }

    /// Sets the style.
    pub fn style(mut self, style: TextAreaStyle) -> Self {
        self.style = style;
        self
    }

    /// Sets the change callback.
    pub fn on_change<F>(mut self, callback: F) -> Self
    where
        F: Fn(&TextAreaChange) + Send + Sync + 'static,
    {
        self.on_change = Some(Box::new(callback));
        self
    }

    /// Builds the TextArea widget.
    pub fn build(self) -> TextArea {
        let mut textarea = TextArea {
            lines: vec![String::new()],
            cursor: TextCursor::new(),
            scroll_row: 0,
            scroll_col: 0,
            show_line_numbers: self.show_line_numbers,
            line_number_width: 0,
            wrap_mode: self.wrap_mode,
            tab_width: self.tab_width,
            focused: false,
            disabled: self.disabled,
            readonly: self.readonly,
            width: self.width,
            height: self.height,
            style: self.style,
            on_change: self.on_change,
        };

        textarea.set_value(&self.value);
        textarea
    }
}

// ============================================================================
// Line Wrapping Utilities
// ============================================================================

/// Wraps a line at character boundaries.
fn wrap_line_char(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    for grapheme in line.graphemes(true) {
        let g_width = grapheme.width();

        if current_width + g_width > max_width && !current.is_empty() {
            result.push(current);
            current = String::new();
            current_width = 0;
        }

        current.push_str(grapheme);
        current_width += g_width;
    }

    if !current.is_empty() || result.is_empty() {
        result.push(current);
    }

    result
}

/// Wraps a line at word boundaries.
fn wrap_line_word(line: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![line.to_string()];
    }

    let mut result = Vec::new();
    let mut current = String::new();
    let mut current_width = 0;

    // Split into words (keeping whitespace attached)
    let graphemes: Vec<&str> = line.graphemes(true).collect();

    let mut i = 0;
    while i < graphemes.len() {
        // Find word boundary
        let mut word = String::new();
        let mut word_width = 0;

        // Collect word characters
        while i < graphemes.len() {
            let g = graphemes[i];
            let c = g.chars().next().unwrap_or(' ');

            if c.is_whitespace() {
                // Include trailing whitespace with the word
                word.push_str(g);
                word_width += g.width();
                i += 1;
                break;
            }

            word.push_str(g);
            word_width += g.width();
            i += 1;
        }

        // Check if word fits on current line
        if current_width + word_width <= max_width || current.is_empty() {
            current.push_str(&word);
            current_width += word_width;
        } else {
            // Word doesn't fit - wrap
            if !current.is_empty() {
                result.push(current.trim_end().to_string());
                current = String::new();
                current_width = 0;
            }

            // If word is longer than max_width, force break it
            if word_width > max_width {
                let wrapped = wrap_line_char(&word, max_width);
                let wrapped_len = wrapped.len();
                for (j, part) in wrapped.into_iter().enumerate() {
                    if j == wrapped_len - 1 {
                        // Last part becomes current line
                        current_width = part.width();
                        current = part;
                    } else {
                        result.push(part);
                    }
                }
            } else {
                current.push_str(&word);
                current_width = word_width;
            }
        }
    }

    if !current.is_empty() || result.is_empty() {
        result.push(current);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_textarea_insert() {
        let mut ta = TextArea::new();
        ta.insert_char('h');
        ta.insert_char('e');
        ta.insert_char('l');
        ta.insert_char('l');
        ta.insert_char('o');

        assert_eq!(ta.value(), "hello");
        assert_eq!(ta.cursor_position(), CursorPosition::new(0, 5));
    }

    #[test]
    fn test_textarea_newline() {
        let mut ta = TextArea::new();
        ta.insert_text("hello");
        ta.insert_newline();
        ta.insert_text("world");

        assert_eq!(ta.value(), "hello\nworld");
        assert_eq!(ta.line_count(), 2);
        assert_eq!(ta.cursor_position(), CursorPosition::new(1, 5));
    }

    #[test]
    fn test_textarea_multiline_paste() {
        let mut ta = TextArea::new();
        ta.insert_text("line1\nline2\nline3");

        assert_eq!(ta.line_count(), 3);
        assert_eq!(ta.lines()[0], "line1");
        assert_eq!(ta.lines()[1], "line2");
        assert_eq!(ta.lines()[2], "line3");
    }

    #[test]
    fn test_textarea_backspace_join() {
        let mut ta = TextArea::builder().value("hello\nworld").build();
        ta.set_cursor_position(CursorPosition::new(1, 0));

        ta.delete_backward();
        assert_eq!(ta.value(), "helloworld");
        assert_eq!(ta.line_count(), 1);
    }

    #[test]
    fn test_textarea_delete_join() {
        let mut ta = TextArea::builder().value("hello\nworld").build();
        ta.set_cursor_position(CursorPosition::new(0, 5));

        ta.delete_forward();
        assert_eq!(ta.value(), "helloworld");
        assert_eq!(ta.line_count(), 1);
    }

    #[test]
    fn test_textarea_selection_delete() {
        let mut ta = TextArea::builder().value("hello world").build();
        ta.select_all();
        ta.delete_selection();

        assert_eq!(ta.value(), "");
        assert_eq!(ta.cursor_position(), CursorPosition::new(0, 0));
    }

    #[test]
    fn test_textarea_multiline_selection() {
        let mut ta = TextArea::builder().value("line1\nline2\nline3").build();

        // Select from middle of line1 to middle of line3
        ta.set_cursor_position(CursorPosition::new(0, 3));
        ta.cursor.set_position(CursorPosition::new(2, 2), true); // extend selection

        let selected = ta.selected_text();
        assert_eq!(selected, Some("e1\nline2\nli".to_string()));
    }

    #[test]
    fn test_wrap_line_char() {
        let line = "hello world";
        let wrapped = wrap_line_char(line, 5);

        assert_eq!(wrapped.len(), 3);
        assert_eq!(wrapped[0], "hello");
        assert_eq!(wrapped[1], " worl");
        assert_eq!(wrapped[2], "d");
    }

    #[test]
    fn test_wrap_line_word() {
        let line = "hello world test";
        let wrapped = wrap_line_word(line, 10);

        assert_eq!(wrapped.len(), 2);
        assert_eq!(wrapped[0], "hello");
        assert_eq!(wrapped[1], "world test");
    }

    #[test]
    fn test_textarea_line_numbers() {
        let mut ta = TextArea::builder()
            .value("line1\nline2\nline3")
            .line_numbers(true)
            .build();

        let visible = ta.visible_lines();
        assert_eq!(visible.lines[0].line_number, Some(1));
        assert_eq!(visible.lines[1].line_number, Some(2));
        assert_eq!(visible.lines[2].line_number, Some(3));
    }

    #[test]
    fn test_textarea_cursor_movement() {
        let mut ta = TextArea::builder().value("hello\nworld").build();

        ta.set_cursor_position(CursorPosition::new(0, 0));

        ta.handle_key(TextAreaKey::Down, TextAreaModifiers::default());
        assert_eq!(ta.cursor_position(), CursorPosition::new(1, 0));

        ta.handle_key(TextAreaKey::End, TextAreaModifiers::default());
        assert_eq!(ta.cursor_position(), CursorPosition::new(1, 5));

        ta.handle_key(TextAreaKey::Up, TextAreaModifiers::default());
        assert_eq!(ta.cursor_position(), CursorPosition::new(0, 5));
    }
}
