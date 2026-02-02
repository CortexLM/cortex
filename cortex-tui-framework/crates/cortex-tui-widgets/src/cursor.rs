//! Cursor utilities for text editing widgets.
//!
//! This module provides cursor position tracking, movement operations,
//! text selection, and word boundary detection for Input and TextArea widgets.

use unicode_segmentation::UnicodeSegmentation;

/// Represents a cursor position in a text buffer.
///
/// For single-line text, only `col` is relevant.
/// For multi-line text, both `row` and `col` are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CursorPosition {
    /// Row (line) index, 0-based.
    pub row: usize,
    /// Column (grapheme) index within the row, 0-based.
    pub col: usize,
}

impl CursorPosition {
    /// Creates a new cursor position at the given row and column.
    #[inline]
    pub const fn new(row: usize, col: usize) -> Self {
        Self { row, col }
    }

    /// Creates a cursor position at the origin (0, 0).
    #[inline]
    pub const fn origin() -> Self {
        Self { row: 0, col: 0 }
    }

    /// Creates a cursor position for single-line text at the given column.
    #[inline]
    pub const fn at_col(col: usize) -> Self {
        Self { row: 0, col }
    }

    /// Returns true if this position is at the origin.
    #[inline]
    pub const fn is_origin(&self) -> bool {
        self.row == 0 && self.col == 0
    }

    /// Returns true if this position is before another position.
    #[inline]
    pub fn is_before(&self, other: &Self) -> bool {
        self.row < other.row || (self.row == other.row && self.col < other.col)
    }

    /// Returns true if this position is after another position.
    #[inline]
    pub fn is_after(&self, other: &Self) -> bool {
        self.row > other.row || (self.row == other.row && self.col > other.col)
    }

    /// Returns the minimum of two positions (earlier in text).
    #[inline]
    pub fn min(self, other: Self) -> Self {
        if self.is_before(&other) {
            self
        } else {
            other
        }
    }

    /// Returns the maximum of two positions (later in text).
    #[inline]
    pub fn max(self, other: Self) -> Self {
        if self.is_after(&other) {
            self
        } else {
            other
        }
    }
}

impl From<(usize, usize)> for CursorPosition {
    fn from((row, col): (usize, usize)) -> Self {
        Self { row, col }
    }
}

impl From<CursorPosition> for (usize, usize) {
    fn from(pos: CursorPosition) -> Self {
        (pos.row, pos.col)
    }
}

/// Represents a text selection range.
///
/// A selection is defined by an anchor (where selection started) and
/// a cursor (current cursor position). The selection can be forward
/// (anchor before cursor) or backward (anchor after cursor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    /// The anchor point where selection started.
    pub anchor: CursorPosition,
    /// The current cursor position (selection end).
    pub cursor: CursorPosition,
}

impl Selection {
    /// Creates a new selection from anchor to cursor.
    #[inline]
    pub const fn new(anchor: CursorPosition, cursor: CursorPosition) -> Self {
        Self { anchor, cursor }
    }

    /// Creates a collapsed selection (no text selected) at the given position.
    #[inline]
    pub const fn collapsed(pos: CursorPosition) -> Self {
        Self {
            anchor: pos,
            cursor: pos,
        }
    }

    /// Returns true if the selection is collapsed (no text selected).
    #[inline]
    pub fn is_collapsed(&self) -> bool {
        self.anchor == self.cursor
    }

    /// Returns true if the selection is forward (anchor before cursor).
    #[inline]
    pub fn is_forward(&self) -> bool {
        self.anchor.is_before(&self.cursor)
    }

    /// Returns true if the selection is backward (anchor after cursor).
    #[inline]
    pub fn is_backward(&self) -> bool {
        self.anchor.is_after(&self.cursor)
    }

    /// Returns the start position (earlier in text).
    #[inline]
    pub fn start(&self) -> CursorPosition {
        self.anchor.min(self.cursor)
    }

    /// Returns the end position (later in text).
    #[inline]
    pub fn end(&self) -> CursorPosition {
        self.anchor.max(self.cursor)
    }

    /// Extends the selection to the given cursor position.
    #[inline]
    pub fn extend_to(&mut self, cursor: CursorPosition) {
        self.cursor = cursor;
    }

    /// Collapses the selection to the cursor position.
    #[inline]
    pub fn collapse_to_cursor(&mut self) {
        self.anchor = self.cursor;
    }

