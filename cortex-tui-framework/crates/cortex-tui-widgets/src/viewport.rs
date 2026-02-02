//! Viewport management for scrollable content.

pub use cortex_tui_core::geometry::Rect;
use std::ops::Range;

/// Scroll offset in both dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollOffset {
    /// Horizontal scroll offset (cells from left edge)
    pub x: i32,
    /// Vertical scroll offset (cells from top edge)
    pub y: i32,
}

impl ScrollOffset {
    /// Creates a new scroll offset.
    #[inline]
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }

    /// Returns the zero offset.
    #[inline]
    pub const fn zero() -> Self {
        Self { x: 0, y: 0 }
    }
}

/// Manages the viewport and content relationship for scrollable areas.
///
/// The viewport is the visible area, while content is the total scrollable area.
/// The scroll offset determines which portion of the content is visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// The viewport rectangle (visible area)
    viewport: Rect,
    /// The content rectangle (total scrollable area)
    content: Rect,
    /// Current scroll offset
    offset: ScrollOffset,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            viewport: Rect::default(),
            content: Rect::default(),
            offset: ScrollOffset::zero(),
        }
    }
}

impl Viewport {
    /// Creates a new viewport with the given dimensions.
    pub fn new(viewport_width: u16, viewport_height: u16) -> Self {
        Self {
            viewport: Rect::new(0, 0, viewport_width, viewport_height),
            content: Rect::new(0, 0, viewport_width, viewport_height),
            offset: ScrollOffset::zero(),
        }
    }

    /// Creates a viewport with explicit viewport and content sizes.
    pub fn with_content(
        viewport_width: u16,
        viewport_height: u16,
        content_width: u16,
        content_height: u16,
    ) -> Self {
        let mut viewport = Self {
            viewport: Rect::new(0, 0, viewport_width, viewport_height),
            content: Rect::new(0, 0, content_width, content_height),
            offset: ScrollOffset::zero(),
        };
        viewport.clamp_offset();
        viewport
    }

    // -------------------------------------------------------------------------
    // Getters
    // -------------------------------------------------------------------------

    /// Returns the viewport rectangle.
    #[inline]
    pub const fn viewport_rect(&self) -> Rect {
        self.viewport
    }

    /// Returns the content rectangle.
    #[inline]
    pub const fn content_rect(&self) -> Rect {
        self.content
    }

    /// Returns the current scroll offset.
    #[inline]
    pub const fn offset(&self) -> ScrollOffset {
        self.offset
    }

    /// Returns the viewport width.
    #[inline]
    pub const fn viewport_width(&self) -> u16 {
        self.viewport.width
    }

    /// Returns the viewport height.
    #[inline]
    pub const fn viewport_height(&self) -> u16 {
        self.viewport.height
    }

    /// Returns the content width.
    #[inline]
    pub const fn content_width(&self) -> u16 {
        self.content.width
    }

    /// Returns the content height.
    #[inline]
    pub const fn content_height(&self) -> u16 {
        self.content.height
    }

    /// Returns the horizontal scroll offset.
    #[inline]
    pub const fn scroll_x(&self) -> i32 {
        self.offset.x
    }

    /// Returns the vertical scroll offset.
    #[inline]
    pub const fn scroll_y(&self) -> i32 {
        self.offset.y
    }

    // -------------------------------------------------------------------------
    // Scroll limits
    // -------------------------------------------------------------------------

    /// Returns the maximum horizontal scroll offset.
    #[inline]
    pub fn max_scroll_x(&self) -> i32 {
        (self.content.width as i32 - self.viewport.width as i32).max(0)
    }

    /// Returns the maximum vertical scroll offset.
    #[inline]
    pub fn max_scroll_y(&self) -> i32 {
        (self.content.height as i32 - self.viewport.height as i32).max(0)
    }

    /// Returns the valid range for horizontal scroll offset.
    #[inline]
    pub fn scroll_x_range(&self) -> Range<i32> {
        0..self.max_scroll_x().saturating_add(1)
    }

