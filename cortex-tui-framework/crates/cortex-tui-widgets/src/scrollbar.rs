//! Scrollbar rendering component.
//!
//! This module provides a scrollbar component that can be rendered
//! vertically or horizontally. It uses Unicode block characters for
//! sub-cell precision rendering of the thumb.

use crate::viewport::Rect;

/// Scrollbar orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Vertical scrollbar (up/down)
    #[default]
    Vertical,
    /// Horizontal scrollbar (left/right)
    Horizontal,
}

/// Scrollbar visibility mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollbarVisibility {
    /// Always show the scrollbar
    Always,
    /// Automatically show/hide based on content size
    #[default]
    Auto,
    /// Never show the scrollbar
    Never,
}

/// Unicode block characters for vertical scrollbar rendering.
/// These provide 8 levels of sub-cell precision.
#[allow(dead_code)]
mod vertical_blocks {
    /// Full block █
    pub const FULL: char = '█';
    /// Upper half block ▀
    pub const UPPER_HALF: char = '▀';
    /// Lower half block ▄
    pub const LOWER_HALF: char = '▄';
    /// Upper one eighth block ▔
    pub const UPPER_EIGHTH: char = '▔';
    /// Lower one eighth block ▁
    pub const LOWER_EIGHTH: char = '▁';
    /// Upper quarter block (approximated with upper half)
    pub const UPPER_QUARTER: char = '▀';
    /// Lower quarter block ▂
    pub const LOWER_QUARTER: char = '▂';
    /// Lower three eighths block ▃
    pub const LOWER_THREE_EIGHTHS: char = '▃';
    /// Lower five eighths block ▅
    pub const LOWER_FIVE_EIGHTHS: char = '▅';
    /// Lower three quarters block ▆
    pub const LOWER_THREE_QUARTERS: char = '▆';
    /// Lower seven eighths block ▇
    pub const LOWER_SEVEN_EIGHTHS: char = '▇';
}

/// Unicode block characters for horizontal scrollbar rendering.
mod horizontal_blocks {
    /// Full block █
    pub const FULL: char = '█';
    /// Left half block ▌
    pub const LEFT_HALF: char = '▌';
    /// Right half block ▐
    pub const RIGHT_HALF: char = '▐';
    /// Left one eighth block ▏
    pub const LEFT_EIGHTH: char = '▏';
    /// Left quarter block ▎
    pub const LEFT_QUARTER: char = '▎';
    /// Left three eighths block ▍
    pub const LEFT_THREE_EIGHTHS: char = '▍';
    /// Left five eighths block ▋
    pub const LEFT_FIVE_EIGHTHS: char = '▋';
    /// Left three quarters block ▊
    pub const LEFT_THREE_QUARTERS: char = '▊';
    /// Left seven eighths block ▉
    pub const LEFT_SEVEN_EIGHTHS: char = '▉';
}

/// A rendered cell in the scrollbar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarCell {
    /// The character to render
    pub character: char,
    /// Whether this cell is part of the thumb
    pub is_thumb: bool,
    /// Whether this is a partial thumb cell (edge)
    pub is_partial: bool,
}

impl Default for ScrollbarCell {
    fn default() -> Self {
        Self {
            character: ' ',
            is_thumb: false,
            is_partial: false,
        }
    }
}

/// Scrollbar state and rendering.
///
/// The scrollbar calculates thumb size and position based on the ratio
/// between viewport and content size, and renders using Unicode block
/// characters for sub-cell precision.
#[derive(Debug, Clone)]
pub struct Scrollbar {
    /// Scrollbar orientation
    orientation: Orientation,
    /// Track length in cells
    track_length: u16,
    /// Current scroll ratio (0.0 to 1.0)
    scroll_ratio: f32,
    /// Viewport to content ratio (determines thumb size)
    viewport_ratio: f32,
    /// Visibility mode
    visibility: ScrollbarVisibility,
    /// Minimum thumb size in cells
    min_thumb_size: u16,
    /// Whether the scrollbar is currently visible
    is_visible: bool,
    /// Virtual resolution multiplier for sub-cell precision
    virtual_resolution: u16,
}