    /// Collapses the selection to the start position.
    #[inline]
    pub fn collapse_to_start(&mut self) {
        let start = self.start();
        self.anchor = start;
        self.cursor = start;
    }

    /// Collapses the selection to the end position.
    #[inline]
    pub fn collapse_to_end(&mut self) {
        let end = self.end();
        self.anchor = end;
        self.cursor = end;
    }

    /// Returns true if the given position is within the selection.
    pub fn contains(&self, pos: CursorPosition) -> bool {
        let start = self.start();
        let end = self.end();

        if pos.row < start.row || pos.row > end.row {
            return false;
        }

        if pos.row == start.row && pos.col < start.col {
            return false;
        }

        if pos.row == end.row && pos.col >= end.col {
            return false;
        }

        true
    }
}

impl Default for Selection {
    fn default() -> Self {
        Self::collapsed(CursorPosition::origin())
    }
}

/// Operations for moving the cursor within text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorMove {
    /// Move left by one grapheme.
    Left,
    /// Move right by one grapheme.
    Right,
    /// Move up by one line.
    Up,
    /// Move down by one line.
    Down,
    /// Move to the start of the current line.
    Home,
    /// Move to the end of the current line.
    End,
    /// Move to the start of the current word.
    WordStart,
    /// Move to the end of the current word.
    WordEnd,
    /// Move left by one word.
    WordLeft,
    /// Move right by one word.
    WordRight,
    /// Move to the start of the document.
    DocumentStart,
    /// Move to the end of the document.
    DocumentEnd,
    /// Move up by one page.
    PageUp(usize),
    /// Move down by one page.
    PageDown(usize),
}

/// Cursor manager for single-line text editing.
///
/// Handles cursor movement, selection, and position validation
/// for a single line of text.
#[derive(Debug, Clone)]
pub struct LineCursor {
    /// Current selection (anchor and cursor).
    selection: Selection,
    /// Cached grapheme count for efficiency.
    grapheme_count: usize,
}

impl LineCursor {
    /// Creates a new cursor at position 0.
    pub fn new() -> Self {
        Self {
            selection: Selection::default(),
            grapheme_count: 0,
        }
    }

    /// Creates a cursor at the end of the given text.
    pub fn at_end(text: &str) -> Self {
        let count = text.graphemes(true).count();
        Self {
            selection: Selection::collapsed(CursorPosition::at_col(count)),
            grapheme_count: count,
        }
    }

    /// Updates the cached grapheme count from the text.
    pub fn update_text(&mut self, text: &str) {
        self.grapheme_count = text.graphemes(true).count();
        self.clamp_position();
    }

    /// Returns the current cursor column.
    #[inline]
    pub fn col(&self) -> usize {
        self.selection.cursor.col
    }

    /// Returns the current selection.
    #[inline]
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Returns true if there is an active selection.
    #[inline]
    pub fn has_selection(&self) -> bool {
        !self.selection.is_collapsed()
    }

    /// Sets the cursor position, optionally extending the selection.
    pub fn set_position(&mut self, col: usize, extend_selection: bool) {
        let col = col.min(self.grapheme_count);
        let new_pos = CursorPosition::at_col(col);

        if extend_selection {
            self.selection.extend_to(new_pos);
        } else {
            self.selection = Selection::collapsed(new_pos);
        }
    }

    /// Moves the cursor by the given operation.
    pub fn move_cursor(&mut self, text: &str, movement: CursorMove, extend_selection: bool) {
        let new_col = match movement {
            CursorMove::Left => self.col().saturating_sub(1),
            CursorMove::Right => (self.col() + 1).min(self.grapheme_count),
            CursorMove::Home | CursorMove::DocumentStart => 0,
            CursorMove::End | CursorMove::DocumentEnd => self.grapheme_count,
            CursorMove::WordLeft => find_word_boundary_left(text, self.col()),
            CursorMove::WordRight => find_word_boundary_right(text, self.col()),
            CursorMove::WordStart => find_word_start(text, self.col()),
            CursorMove::WordEnd => find_word_end(text, self.col()),
            // Up/Down/PageUp/PageDown don't apply to single-line
            CursorMove::Up | CursorMove::Down | CursorMove::PageUp(_) | CursorMove::PageDown(_) => {
                self.col()
            }
        };

        self.set_position(new_col, extend_selection);
    }