    /// Returns the valid range for vertical scroll offset.
    #[inline]
    pub fn scroll_y_range(&self) -> Range<i32> {
        0..self.max_scroll_y().saturating_add(1)
    }

    /// Returns true if horizontal scrolling is possible.
    #[inline]
    pub fn can_scroll_x(&self) -> bool {
        self.content.width > self.viewport.width
    }

    /// Returns true if vertical scrolling is possible.
    #[inline]
    pub fn can_scroll_y(&self) -> bool {
        self.content.height > self.viewport.height
    }

    /// Returns true if scrolling is possible in any direction.
    #[inline]
    pub fn can_scroll(&self) -> bool {
        self.can_scroll_x() || self.can_scroll_y()
    }

    /// Returns true if the viewport is at the top edge.
    #[inline]
    pub fn is_at_top(&self) -> bool {
        self.offset.y <= 0
    }

    /// Returns true if the viewport is at the bottom edge.
    #[inline]
    pub fn is_at_bottom(&self) -> bool {
        self.offset.y >= self.max_scroll_y()
    }

    /// Returns true if the viewport is at the left edge.
    #[inline]
    pub fn is_at_left(&self) -> bool {
        self.offset.x <= 0
    }

    /// Returns true if the viewport is at the right edge.
    #[inline]
    pub fn is_at_right(&self) -> bool {
        self.offset.x >= self.max_scroll_x()
    }

    // -------------------------------------------------------------------------
    // Setters
    // -------------------------------------------------------------------------

    /// Sets the viewport position.
    pub fn set_viewport_position(&mut self, x: i32, y: i32) {
        self.viewport.x = x;
        self.viewport.y = y;
    }

    /// Sets the viewport size.
    pub fn set_viewport_size(&mut self, width: u16, height: u16) {
        self.viewport.width = width;
        self.viewport.height = height;
        self.clamp_offset();
    }

    /// Sets the content size.
    pub fn set_content_size(&mut self, width: u16, height: u16) {
        self.content.width = width;
        self.content.height = height;
        self.clamp_offset();
    }

    /// Sets the horizontal scroll offset, clamping to valid range.
    pub fn set_scroll_x(&mut self, x: i32) {
        self.offset.x = x.clamp(0, self.max_scroll_x());
    }

    /// Sets the vertical scroll offset, clamping to valid range.
    pub fn set_scroll_y(&mut self, y: i32) {
        self.offset.y = y.clamp(0, self.max_scroll_y());
    }

    /// Sets both scroll offsets, clamping to valid range.
    pub fn set_scroll(&mut self, x: i32, y: i32) {
        self.set_scroll_x(x);
        self.set_scroll_y(y);
    }

    /// Clamps the current offset to valid bounds.
    fn clamp_offset(&mut self) {
        self.offset.x = self.offset.x.clamp(0, self.max_scroll_x());
        self.offset.y = self.offset.y.clamp(0, self.max_scroll_y());
    }

    // -------------------------------------------------------------------------
    // Scroll operations
    // -------------------------------------------------------------------------

    /// Scrolls by the given delta, clamping to valid range.
    /// Returns the actual delta applied.
    pub fn scroll_by(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        let old_x = self.offset.x;
        let old_y = self.offset.y;

        self.set_scroll_x(self.offset.x.saturating_add(dx));
        self.set_scroll_y(self.offset.y.saturating_add(dy));

        (self.offset.x - old_x, self.offset.y - old_y)
    }

    /// Scrolls to the top.
    pub fn scroll_to_top(&mut self) {
        self.offset.y = 0;
    }