impl Default for Scrollbar {
    fn default() -> Self {
        Self {
            orientation: Orientation::Vertical,
            track_length: 10,
            scroll_ratio: 0.0,
            viewport_ratio: 1.0,
            visibility: ScrollbarVisibility::Auto,
            min_thumb_size: 1,
            is_visible: false,
            virtual_resolution: 8, // 8 levels of sub-cell precision
        }
    }
}

impl Scrollbar {
    /// Creates a new scrollbar with the given orientation.
    pub fn new(orientation: Orientation) -> Self {
        Self {
            orientation,
            ..Default::default()
        }
    }

    /// Creates a new vertical scrollbar.
    pub fn vertical() -> Self {
        Self::new(Orientation::Vertical)
    }

    /// Creates a new horizontal scrollbar.
    pub fn horizontal() -> Self {
        Self::new(Orientation::Horizontal)
    }

    // -------------------------------------------------------------------------
    // Configuration
    // -------------------------------------------------------------------------

    /// Sets the track length in cells.
    pub fn set_track_length(&mut self, length: u16) {
        self.track_length = length.max(1);
        self.update_visibility();
    }

    /// Sets the scroll ratio (0.0 = start, 1.0 = end).
    pub fn set_scroll_ratio(&mut self, ratio: f32) {
        self.scroll_ratio = ratio.clamp(0.0, 1.0);
    }

    /// Sets the viewport to content ratio.
    /// A ratio of 1.0 means content fits in viewport (no scrolling).
    /// A ratio of 0.5 means viewport shows half of content.
    pub fn set_viewport_ratio(&mut self, ratio: f32) {
        self.viewport_ratio = ratio.clamp(0.0, 1.0);
        self.update_visibility();
    }

    /// Sets the visibility mode.
    pub fn set_visibility(&mut self, visibility: ScrollbarVisibility) {
        self.visibility = visibility;
        self.update_visibility();
    }

    /// Sets the minimum thumb size in cells.
    pub fn set_min_thumb_size(&mut self, size: u16) {
        self.min_thumb_size = size.max(1);
    }

    /// Updates scrollbar state from viewport dimensions.
    pub fn update_from_viewport(
        &mut self,
        viewport_size: u16,
        content_size: u16,
        scroll_offset: i32,
    ) {
        let max_scroll = (content_size as i32 - viewport_size as i32).max(0);

        self.viewport_ratio = if content_size == 0 {
            1.0
        } else {
            (viewport_size as f32 / content_size as f32).min(1.0)
        };

        self.scroll_ratio = if max_scroll == 0 {
            0.0
        } else {
            (scroll_offset as f32 / max_scroll as f32).clamp(0.0, 1.0)
        };

        self.update_visibility();
    }

    fn update_visibility(&mut self) {
        self.is_visible = match self.visibility {
            ScrollbarVisibility::Always => true,
            ScrollbarVisibility::Never => false,
            ScrollbarVisibility::Auto => self.viewport_ratio < 1.0 && self.track_length > 0,
        };
    }

    // -------------------------------------------------------------------------
    // Getters
    // -------------------------------------------------------------------------

    /// Returns the orientation.
    #[inline]
    pub const fn orientation(&self) -> Orientation {
        self.orientation
    }

    /// Returns the track length in cells.
    #[inline]
    pub const fn track_length(&self) -> u16 {
        self.track_length
    }

    /// Returns the scroll ratio.
    #[inline]
    pub const fn scroll_ratio(&self) -> f32 {
        self.scroll_ratio
    }

    /// Returns the viewport ratio.
    #[inline]
    pub const fn viewport_ratio(&self) -> f32 {
        self.viewport_ratio
    }

    /// Returns whether the scrollbar should be visible.
    #[inline]
    pub const fn is_visible(&self) -> bool {
        self.is_visible
    }

    /// Returns the visibility mode.
    #[inline]
    pub const fn visibility(&self) -> ScrollbarVisibility {
        self.visibility
    }

    // -------------------------------------------------------------------------
    // Thumb calculations
    // -------------------------------------------------------------------------

