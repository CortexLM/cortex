//! Layout management utilities for minimal session view.

use ratatui::layout::Rect;

/// Layout manager for automatic vertical positioning of UI elements.
/// Tracks the next available Y position and allocates space for each element.
pub struct LayoutManager {
    x: u16,
    width: u16,
    pub next_y: u16,
    max_y: u16,
}

impl LayoutManager {
    pub fn new(area: Rect) -> Self {
        Self {
            x: area.x,
            width: area.width,
            next_y: area.y,
            max_y: area.y + area.height,
        }
    }

    /// Allocates space for an element and returns the Rect.
    /// Automatically advances next_y.
    pub fn allocate(&mut self, height: u16) -> Rect {
        let available = self.max_y.saturating_sub(self.next_y);
        let actual_height = height.min(available);
        let rect = Rect::new(self.x, self.next_y, self.width, actual_height);
        self.next_y += actual_height;
        rect
    }

    /// Adds vertical gap.
    pub fn gap(&mut self, lines: u16) {
        self.next_y = (self.next_y + lines).min(self.max_y);
    }

    /// Returns remaining height from current position.
    pub fn remaining_height(&self) -> u16 {
        self.max_y.saturating_sub(self.next_y)
    }

    /// Allocates remaining space minus reserved bottom space.
    #[allow(dead_code)]
    pub fn allocate_remaining(&mut self, reserve_bottom: u16) -> Rect {
        let available = self.remaining_height().saturating_sub(reserve_bottom);
        self.allocate(available)
    }
}