    /// Scrolls to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.offset.y = self.max_scroll_y();
    }

    /// Scrolls to the left edge.
    pub fn scroll_to_left(&mut self) {
        self.offset.x = 0;
    }

    /// Scrolls to the right edge.
    pub fn scroll_to_right(&mut self) {
        self.offset.x = self.max_scroll_x();
    }

    /// Scrolls up by the given amount.
    pub fn scroll_up(&mut self, amount: u16) -> i32 {
        let (_, dy) = self.scroll_by(0, -(amount as i32));
        dy
    }

    /// Scrolls down by the given amount.
    pub fn scroll_down(&mut self, amount: u16) -> i32 {
        let (_, dy) = self.scroll_by(0, amount as i32);
        dy
    }

    /// Scrolls left by the given amount.
    pub fn scroll_left(&mut self, amount: u16) -> i32 {
        let (dx, _) = self.scroll_by(-(amount as i32), 0);
        dx
    }

    /// Scrolls right by the given amount.
    pub fn scroll_right(&mut self, amount: u16) -> i32 {
        let (dx, _) = self.scroll_by(amount as i32, 0);
        dx
    }

    /// Scrolls up by one page (viewport height).
    pub fn page_up(&mut self) -> i32 {
        self.scroll_up(self.viewport.height.saturating_sub(1).max(1))
    }

    /// Scrolls down by one page (viewport height).
    pub fn page_down(&mut self) -> i32 {
        self.scroll_down(self.viewport.height.saturating_sub(1).max(1))
    }

    /// Scrolls left by one page (viewport width).
    pub fn page_left(&mut self) -> i32 {
        self.scroll_left(self.viewport.width.saturating_sub(1).max(1))
    }

    /// Scrolls right by one page (viewport width).
    pub fn page_right(&mut self) -> i32 {
        self.scroll_right(self.viewport.width.saturating_sub(1).max(1))
    }

    // -------------------------------------------------------------------------
    // Visibility checks
    // -------------------------------------------------------------------------

    /// Returns the visible rect in content coordinates.
    pub fn visible_content_rect(&self) -> Rect {
        Rect::new(
            self.offset.x,
            self.offset.y,
            self.viewport.width.min(self.content.width),
            self.viewport.height.min(self.content.height),
        )
    }

    /// Checks if a point (in content coordinates) is visible.
    #[inline]
    pub fn is_point_visible(&self, x: i32, y: i32) -> bool {
        x >= self.offset.x
            && x < self.offset.x + self.viewport.width as i32
            && y >= self.offset.y
            && y < self.offset.y + self.viewport.height as i32
    }

    /// Checks if a rectangle (in content coordinates) is at least partially visible.
    pub fn is_rect_visible(&self, rect: &Rect) -> bool {
        let visible = self.visible_content_rect();
        visible.intersects(*rect)
    }

    /// Checks if a rectangle (in content coordinates) is fully visible.
    pub fn is_rect_fully_visible(&self, rect: &Rect) -> bool {
        rect.x >= self.offset.x
            && rect.right() <= self.offset.x + self.viewport.width as i32
            && rect.y >= self.offset.y
            && rect.bottom() <= self.offset.y + self.viewport.height as i32
    }

    /// Converts content coordinates to viewport coordinates.
    #[inline]
    pub fn content_to_viewport(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x - self.offset.x + self.viewport.x,
            y - self.offset.y + self.viewport.y,
        )
    }

    /// Converts viewport coordinates to content coordinates.
    #[inline]
    pub fn viewport_to_content(&self, x: i32, y: i32) -> (i32, i32) {
        (
            x - self.viewport.x + self.offset.x,
            y - self.viewport.y + self.offset.y,
        )
    }

    // -------------------------------------------------------------------------
    // Scroll into view
    // -------------------------------------------------------------------------

    /// Scrolls the minimum amount to make a point visible.
    /// Returns true if scrolling occurred.
    pub fn scroll_to_point(&mut self, x: i32, y: i32) -> bool {
        let old_offset = self.offset;

        // Horizontal adjustment
        if x < self.offset.x {
            self.offset.x = x;
        } else if x >= self.offset.x + self.viewport.width as i32 {
            self.offset.x = x - self.viewport.width as i32 + 1;
        }

        // Vertical adjustment
        if y < self.offset.y {
            self.offset.y = y;
        } else if y >= self.offset.y + self.viewport.height as i32 {
            self.offset.y = y - self.viewport.height as i32 + 1;
        }

        self.clamp_offset();
        self.offset != old_offset
    }

    /// Scrolls the minimum amount to make a rectangle visible.
    /// Returns true if scrolling occurred.
    pub fn scroll_to_rect(&mut self, rect: &Rect) -> bool {
        let old_offset = self.offset;

        // Horizontal adjustment
        if rect.x < self.offset.x {
            self.offset.x = rect.x;
        } else if rect.right() > self.offset.x + self.viewport.width as i32 {
            // If rect is wider than viewport, align to left edge
            if rect.width >= self.viewport.width {
                self.offset.x = rect.x;
            } else {
                self.offset.x = rect.right() - self.viewport.width as i32;
            }
        }

        // Vertical adjustment
        if rect.y < self.offset.y {
            self.offset.y = rect.y;
        } else if rect.bottom() > self.offset.y + self.viewport.height as i32 {
            // If rect is taller than viewport, align to top edge
            if rect.height >= self.viewport.height {
                self.offset.y = rect.y;
            } else {
                self.offset.y = rect.bottom() - self.viewport.height as i32;
            }
        }

        self.clamp_offset();
        self.offset != old_offset
    }

    /// Scrolls to center a point in the viewport if possible.
    /// Returns true if scrolling occurred.
    pub fn scroll_to_center_point(&mut self, x: i32, y: i32) -> bool {
        let old_offset = self.offset;

        self.offset.x = x - (self.viewport.width as i32 / 2);
        self.offset.y = y - (self.viewport.height as i32 / 2);

        self.clamp_offset();
        self.offset != old_offset
    }

    /// Scrolls to center a rectangle in the viewport if possible.
    /// Returns true if scrolling occurred.
    pub fn scroll_to_center_rect(&mut self, rect: &Rect) -> bool {
        let center_x = rect.x + rect.width as i32 / 2;
        let center_y = rect.y + rect.height as i32 / 2;
        self.scroll_to_center_point(center_x, center_y)
    }

    // -------------------------------------------------------------------------
    // Scroll ratios (for scrollbar positioning)
    // -------------------------------------------------------------------------

    /// Returns the horizontal scroll ratio (0.0 to 1.0).
    pub fn scroll_ratio_x(&self) -> f32 {
        let max = self.max_scroll_x();
        if max == 0 {
            0.0
        } else {
            self.offset.x as f32 / max as f32
        }
    }

    /// Returns the vertical scroll ratio (0.0 to 1.0).
    pub fn scroll_ratio_y(&self) -> f32 {
        let max = self.max_scroll_y();
        if max == 0 {
            0.0
        } else {
            self.offset.y as f32 / max as f32
        }
    }

    /// Returns the viewport to content ratio for horizontal dimension.
    /// This determines the scrollbar thumb size.
    pub fn viewport_ratio_x(&self) -> f32 {
        if self.content.width == 0 {
            1.0
        } else {
            (self.viewport.width as f32 / self.content.width as f32).min(1.0)
        }
    }

    /// Returns the viewport to content ratio for vertical dimension.
    /// This determines the scrollbar thumb size.
    pub fn viewport_ratio_y(&self) -> f32 {
        if self.content.height == 0 {
            1.0
        } else {
            (self.viewport.height as f32 / self.content.height as f32).min(1.0)
        }
    }

    /// Sets the horizontal scroll position from a ratio (0.0 to 1.0).
    pub fn set_scroll_ratio_x(&mut self, ratio: f32) {
        let ratio = ratio.clamp(0.0, 1.0);
        self.offset.x = (ratio * self.max_scroll_x() as f32).round() as i32;
    }

    /// Sets the vertical scroll position from a ratio (0.0 to 1.0).
    pub fn set_scroll_ratio_y(&mut self, ratio: f32) {
        let ratio = ratio.clamp(0.0, 1.0);
        self.offset.y = (ratio * self.max_scroll_y() as f32).round() as i32;
    }
}

