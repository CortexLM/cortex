//! Scroll state management and utilities.
//!
//! Provides consistent scrolling behavior across all list-based components.

use cortex_core::style::{SURFACE_1, TEXT_MUTED};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget};

/// State for scrollable content.
///
/// # Example
///
/// ```rust
/// use cortex_tui_components::scroll::ScrollState;
///
/// let mut scroll = ScrollState::new(100, 20); // 100 items, 20 visible
///
/// scroll.scroll_down(5);
/// assert_eq!(scroll.offset(), 5);
///
/// scroll.ensure_visible(90);
/// assert!(scroll.offset() >= 71); // Item 90 should be visible
/// ```
#[derive(Debug, Clone)]
pub struct ScrollState {
    /// Total number of items
    total: usize,
    /// Number of visible items
    visible: usize,
    /// Current scroll offset
    offset: usize,
}

impl ScrollState {
    /// Create a new scroll state.
    ///
    /// # Arguments
    /// * `total` - Total number of items
    /// * `visible` - Number of items visible at once
    pub fn new(total: usize, visible: usize) -> Self {
        Self {
            total,
            visible,
            offset: 0,
        }
    }

    /// Get the current scroll offset.
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Get the total number of items.
    pub fn total(&self) -> usize {
        self.total
    }

    /// Get the number of visible items.
    pub fn visible(&self) -> usize {
        self.visible
    }

    /// Set the total number of items.
    pub fn set_total(&mut self, total: usize) {
        self.total = total;
        self.clamp_offset();
    }

    /// Set the number of visible items.
    pub fn set_visible(&mut self, visible: usize) {
        self.visible = visible;
        self.clamp_offset();
    }

    /// Set the scroll offset directly.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
        self.clamp_offset();
    }

    /// Scroll up by the given number of lines.
    pub fn scroll_up(&mut self, lines: usize) {
        self.offset = self.offset.saturating_sub(lines);
    }

    /// Scroll down by the given number of lines.
    pub fn scroll_down(&mut self, lines: usize) {
        self.offset = self.offset.saturating_add(lines);
        self.clamp_offset();
    }

    /// Scroll to the top.
    pub fn scroll_to_top(&mut self) {
        self.offset = 0;
    }

    /// Scroll to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        if self.total > self.visible {
            self.offset = self.total - self.visible;
        } else {
            self.offset = 0;
        }
    }

    /// Page up (scroll by visible height).
    pub fn page_up(&mut self) {
        self.scroll_up(self.visible.saturating_sub(1).max(1));
    }

    /// Page down (scroll by visible height).
    pub fn page_down(&mut self) {
        self.scroll_down(self.visible.saturating_sub(1).max(1));
    }

    /// Ensure a specific item index is visible.
    ///
    /// Adjusts offset if necessary to make the item visible.
    pub fn ensure_visible(&mut self, index: usize) {
        // Guard against zero visible items to prevent underflow
        if self.visible == 0 {
            return;
        }
        if index < self.offset {
            // Item is above visible area - scroll up
            self.offset = index;
        } else if index >= self.offset + self.visible {
            // Item is below visible area - scroll down
            self.offset = index.saturating_sub(self.visible.saturating_sub(1));
        }
        self.clamp_offset();
    }

    /// Check if an item at the given index is currently visible.
    pub fn is_visible(&self, index: usize) -> bool {
        index >= self.offset && index < self.offset + self.visible
    }

    /// Check if scrollbar is needed (total > visible).
    pub fn needs_scrollbar(&self) -> bool {
        self.total > self.visible
    }

    /// Get the range of currently visible items.
    pub fn visible_range(&self) -> std::ops::Range<usize> {
        let start = self.offset;
        let end = (self.offset + self.visible).min(self.total);
        start..end
    }

    /// Calculate the scrollbar thumb position (0.0 to 1.0).
    pub fn thumb_position(&self) -> f32 {
        if self.total <= self.visible {
            return 0.0;
        }
        let max_offset = self.total - self.visible;
        self.offset as f32 / max_offset as f32
    }

    /// Calculate the scrollbar thumb size (0.0 to 1.0).
    pub fn thumb_size(&self) -> f32 {
        if self.total == 0 {
            return 1.0;
        }
        (self.visible as f32 / self.total as f32).min(1.0)
    }

    /// Clamp offset to valid range.
    fn clamp_offset(&mut self) {
        if self.total <= self.visible {
            self.offset = 0;
        } else {
            self.offset = self.offset.min(self.total - self.visible);
        }
    }
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new(0, 10)
    }
}

/// Trait for components that support scrolling.
pub trait Scrollable {
    /// Get the scroll state.
    fn scroll_state(&self) -> &ScrollState;

    /// Get mutable scroll state.
    fn scroll_state_mut(&mut self) -> &mut ScrollState;

    /// Scroll up by one line.
    fn scroll_up(&mut self) {
        self.scroll_state_mut().scroll_up(1);
    }

    /// Scroll down by one line.
    fn scroll_down(&mut self) {
        self.scroll_state_mut().scroll_down(1);
    }