    /// Selects all text.
    pub fn select_all(&mut self) {
        self.selection.anchor = CursorPosition::at_col(0);
        self.selection.cursor = CursorPosition::at_col(self.grapheme_count);
    }

    /// Clears the selection, keeping the cursor at its current position.
    pub fn clear_selection(&mut self) {
        self.selection.collapse_to_cursor();
    }

    /// Clamps the cursor position to valid bounds.
    fn clamp_position(&mut self) {
        if self.selection.cursor.col > self.grapheme_count {
            self.selection.cursor.col = self.grapheme_count;
        }
        if self.selection.anchor.col > self.grapheme_count {
            self.selection.anchor.col = self.grapheme_count;
        }
    }

    /// Converts a grapheme index to a byte offset.
    pub fn grapheme_to_byte_offset(text: &str, grapheme_idx: usize) -> usize {
        text.grapheme_indices(true)
            .nth(grapheme_idx)
            .map(|(offset, _)| offset)
            .unwrap_or(text.len())
    }

    /// Converts a byte offset to a grapheme index.
    pub fn byte_to_grapheme_offset(text: &str, byte_offset: usize) -> usize {
        text.grapheme_indices(true)
            .take_while(|(offset, _)| *offset < byte_offset)
            .count()
    }

    /// Gets the selected text, if any.
    pub fn get_selected_text<'a>(&self, text: &'a str) -> Option<&'a str> {
        if self.selection.is_collapsed() {
            return None;
        }

        let start = self.selection.start().col;
        let end = self.selection.end().col;

        let start_byte = Self::grapheme_to_byte_offset(text, start);
        let end_byte = Self::grapheme_to_byte_offset(text, end);

        Some(&text[start_byte..end_byte])
    }
}

impl Default for LineCursor {
    fn default() -> Self {
        Self::new()
    }
}

/// Cursor manager for multi-line text editing.
///
/// Handles cursor movement, selection, and position validation
/// for multiple lines of text.
#[derive(Debug, Clone)]
pub struct TextCursor {
    /// Current selection (anchor and cursor).
    selection: Selection,
    /// Cached line information for efficiency.
    line_info: Vec<LineInfo>,
    /// Preferred column for vertical movement (sticky column).
    preferred_col: Option<usize>,
}

/// Information about a line of text.
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
struct LineInfo {
    /// Number of graphemes in the line.
    grapheme_count: usize,
    /// Byte offset of the line start.
    byte_offset: usize,
}

impl TextCursor {
    /// Creates a new cursor at position (0, 0).
    pub fn new() -> Self {
        Self {
            selection: Selection::default(),
            line_info: vec![LineInfo::default()],
            preferred_col: None,
        }
    }

    /// Creates a cursor at the end of the given text.
    pub fn at_end(lines: &[String]) -> Self {
        let mut cursor = Self::new();
        cursor.update_lines(lines);

        let row = lines.len().saturating_sub(1);
        let col = lines.last().map(|l| l.graphemes(true).count()).unwrap_or(0);

        cursor.selection = Selection::collapsed(CursorPosition::new(row, col));
        cursor
    }

    /// Updates cached line information.
    pub fn update_lines(&mut self, lines: &[String]) {
        self.line_info.clear();
        let mut byte_offset = 0;

        for line in lines {
            let grapheme_count = line.graphemes(true).count();
            self.line_info.push(LineInfo {
                grapheme_count,
                byte_offset,
            });
            byte_offset += line.len() + 1; // +1 for newline
        }

        if self.line_info.is_empty() {
            self.line_info.push(LineInfo::default());
        }

        self.clamp_position();
    }

    /// Returns the current cursor position.
    #[inline]
    pub fn position(&self) -> CursorPosition {
        self.selection.cursor
    }

    /// Returns the current row.
    #[inline]
    pub fn row(&self) -> usize {
        self.selection.cursor.row
    }

    /// Returns the current column.
    #[inline]
    pub fn col(&self) -> usize {
        self.selection.cursor.col
    }

    /// Returns the current selection.
    #[inline]
    pub fn selection(&self) -> &Selection {
        &self.selection
    }

    /// Returns true if there is an active selection.
    #[inline]
    pub fn has_selection(&self) -> bool {
        !self.selection.is_collapsed()
    }

    /// Returns the number of lines.
    #[inline]
    pub fn line_count(&self) -> usize {
        self.line_info.len()
    }