/// Builder for creating [`Viewport`] instances.
#[derive(Debug, Clone, Default)]
pub struct ViewportBuilder {
    viewport_width: u16,
    viewport_height: u16,
    viewport_x: i32,
    viewport_y: i32,
    content_width: Option<u16>,
    content_height: Option<u16>,
    initial_scroll_x: i32,
    initial_scroll_y: i32,
}

impl ViewportBuilder {
    /// Creates a new viewport builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the viewport dimensions.
    pub fn viewport_size(mut self, width: u16, height: u16) -> Self {
        self.viewport_width = width;
        self.viewport_height = height;
        self
    }

    /// Sets the viewport position.
    pub fn viewport_position(mut self, x: i32, y: i32) -> Self {
        self.viewport_x = x;
        self.viewport_y = y;
        self
    }

    /// Sets the content dimensions.
    pub fn content_size(mut self, width: u16, height: u16) -> Self {
        self.content_width = Some(width);
        self.content_height = Some(height);
        self
    }

    /// Sets the initial scroll position.
    pub fn initial_scroll(mut self, x: i32, y: i32) -> Self {
        self.initial_scroll_x = x;
        self.initial_scroll_y = y;
        self
    }

    /// Builds the viewport.
    pub fn build(self) -> Viewport {
        let content_width = self.content_width.unwrap_or(self.viewport_width);
        let content_height = self.content_height.unwrap_or(self.viewport_height);

        let mut viewport = Viewport {
            viewport: Rect::new(
                self.viewport_x,
                self.viewport_y,
                self.viewport_width,
                self.viewport_height,
            ),
            content: Rect::new(0, 0, content_width, content_height),
            offset: ScrollOffset::new(self.initial_scroll_x, self.initial_scroll_y),
        };

        viewport.clamp_offset();
        viewport
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect_intersection() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(5, 5, 10, 10);

        assert!(a.intersects(b));

        let intersection = a.intersection(b).unwrap();
        assert_eq!(intersection, Rect::new(5, 5, 5, 5));
    }