    /// Returns the thumb size in virtual units.
    fn virtual_thumb_size(&self) -> u32 {
        let virtual_track = self.track_length as u32 * self.virtual_resolution as u32;

        if self.viewport_ratio >= 1.0 {
            return virtual_track;
        }

        let thumb_size = (virtual_track as f32 * self.viewport_ratio).round() as u32;
        let min_virtual_size = self.min_thumb_size as u32 * self.virtual_resolution as u32;

        thumb_size.clamp(min_virtual_size, virtual_track)
    }

    /// Returns the thumb start position in virtual units.
    fn virtual_thumb_start(&self) -> u32 {
        let virtual_track = self.track_length as u32 * self.virtual_resolution as u32;
        let thumb_size = self.virtual_thumb_size();
        let available_range = virtual_track.saturating_sub(thumb_size);

        (self.scroll_ratio * available_range as f32).round() as u32
    }

    /// Returns the thumb end position in virtual units (exclusive).
    fn virtual_thumb_end(&self) -> u32 {
        self.virtual_thumb_start() + self.virtual_thumb_size()
    }

    /// Returns the thumb size in cells.
    pub fn thumb_size(&self) -> u16 {
        let virtual_size = self.virtual_thumb_size();
        ((virtual_size + self.virtual_resolution as u32 - 1) / self.virtual_resolution as u32)
            as u16
    }

    /// Returns the thumb start position in cells.
    pub fn thumb_start(&self) -> u16 {
        (self.virtual_thumb_start() / self.virtual_resolution as u32) as u16
    }

    /// Returns the thumb end position in cells (exclusive).
    pub fn thumb_end(&self) -> u16 {
        ((self.virtual_thumb_end() + self.virtual_resolution as u32 - 1)
            / self.virtual_resolution as u32) as u16
    }

    // -------------------------------------------------------------------------
    // Hit testing
    // -------------------------------------------------------------------------

    /// Checks if a position is on the thumb.
    pub fn hit_test_thumb(&self, position: u16) -> bool {
        position >= self.thumb_start() && position < self.thumb_end()
    }

    /// Checks if a position is on the track (before the thumb).
    pub fn hit_test_track_before(&self, position: u16) -> bool {
        position < self.thumb_start()
    }

    /// Checks if a position is on the track (after the thumb).
    pub fn hit_test_track_after(&self, position: u16) -> bool {
        position >= self.thumb_end()
    }

    /// Converts a track position to a scroll ratio.
    pub fn position_to_ratio(&self, position: u16) -> f32 {
        let thumb_size = self.thumb_size();
        let available_range = self.track_length.saturating_sub(thumb_size);

        if available_range == 0 {
            return 0.0;
        }

        // Center the position on the thumb
        let centered_position = position.saturating_sub(thumb_size / 2);
        (centered_position as f32 / available_range as f32).clamp(0.0, 1.0)
    }

    // -------------------------------------------------------------------------
    // Rendering
    // -------------------------------------------------------------------------

    /// Renders the scrollbar to a vector of cells.
    /// Returns an empty vector if the scrollbar is not visible.
    pub fn render(&self) -> Vec<ScrollbarCell> {
        if !self.is_visible || self.track_length == 0 {
            return Vec::new();
        }

        let mut cells = Vec::with_capacity(self.track_length as usize);
        let virtual_start = self.virtual_thumb_start();
        let virtual_end = self.virtual_thumb_end();

        for cell_idx in 0..self.track_length {
            let virtual_cell_start = cell_idx * self.virtual_resolution;
            let virtual_cell_end = virtual_cell_start + self.virtual_resolution;

            let cell = if u32::from(virtual_cell_end) <= virtual_start
                || u32::from(virtual_cell_start) >= virtual_end
            {
                // Cell is entirely outside thumb
                ScrollbarCell {
                    character: ' ',
                    is_thumb: false,
                    is_partial: false,
                }
            } else if u32::from(virtual_cell_start) >= virtual_start
                && u32::from(virtual_cell_end) <= virtual_end
            {
                // Cell is entirely inside thumb
                ScrollbarCell {
                    character: self.full_block_char(),
                    is_thumb: true,
                    is_partial: false,
                }
            } else {
                // Cell is partially covered by thumb
                let coverage_start = virtual_start.max(u32::from(virtual_cell_start))
                    - u32::from(virtual_cell_start);
                let coverage_end =
                    virtual_end.min(u32::from(virtual_cell_end)) - u32::from(virtual_cell_start);
                let coverage = coverage_end - coverage_start;

                let character = self.partial_block_char(coverage_start, coverage);
                ScrollbarCell {
                    character,
                    is_thumb: true,
                    is_partial: true,
                }
            };

            cells.push(cell);
        }

        cells
    }