    /// Returns the grapheme count for a given line.
    pub fn line_grapheme_count(&self, row: usize) -> usize {
        self.line_info
            .get(row)
            .map(|info| info.grapheme_count)
            .unwrap_or(0)
    }

    /// Sets the cursor position, optionally extending the selection.
    pub fn set_position(&mut self, pos: CursorPosition, extend_selection: bool) {
        let row = pos.row.min(self.line_info.len().saturating_sub(1));
        let col = pos.col.min(self.line_grapheme_count(row));
        let new_pos = CursorPosition::new(row, col);

        if extend_selection {
            self.selection.extend_to(new_pos);
        } else {
            self.selection = Selection::collapsed(new_pos);
        }

        self.preferred_col = None;
    }

    /// Moves the cursor by the given operation.
    pub fn move_cursor(&mut self, lines: &[String], movement: CursorMove, extend_selection: bool) {
        let (new_row, new_col) = match movement {
            CursorMove::Left => {
                if self.col() > 0 {
                    (self.row(), self.col() - 1)
                } else if self.row() > 0 {
                    // Move to end of previous line
                    let prev_row = self.row() - 1;
                    (prev_row, self.line_grapheme_count(prev_row))
                } else {
                    (0, 0)
                }
            }
            CursorMove::Right => {
                let line_len = self.line_grapheme_count(self.row());
                if self.col() < line_len {
                    (self.row(), self.col() + 1)
                } else if self.row() < self.line_count().saturating_sub(1) {
                    // Move to start of next line
                    (self.row() + 1, 0)
                } else {
                    (self.row(), line_len)
                }
            }
            CursorMove::Up => {
                let preferred = self.preferred_col.unwrap_or(self.col());
                if self.row() > 0 {
                    let new_row = self.row() - 1;
                    let new_col = preferred.min(self.line_grapheme_count(new_row));
                    self.preferred_col = Some(preferred);
                    (new_row, new_col)
                } else {
                    (0, 0)
                }
            }
            CursorMove::Down => {
                let preferred = self.preferred_col.unwrap_or(self.col());
                if self.row() < self.line_count().saturating_sub(1) {
                    let new_row = self.row() + 1;
                    let new_col = preferred.min(self.line_grapheme_count(new_row));
                    self.preferred_col = Some(preferred);
                    (new_row, new_col)
                } else {
                    (self.row(), self.line_grapheme_count(self.row()))
                }
            }
            CursorMove::Home => (self.row(), 0),
            CursorMove::End => (self.row(), self.line_grapheme_count(self.row())),
            CursorMove::DocumentStart => (0, 0),
            CursorMove::DocumentEnd => {
                let last_row = self.line_count().saturating_sub(1);
                (last_row, self.line_grapheme_count(last_row))
            }
            CursorMove::WordLeft => {
                let line = lines.get(self.row()).map(|s| s.as_str()).unwrap_or("");
                let new_col = find_word_boundary_left(line, self.col());
                if new_col == self.col() && self.row() > 0 {
                    // Move to end of previous line
                    let prev_row = self.row() - 1;
                    (prev_row, self.line_grapheme_count(prev_row))
                } else {
                    (self.row(), new_col)
                }
            }
            CursorMove::WordRight => {
                let line = lines.get(self.row()).map(|s| s.as_str()).unwrap_or("");
                let new_col = find_word_boundary_right(line, self.col());
                let line_len = self.line_grapheme_count(self.row());
                if new_col == self.col()
                    && self.col() >= line_len
                    && self.row() < self.line_count().saturating_sub(1)
                {
                    // Move to start of next line
                    (self.row() + 1, 0)
                } else {
                    (self.row(), new_col)
                }
            }
            CursorMove::WordStart => {
                let line = lines.get(self.row()).map(|s| s.as_str()).unwrap_or("");
                (self.row(), find_word_start(line, self.col()))
            }
            CursorMove::WordEnd => {
                let line = lines.get(self.row()).map(|s| s.as_str()).unwrap_or("");
                (self.row(), find_word_end(line, self.col()))
            }
            CursorMove::PageUp(page_size) => {
                let new_row = self.row().saturating_sub(page_size);
                let new_col = self.col().min(self.line_grapheme_count(new_row));
                (new_row, new_col)
            }
            CursorMove::PageDown(page_size) => {
                let new_row = (self.row() + page_size).min(self.line_count().saturating_sub(1));
                let new_col = self.col().min(self.line_grapheme_count(new_row));
                (new_row, new_col)
            }
        };

        let new_pos = CursorPosition::new(new_row, new_col);

        if extend_selection {
            self.selection.extend_to(new_pos);
        } else {
            self.selection = Selection::collapsed(new_pos);
            // Clear preferred column on horizontal movement
            if !matches!(movement, CursorMove::Up | CursorMove::Down) {
                self.preferred_col = None;
            }
        }
    }

