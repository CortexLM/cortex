//! Text selection state for the entire TUI screen.
//!
//! Provides mouse-based text selection functionality similar to terminal emulators.
//! Text can be selected anywhere on the screen and copied with right-click or Ctrl+C.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use cortex_tui::selection::TextSelection;
//!
//! let mut selection = TextSelection::new();
//!
//! // On mouse down (screen coordinates)
//! selection.start_selection(x, y);
//!
//! // On mouse drag
//! selection.update_selection(x, y);
//!
//! // On mouse up - finish and get bounds for copying
//! selection.finish_selection();
//! if let Some(bounds) = selection.get_bounds() {
//!     // Copy text within bounds from screen buffer
//! }
//! ```

use ratatui::layout::Rect;

// ============================================================================
// TEXT SELECTION
// ============================================================================

/// Text selection state for the entire TUI screen.
///
/// Tracks the start and end positions of a mouse selection using screen coordinates,
/// and provides methods to manage the selection lifecycle.
#[derive(Debug, Clone, Default)]
pub struct TextSelection {
    /// Start position (column, row) in screen coordinates
    start: Option<(u16, u16)>,
    /// End position (column, row) in screen coordinates
    end: Option<(u16, u16)>,
    /// Whether selection is active (mouse button held down)
    selecting: bool,
    /// The screen area for bounds checking (usually full terminal)
    screen_area: Option<Rect>,
}

impl TextSelection {
    /// Creates a new empty text selection.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the screen area for coordinate validation.
    pub fn set_screen_area(&mut self, area: Rect) {
        self.screen_area = Some(area);
    }

    /// Sets the chat area for coordinate validation (alias for backwards compatibility).
    pub fn set_chat_area(&mut self, area: Rect) {
        self.screen_area = Some(area);
    }

    /// Starts a new selection at the given position.
    ///
    /// This is called on mouse down. Clears any existing selection
    /// and marks the start point.
    pub fn start_selection(&mut self, x: u16, y: u16) {
        self.start = Some((x, y));
        self.end = Some((x, y));
        self.selecting = true;
    }

    /// Updates the selection end position.
    ///
    /// This is called on mouse drag while selecting.
    pub fn update_selection(&mut self, x: u16, y: u16) {
        if self.selecting {
            self.end = Some((x, y));
        }
    }

    /// Finishes the selection (mouse released).
    ///
    /// After this call, `is_selecting()` returns false but
    /// `has_selection()` may still return true if a valid
    /// selection exists.
    pub fn finish_selection(&mut self) {
        self.selecting = false;
    }

    /// Clears the selection entirely.
    pub fn clear(&mut self) {
        self.start = None;
        self.end = None;
        self.selecting = false;
    }

    /// Returns true if a selection exists (start and end are set).
    pub fn has_selection(&self) -> bool {
        if let (Some(start), Some(end)) = (self.start, self.end) {
            // Only count as selection if start != end
            start != end
        } else {
            false
        }
    }

    /// Returns true if currently selecting (mouse button held).
    pub fn is_selecting(&self) -> bool {
        self.selecting
    }

    /// Returns the start position if set.
    pub fn start_pos(&self) -> Option<(u16, u16)> {
        self.start
    }

    /// Returns the end position if set.
    pub fn end_pos(&self) -> Option<(u16, u16)> {
        self.end
    }

    /// Returns normalized selection bounds where start <= end.
    ///
    /// The bounds are normalized so that the start position is always
    /// before (or equal to) the end position in reading order
    /// (top-to-bottom, left-to-right).
    ///
    /// Returns `None` if no selection exists.
    pub fn get_bounds(&self) -> Option<((u16, u16), (u16, u16))> {
        let start = self.start?;
        let end = self.end?;

        // Don't return bounds if start == end (no actual selection)
        if start == end {
            return None;
        }

        // Normalize: ensure start is before end in reading order
        if start.1 < end.1 || (start.1 == end.1 && start.0 <= end.0) {
            Some((start, end))
        } else {
            Some((end, start))
        }
    }

    /// Converts screen coordinates to area-relative coordinates.
    ///
    /// Returns `None` if the position is outside the screen area.
    pub fn screen_to_relative(&self, screen_x: u16, screen_y: u16) -> Option<(u16, u16)> {
        let area = self.screen_area?;

        if screen_x >= area.x
            && screen_x < area.x + area.width
            && screen_y >= area.y
            && screen_y < area.y + area.height
        {
            Some((screen_x - area.x, screen_y - area.y))
        } else {
            None
        }
    }

