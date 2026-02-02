//! Core types for the widgets crate.

pub use cortex_tui_buffer::Cell;
pub use cortex_tui_core::color::Color;
pub use cortex_tui_core::geometry::{Point, Rect, Size};
pub use cortex_tui_core::style::{Style, TextAttributes};

/// RGBA color with 8-bit components.
/// Re-exported as Color from cortex_tui_core for compatibility.
pub use cortex_tui_core::Color as RGBA;

/// Edge values for padding, margin, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Edges {
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
    pub left: u16,
}

impl Edges {
    /// Creates new edge values.
    pub const fn new(top: u16, right: u16, bottom: u16, left: u16) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    /// Creates uniform edge values.
    pub const fn uniform(value: u16) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }

    /// Creates symmetric edge values (vertical, horizontal).
    pub const fn symmetric(vertical: u16, horizontal: u16) -> Self {
        Self {
            top: vertical,
            right: horizontal,
            bottom: vertical,
            left: horizontal,
        }
    }

    /// Zero edges.
    pub const ZERO: Self = Self {
        top: 0,
        right: 0,
        bottom: 0,
        left: 0,
    };

    /// Returns the total horizontal size.
    pub const fn horizontal(&self) -> u16 {
        self.left.saturating_add(self.right)
    }

    /// Returns the total vertical size.
    pub const fn vertical(&self) -> u16 {
        self.top.saturating_add(self.bottom)
    }
}