    /// Selects all text.
    pub fn select_all(&mut self) {
        self.selection.anchor = CursorPosition::origin();
        let last_row = self.line_count().saturating_sub(1);
        self.selection.cursor = CursorPosition::new(last_row, self.line_grapheme_count(last_row));
    }

    /// Clears the selection.
    pub fn clear_selection(&mut self) {
        self.selection.collapse_to_cursor();
    }

    /// Clamps the cursor position to valid bounds.
    fn clamp_position(&mut self) {
        let max_row = self.line_info.len().saturating_sub(1);

        if self.selection.cursor.row > max_row {
            self.selection.cursor.row = max_row;
        }
        let max_col = self.line_grapheme_count(self.selection.cursor.row);
        if self.selection.cursor.col > max_col {
            self.selection.cursor.col = max_col;
        }

        if self.selection.anchor.row > max_row {
            self.selection.anchor.row = max_row;
        }
        let max_col = self.line_grapheme_count(self.selection.anchor.row);
        if self.selection.anchor.col > max_col {
            self.selection.anchor.col = max_col;
        }
    }

    /// Returns the selected text across multiple lines, if any.
    pub fn get_selected_text(&self, lines: &[String]) -> Option<String> {
        if self.selection.is_collapsed() {
            return None;
        }

        let start = self.selection.start();
        let end = self.selection.end();

        let mut result = String::new();

        for (row, line) in lines.iter().enumerate() {
            if row < start.row || row > end.row {
                continue;
            }

            let line_start = if row == start.row { start.col } else { 0 };
            let line_end = if row == end.row {
                end.col
            } else {
                line.graphemes(true).count()
            };

            let start_byte = grapheme_to_byte_offset(line, line_start);
            let end_byte = grapheme_to_byte_offset(line, line_end);

            result.push_str(&line[start_byte..end_byte]);

            if row < end.row {
                result.push('\n');
            }
        }

        Some(result)
    }
}