    /// Scroll up by a page.
    fn page_up(&mut self) {
        self.scroll_state_mut().page_up();
    }

    /// Scroll down by a page.
    fn page_down(&mut self) {
        self.scroll_state_mut().page_down();
    }

    /// Scroll to the top.
    fn scroll_to_top(&mut self) {
        self.scroll_state_mut().scroll_to_top();
    }

    /// Scroll to the bottom.
    fn scroll_to_bottom(&mut self) {
        self.scroll_state_mut().scroll_to_bottom();
    }
}

/// Render a vertical scrollbar for the given scroll state.
///
/// # Arguments
/// * `area` - Area for the scrollbar (usually 1 column wide on the right)
/// * `buf` - Buffer to render to
/// * `scroll` - Scroll state
pub fn render_scrollbar(area: Rect, buf: &mut Buffer, scroll: &ScrollState) {
    if !scroll.needs_scrollbar() || area.width == 0 || area.height == 0 {
        return;
    }

    let scrollable_range = scroll.total.saturating_sub(scroll.visible);
    let mut scrollbar_state = ScrollbarState::new(scrollable_range).position(scroll.offset);

    Scrollbar::new(ScrollbarOrientation::VerticalRight)
        .begin_symbol(None)
        .end_symbol(None)
        .track_symbol(Some("│"))
        .track_style(Style::default().fg(SURFACE_1))
        .thumb_symbol("█")
        .thumb_style(Style::default().fg(TEXT_MUTED))
        .render(area, buf, &mut scrollbar_state);
}

/// Render scroll indicators (arrows showing more content above/below).
///
/// # Arguments
/// * `area` - Full content area
/// * `buf` - Buffer to render to
/// * `scroll` - Scroll state
/// * `style` - Style for the indicators
pub fn render_scroll_indicators(area: Rect, buf: &mut Buffer, scroll: &ScrollState, style: Style) {
    // More content above indicator
    if scroll.offset > 0 {
        let indicator = "▲";
        let x = area.x + area.width.saturating_sub(2);
        if let Some(cell) = buf.cell_mut((x, area.y)) {
            cell.set_symbol(indicator).set_style(style);
        }
    }

    // More content below indicator
    if scroll.offset + scroll.visible < scroll.total {
        let indicator = "▼";
        let x = area.x + area.width.saturating_sub(2);
        let y = area.y + area.height.saturating_sub(1);
        if let Some(cell) = buf.cell_mut((x, y)) {
            cell.set_symbol(indicator).set_style(style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scroll_state_basic() {
        let scroll = ScrollState::new(100, 20);
        assert_eq!(scroll.offset(), 0);
        assert_eq!(scroll.total(), 100);
        assert_eq!(scroll.visible(), 20);
        assert!(scroll.needs_scrollbar());
    }

    #[test]
    fn test_scroll_state_no_scrollbar_needed() {
        let scroll = ScrollState::new(10, 20);
        assert!(!scroll.needs_scrollbar());
    }

    #[test]
    fn test_scroll_up_down() {
        let mut scroll = ScrollState::new(100, 20);

        scroll.scroll_down(10);
        assert_eq!(scroll.offset(), 10);

        scroll.scroll_up(5);
        assert_eq!(scroll.offset(), 5);

        // Can't scroll past 0
        scroll.scroll_up(100);
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn test_scroll_clamp() {
        let mut scroll = ScrollState::new(100, 20);

        // Can't scroll past max
        scroll.scroll_down(1000);
        assert_eq!(scroll.offset(), 80); // 100 - 20
    }

    #[test]
    fn test_ensure_visible() {
        let mut scroll = ScrollState::new(100, 20);

        // Item below viewport
        scroll.ensure_visible(50);
        assert!(scroll.is_visible(50));

        // Item above viewport
        scroll.ensure_visible(10);
        assert!(scroll.is_visible(10));
    }

    #[test]
    fn test_page_up_down() {
        let mut scroll = ScrollState::new(100, 20);

        scroll.page_down();
        assert_eq!(scroll.offset(), 19); // visible - 1

        scroll.page_up();
        assert_eq!(scroll.offset(), 0);
    }

    #[test]
    fn test_visible_range() {
        let mut scroll = ScrollState::new(100, 20);
        scroll.set_offset(30);

        let range = scroll.visible_range();
        assert_eq!(range, 30..50);
    }

    #[test]
    fn test_thumb_position() {
        let mut scroll = ScrollState::new(100, 20);

        assert_eq!(scroll.thumb_position(), 0.0);

        scroll.scroll_to_bottom();
        assert_eq!(scroll.thumb_position(), 1.0);

        scroll.set_offset(40);
        assert_eq!(scroll.thumb_position(), 0.5);
    }

    #[test]
    fn test_thumb_size() {
        let scroll = ScrollState::new(100, 20);
        assert_eq!(scroll.thumb_size(), 0.2);

        let scroll2 = ScrollState::new(10, 20);
        assert_eq!(scroll2.thumb_size(), 1.0);
    }
}