    /// Returns the full block character for the orientation.
    fn full_block_char(&self) -> char {
        match self.orientation {
            Orientation::Vertical => vertical_blocks::FULL,
            Orientation::Horizontal => horizontal_blocks::FULL,
        }
    }

    /// Returns a partial block character based on coverage.
    fn partial_block_char(&self, start: u32, coverage: u32) -> char {
        match self.orientation {
            Orientation::Vertical => self.vertical_partial_char(start, coverage),
            Orientation::Horizontal => self.horizontal_partial_char(start, coverage),
        }
    }

    /// Returns a vertical partial block character.
    /// Uses 8-level precision with Unicode box-drawing characters.
    fn vertical_partial_char(&self, start: u32, coverage: u32) -> char {
        // With 8 virtual units per cell:
        // start 0, coverage 8 = full
        // start 0, coverage 4 = upper half
        // start 4, coverage 4 = lower half
        // etc.

        let eighths = (coverage * 8 / u32::from(self.virtual_resolution)).min(8);
        let is_lower = start >= u32::from(self.virtual_resolution) / 2;

        if is_lower {
            // Coverage starts in lower half
            match eighths {
                0 => ' ',
                1 => vertical_blocks::LOWER_EIGHTH,
                2 => vertical_blocks::LOWER_QUARTER,
                3 => vertical_blocks::LOWER_THREE_EIGHTHS,
                4 => vertical_blocks::LOWER_HALF,
                5 => vertical_blocks::LOWER_FIVE_EIGHTHS,
                6 => vertical_blocks::LOWER_THREE_QUARTERS,
                7 => vertical_blocks::LOWER_SEVEN_EIGHTHS,
                _ => vertical_blocks::FULL,
            }
        } else {
            // Coverage starts in upper half
            match eighths {
                0 => ' ',
                1..=3 => vertical_blocks::UPPER_EIGHTH,
                4 => vertical_blocks::UPPER_HALF,
                5..=7 => {
                    // Upper portion plus some lower
                    if coverage + start >= u32::from(self.virtual_resolution) {
                        vertical_blocks::FULL
                    } else {
                        vertical_blocks::UPPER_HALF
                    }
                }
                _ => vertical_blocks::FULL,
            }
        }
    }

    /// Returns a horizontal partial block character.
    fn horizontal_partial_char(&self, start: u32, coverage: u32) -> char {
        let eighths = (coverage * 8 / u32::from(self.virtual_resolution)).min(8);
        let is_right = start >= u32::from(self.virtual_resolution) / 2;

        if is_right {
            // Coverage starts in right half
            match eighths {
                0 => ' ',
                1..=3 => horizontal_blocks::RIGHT_HALF,
                _ => horizontal_blocks::FULL,
            }
        } else {
            // Coverage starts in left half
            match eighths {
                0 => ' ',
                1 => horizontal_blocks::LEFT_EIGHTH,
                2 => horizontal_blocks::LEFT_QUARTER,
                3 => horizontal_blocks::LEFT_THREE_EIGHTHS,
                4 => horizontal_blocks::LEFT_HALF,
                5 => horizontal_blocks::LEFT_FIVE_EIGHTHS,
                6 => horizontal_blocks::LEFT_THREE_QUARTERS,
                7 => horizontal_blocks::LEFT_SEVEN_EIGHTHS,
                _ => horizontal_blocks::FULL,
            }
        }
    }
}

/// Information about a rendered scrollbar for layout purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollbarMetrics {
    /// The bounding rectangle of the scrollbar
    pub bounds: Rect,
    /// The thumb start position (in track coordinates)
    pub thumb_start: u32,
    /// The thumb end position (in track coordinates, exclusive)
    pub thumb_end: u32,
    /// Whether the scrollbar is visible
    pub is_visible: bool,
}