    #[test]
    fn test_rect_no_intersection() {
        let a = Rect::new(0, 0, 10, 10);
        let b = Rect::new(20, 20, 10, 10);

        assert!(!a.intersects(b));
        assert!(a.intersection(b).is_none());
    }

    #[test]
    fn test_viewport_scroll_limits() {
        let viewport = Viewport::with_content(100, 50, 200, 100);

        assert_eq!(viewport.max_scroll_x(), 100);
        assert_eq!(viewport.max_scroll_y(), 50);
        assert!(viewport.can_scroll());
    }

    #[test]
    fn test_viewport_no_scroll_when_content_fits() {
        let viewport = Viewport::with_content(100, 50, 50, 25);

        assert_eq!(viewport.max_scroll_x(), 0);
        assert_eq!(viewport.max_scroll_y(), 0);
        assert!(!viewport.can_scroll());
    }

    #[test]
    fn test_viewport_scroll_clamping() {
        let mut viewport = Viewport::with_content(100, 50, 200, 100);

        viewport.set_scroll_x(150);
        assert_eq!(viewport.scroll_x(), 100); // Clamped to max

        viewport.set_scroll_x(-10);
        assert_eq!(viewport.scroll_x(), 0); // Clamped to min
    }

    #[test]
    fn test_viewport_scroll_to_point() {
        let mut viewport = Viewport::with_content(100, 50, 200, 100);

        // Point below visible area
        assert!(viewport.scroll_to_point(50, 80));
        assert!(viewport.is_point_visible(50, 80));

        // Point already visible - no scroll
        assert!(!viewport.scroll_to_point(50, 40));
    }

    #[test]
    fn test_viewport_ratios() {
        let mut viewport = Viewport::with_content(100, 50, 200, 100);

        assert_eq!(viewport.viewport_ratio_x(), 0.5);
        assert_eq!(viewport.viewport_ratio_y(), 0.5);

        viewport.set_scroll_x(50);
        assert_eq!(viewport.scroll_ratio_x(), 0.5);
    }

    #[test]
    fn test_viewport_builder() {
        let viewport = ViewportBuilder::new()
            .viewport_size(100, 50)
            .content_size(200, 100)
            .initial_scroll(25, 10)
            .build();

        assert_eq!(viewport.viewport_width(), 100);
        assert_eq!(viewport.content_width(), 200);
        assert_eq!(viewport.scroll_x(), 25);
        assert_eq!(viewport.scroll_y(), 10);
    }
}