impl Default for TextCursor {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Word Boundary Detection
// ============================================================================

/// Checks if a character is a word character (alphanumeric or underscore).
#[inline]
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Finds the word boundary to the left of the given position.
fn find_word_boundary_left(text: &str, col: usize) -> usize {
    if col == 0 {
        return 0;
    }

    let graphemes: Vec<&str> = text.graphemes(true).collect();

    if col > graphemes.len() {
        return graphemes.len();
    }

    let mut pos = col - 1;

    // Skip whitespace
    while pos > 0 {
        let c = graphemes[pos].chars().next().unwrap_or(' ');
        if !c.is_whitespace() {
            break;
        }
        pos -= 1;
    }

    // If we're on whitespace at start, return 0
    if pos == 0 {
        let c = graphemes[0].chars().next().unwrap_or(' ');
        if c.is_whitespace() {
            return 0;
        }
    }

    // Determine character class at current position
    let current_char = graphemes[pos].chars().next().unwrap_or(' ');
    let is_word = is_word_char(current_char);

    // Move back while in same character class
    while pos > 0 {
        let c = graphemes[pos - 1].chars().next().unwrap_or(' ');
        if is_word != is_word_char(c) || c.is_whitespace() {
            break;
        }
        pos -= 1;
    }

    pos
}

/// Finds the word boundary to the right of the given position.
fn find_word_boundary_right(text: &str, col: usize) -> usize {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let len = graphemes.len();

    if col >= len {
        return len;
    }

    let mut pos = col;

    // Determine character class at current position
    let current_char = graphemes[pos].chars().next().unwrap_or(' ');

    // If on whitespace, skip it first
    if current_char.is_whitespace() {
        while pos < len {
            let c = graphemes[pos].chars().next().unwrap_or(' ');
            if !c.is_whitespace() {
                break;
            }
            pos += 1;
        }
        return pos;
    }

    let is_word = is_word_char(current_char);

    // Move forward while in same character class
    while pos < len {
        let c = graphemes[pos].chars().next().unwrap_or(' ');
        if is_word != is_word_char(c) || c.is_whitespace() {
            break;
        }
        pos += 1;
    }

    // Skip trailing whitespace
    while pos < len {
        let c = graphemes[pos].chars().next().unwrap_or(' ');
        if !c.is_whitespace() {
            break;
        }
        pos += 1;
    }

    pos
}

/// Finds the start of the word at the given position.
fn find_word_start(text: &str, col: usize) -> usize {
    if col == 0 {
        return 0;
    }

    let graphemes: Vec<&str> = text.graphemes(true).collect();

    if col > graphemes.len() {
        return find_word_start(text, graphemes.len());
    }

    // Get character at col - 1 (we're finding start of word cursor is in/after)
    let check_col = col.saturating_sub(1);
    let current_char = graphemes[check_col].chars().next().unwrap_or(' ');

    if current_char.is_whitespace() {
        return col;
    }

    let is_word = is_word_char(current_char);
    let mut pos = check_col;

    // Move back while in same character class
    while pos > 0 {
        let c = graphemes[pos - 1].chars().next().unwrap_or(' ');
        if is_word != is_word_char(c) || c.is_whitespace() {
            break;
        }
        pos -= 1;
    }

    pos
}

/// Finds the end of the word at the given position.
fn find_word_end(text: &str, col: usize) -> usize {
    let graphemes: Vec<&str> = text.graphemes(true).collect();
    let len = graphemes.len();

    if col >= len {
        return len;
    }

    let current_char = graphemes[col].chars().next().unwrap_or(' ');

    if current_char.is_whitespace() {
        return col;
    }

    let is_word = is_word_char(current_char);
    let mut pos = col;

    // Move forward while in same character class
    while pos < len {
        let c = graphemes[pos].chars().next().unwrap_or(' ');
        if is_word != is_word_char(c) || c.is_whitespace() {
            break;
        }
        pos += 1;
    }

    pos
}

/// Converts a grapheme index to a byte offset in text.
pub fn grapheme_to_byte_offset(text: &str, grapheme_idx: usize) -> usize {
    text.grapheme_indices(true)
        .nth(grapheme_idx)
        .map(|(offset, _)| offset)
        .unwrap_or(text.len())
}

/// Converts a byte offset to a grapheme index.
pub fn byte_to_grapheme_offset(text: &str, byte_offset: usize) -> usize {
    text.grapheme_indices(true)
        .take_while(|(offset, _)| *offset < byte_offset)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_position_ordering() {
        let a = CursorPosition::new(0, 5);
        let b = CursorPosition::new(0, 10);
        let c = CursorPosition::new(1, 0);

        assert!(a.is_before(&b));
        assert!(b.is_before(&c));
        assert!(a.is_before(&c));
        assert!(!b.is_before(&a));
    }

    #[test]
    fn test_selection_contains() {
        let sel = Selection::new(CursorPosition::new(0, 5), CursorPosition::new(0, 10));

        assert!(sel.contains(CursorPosition::new(0, 7)));
        assert!(!sel.contains(CursorPosition::new(0, 4)));
        assert!(!sel.contains(CursorPosition::new(0, 10)));
    }

    #[test]
    fn test_word_boundary_left() {
        let text = "hello world test";
        assert_eq!(find_word_boundary_left(text, 11), 6); // "w" -> "w"
        assert_eq!(find_word_boundary_left(text, 6), 0); // space before "world" -> "hello"
        assert_eq!(find_word_boundary_left(text, 5), 0); // "o" in hello -> "h"
    }

    #[test]
    fn test_word_boundary_right() {
        let text = "hello world test";
        assert_eq!(find_word_boundary_right(text, 0), 6); // "h" -> end of "hello" + space
        assert_eq!(find_word_boundary_right(text, 6), 12); // "w" -> end of "world" + space
    }

    #[test]
    fn test_grapheme_offset_conversion() {
        let text = "héllo 🌍";
        assert_eq!(grapheme_to_byte_offset(text, 0), 0);
        assert_eq!(grapheme_to_byte_offset(text, 1), 1); // h
        assert_eq!(grapheme_to_byte_offset(text, 2), 3); // é is 2 bytes
        assert_eq!(byte_to_grapheme_offset(text, 3), 2);
    }
}