impl ScrollbarMetrics {
    /// Returns the thumb rectangle in absolute coordinates.
    pub fn thumb_rect(&self, orientation: Orientation) -> Rect {
        match orientation {
            Orientation::Vertical => Rect::new(
                self.bounds.x,
                self.bounds.y + self.thumb_start as i32,
                self.bounds.width,
                (self.thumb_end - self.thumb_start) as u16,
            ),
            Orientation::Horizontal => Rect::new(
                self.bounds.x + self.thumb_start as i32,
                self.bounds.y,
                (self.thumb_end - self.thumb_start) as u16,
                self.bounds.height,
            ),
        }
    }
}

/// Builder for creating [`Scrollbar`] instances.
#[derive(Debug, Clone, Default)]
pub struct ScrollbarBuilder {
    orientation: Orientation,
    track_length: u16,
    visibility: ScrollbarVisibility,
    min_thumb_size: u16,
    viewport_ratio: Option<f32>,
    scroll_ratio: Option<f32>,
}

impl ScrollbarBuilder {
    /// Creates a new scrollbar builder.
    pub fn new() -> Self {
        Self {
            min_thumb_size: 1,
            track_length: 10,
            ..Default::default()
        }
    }

    /// Sets the orientation.
    pub fn orientation(mut self, orientation: Orientation) -> Self {
        self.orientation = orientation;
        self
    }

    /// Sets the orientation to vertical.
    pub fn vertical(mut self) -> Self {
        self.orientation = Orientation::Vertical;
        self
    }

    /// Sets the orientation to horizontal.
    pub fn horizontal(mut self) -> Self {
        self.orientation = Orientation::Horizontal;
        self
    }

    /// Sets the track length.
    pub fn track_length(mut self, length: u16) -> Self {
        self.track_length = length;
        self
    }

    /// Sets the visibility mode.
    pub fn visibility(mut self, visibility: ScrollbarVisibility) -> Self {
        self.visibility = visibility;
        self
    }

    /// Sets the minimum thumb size.
    pub fn min_thumb_size(mut self, size: u16) -> Self {
        self.min_thumb_size = size;
        self
    }

    /// Sets the viewport ratio.
    pub fn viewport_ratio(mut self, ratio: f32) -> Self {
        self.viewport_ratio = Some(ratio);
        self
    }

    /// Sets the scroll ratio.
    pub fn scroll_ratio(mut self, ratio: f32) -> Self {
        self.scroll_ratio = Some(ratio);
        self
    }

    /// Builds the scrollbar.
    pub fn build(self) -> Scrollbar {
        let mut scrollbar = Scrollbar {
            orientation: self.orientation,
            track_length: self.track_length.max(1),
            scroll_ratio: self.scroll_ratio.unwrap_or(0.0).clamp(0.0, 1.0),
            viewport_ratio: self.viewport_ratio.unwrap_or(1.0).clamp(0.0, 1.0),
            visibility: self.visibility,
            min_thumb_size: self.min_thumb_size.max(1),
            is_visible: false,
            virtual_resolution: 8,
        };
        scrollbar.update_visibility();
        scrollbar
    }
}

/// Scrollbar style configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollbarStyle {
    /// Width/height of the scrollbar track in cells (typically 1)
    pub thickness: u16,
    /// Whether to show track background
    pub show_track: bool,
    /// Minimum thumb size in cells
    pub min_thumb_size: u16,
    /// Padding from the edge of the container
    pub padding: u16,
}