    /// Alias for backwards compatibility.
    pub fn screen_to_chat(&self, screen_x: u16, screen_y: u16) -> Option<(u16, u16)> {
        self.screen_to_relative(screen_x, screen_y)
    }

    /// Checks if the given screen position is within the screen area.
    pub fn is_in_screen_area(&self, screen_x: u16, screen_y: u16) -> bool {
        self.screen_to_relative(screen_x, screen_y).is_some()
    }

    /// Alias for backwards compatibility.
    pub fn is_in_chat_area(&self, screen_x: u16, screen_y: u16) -> bool {
        self.is_in_screen_area(screen_x, screen_y)
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_selection_is_empty() {
        let selection = TextSelection::new();
        assert!(!selection.has_selection());
        assert!(!selection.is_selecting());
        assert!(selection.get_bounds().is_none());
    }

    #[test]
    fn test_start_selection() {
        let mut selection = TextSelection::new();
        selection.start_selection(5, 10);

        assert!(selection.is_selecting());
        assert!(!selection.has_selection()); // start == end, so no selection yet
        assert_eq!(selection.start_pos(), Some((5, 10)));
        assert_eq!(selection.end_pos(), Some((5, 10)));
    }

    #[test]
    fn test_update_selection() {
        let mut selection = TextSelection::new();
        selection.start_selection(5, 10);
        selection.update_selection(15, 12);

        assert!(selection.is_selecting());
        assert!(selection.has_selection());
        assert_eq!(selection.start_pos(), Some((5, 10)));
        assert_eq!(selection.end_pos(), Some((15, 12)));
    }

    #[test]
    fn test_finish_selection() {
        let mut selection = TextSelection::new();
        selection.start_selection(5, 10);
        selection.update_selection(15, 12);
        selection.finish_selection();

        assert!(!selection.is_selecting());
        assert!(selection.has_selection()); // Selection still exists
    }

    #[test]
    fn test_clear_selection() {
        let mut selection = TextSelection::new();
        selection.start_selection(5, 10);
        selection.update_selection(15, 12);
        selection.clear();

        assert!(!selection.is_selecting());
        assert!(!selection.has_selection());
        assert!(selection.get_bounds().is_none());
    }

    #[test]
    fn test_get_bounds_normalizes() {
        let mut selection = TextSelection::new();

        // Selection from top-left to bottom-right
        selection.start_selection(5, 10);
        selection.update_selection(15, 12);
        let bounds = selection.get_bounds().unwrap();
        assert_eq!(bounds, ((5, 10), (15, 12)));

        // Selection from bottom-right to top-left (should be normalized)
        selection.start_selection(15, 12);
        selection.update_selection(5, 10);
        let bounds = selection.get_bounds().unwrap();
        assert_eq!(bounds, ((5, 10), (15, 12)));
    }

    #[test]
    fn test_get_bounds_same_line() {
        let mut selection = TextSelection::new();

        // Left to right on same line
        selection.start_selection(5, 10);
        selection.update_selection(15, 10);
        let bounds = selection.get_bounds().unwrap();
        assert_eq!(bounds, ((5, 10), (15, 10)));

        // Right to left on same line (should be normalized)
        selection.start_selection(15, 10);
        selection.update_selection(5, 10);
        let bounds = selection.get_bounds().unwrap();
        assert_eq!(bounds, ((5, 10), (15, 10)));
    }

    #[test]
    fn test_screen_to_chat() {
        let mut selection = TextSelection::new();
        selection.set_chat_area(Rect::new(10, 5, 80, 20));

        // Inside chat area
        assert_eq!(selection.screen_to_chat(10, 5), Some((0, 0)));
        assert_eq!(selection.screen_to_chat(50, 15), Some((40, 10)));

        // Outside chat area
        assert_eq!(selection.screen_to_chat(5, 5), None);
        assert_eq!(selection.screen_to_chat(10, 3), None);
        assert_eq!(selection.screen_to_chat(100, 10), None);
    }

    #[test]
    fn test_is_in_chat_area() {
        let mut selection = TextSelection::new();
        selection.set_chat_area(Rect::new(10, 5, 80, 20));

        assert!(selection.is_in_chat_area(10, 5));
        assert!(selection.is_in_chat_area(50, 15));
        assert!(!selection.is_in_chat_area(5, 5));
        assert!(!selection.is_in_chat_area(100, 10));
    }

    #[test]
    fn test_update_only_when_selecting() {
        let mut selection = TextSelection::new();
        selection.start_selection(5, 10);
        selection.finish_selection();

        // Update should be ignored when not selecting
        selection.update_selection(100, 100);
        assert_eq!(selection.end_pos(), Some((5, 10)));
    }
}