impl Default for ScrollbarStyle {
    fn default() -> Self {
        Self {
            thickness: 1,
            show_track: true,
            min_thumb_size: 1,
            padding: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrollbar_thumb_full_viewport() {
        let scrollbar = ScrollbarBuilder::new()
            .track_length(10)
            .viewport_ratio(1.0)
            .build();

        // When viewport shows all content, thumb fills track
        assert_eq!(scrollbar.thumb_size(), 10);
        assert!(!scrollbar.is_visible()); // Auto-hide when not needed
    }

    #[test]
    fn test_scrollbar_thumb_half_viewport() {
        let scrollbar = ScrollbarBuilder::new()
            .track_length(10)
            .viewport_ratio(0.5)
            .visibility(ScrollbarVisibility::Always)
            .build();

        // When viewport shows half of content, thumb is half of track
        assert_eq!(scrollbar.thumb_size(), 5);
        assert!(scrollbar.is_visible());
    }

    #[test]
    fn test_scrollbar_thumb_position() {
        let mut scrollbar = ScrollbarBuilder::new()
            .track_length(10)
            .viewport_ratio(0.5)
            .visibility(ScrollbarVisibility::Always)
            .build();

        // At start
        scrollbar.set_scroll_ratio(0.0);
        assert_eq!(scrollbar.thumb_start(), 0);

        // At middle
        scrollbar.set_scroll_ratio(0.5);
        let start = scrollbar.thumb_start();
        let end = scrollbar.thumb_end();
        assert!(start > 0);
        assert!(end < 10);

        // At end
        scrollbar.set_scroll_ratio(1.0);
        assert_eq!(scrollbar.thumb_end(), 10);
    }

    #[test]
    fn test_scrollbar_render() {
        let scrollbar = ScrollbarBuilder::new()
            .track_length(5)
            .viewport_ratio(0.4) // Small thumb
            .scroll_ratio(0.0)
            .visibility(ScrollbarVisibility::Always)
            .build();

        let cells = scrollbar.render();
        assert_eq!(cells.len(), 5);

        // First cells should be thumb
        assert!(cells[0].is_thumb);
        // Last cell should be track
        assert!(!cells[4].is_thumb);
    }

    #[test]
    fn test_scrollbar_hit_test() {
        let scrollbar = ScrollbarBuilder::new()
            .track_length(10)
            .viewport_ratio(0.3)
            .scroll_ratio(0.5)
            .visibility(ScrollbarVisibility::Always)
            .build();

        let thumb_start = scrollbar.thumb_start();
        let thumb_end = scrollbar.thumb_end();

        assert!(scrollbar.hit_test_thumb(thumb_start));
        assert!(scrollbar.hit_test_thumb(thumb_end - 1));
        assert!(!scrollbar.hit_test_thumb(thumb_end));

        if thumb_start > 0 {
            assert!(scrollbar.hit_test_track_before(0));
        }
        if thumb_end < 10 {
            assert!(scrollbar.hit_test_track_after(9));
        }
    }

    #[test]
    fn test_scrollbar_position_to_ratio() {
        let scrollbar = ScrollbarBuilder::new()
            .track_length(10)
            .viewport_ratio(0.2)
            .visibility(ScrollbarVisibility::Always)
            .build();

        // Position 0 should give ratio near 0
        let ratio_start = scrollbar.position_to_ratio(0);
        assert!(ratio_start <= 0.1);

        // Position at end should give ratio near 1
        let ratio_end = scrollbar.position_to_ratio(9);
        assert!(ratio_end >= 0.9);
    }

    #[test]
    fn test_scrollbar_visibility() {
        let mut scrollbar = Scrollbar::vertical();
        scrollbar.set_track_length(10);

        // Auto visibility
        scrollbar.set_visibility(ScrollbarVisibility::Auto);
        scrollbar.set_viewport_ratio(1.0);
        assert!(!scrollbar.is_visible());

        scrollbar.set_viewport_ratio(0.5);
        assert!(scrollbar.is_visible());

        // Always visible
        scrollbar.set_visibility(ScrollbarVisibility::Always);
        scrollbar.set_viewport_ratio(1.0);
        assert!(scrollbar.is_visible());

        // Never visible
        scrollbar.set_visibility(ScrollbarVisibility::Never);
        scrollbar.set_viewport_ratio(0.5);
        assert!(!scrollbar.is_visible());
    }

    #[test]
    fn test_scrollbar_update_from_viewport() {
        let mut scrollbar = Scrollbar::vertical();
        scrollbar.set_track_length(20);
        scrollbar.set_visibility(ScrollbarVisibility::Always);

        scrollbar.update_from_viewport(50, 200, 75);

        // Viewport shows 50/200 = 0.25 of content
        assert!((scrollbar.viewport_ratio() - 0.25).abs() < 0.01);

        // Scroll is at 75 out of max 150 = 0.5
        assert!((scrollbar.scroll_ratio() - 0.5).abs() < 0.01);
    }
}
