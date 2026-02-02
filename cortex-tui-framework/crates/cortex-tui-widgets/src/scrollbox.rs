//! ScrollBox widget for scrollable content.
//!
//! The ScrollBox widget provides a scrollable container that can hold
//! content larger than its viewport. It supports:
//!
//! - Vertical and horizontal scrolling
//! - Sticky scroll behavior (stick to top/bottom)
//! - Scroll acceleration for smooth scrolling
//! - Mouse wheel and keyboard navigation
//! - Customizable scrollbars

use crate::scrollbar::{Orientation, Scrollbar, ScrollbarStyle, ScrollbarVisibility};
use crate::viewport::{Rect, ScrollOffset, Viewport};

/// Scroll direction for events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrollDirection {
    /// Scroll upward (content moves down)
    Up,
    /// Scroll downward (content moves up)
    Down,
    /// Scroll leftward (content moves right)
    Left,
    /// Scroll rightward (content moves left)
    Right,
}

impl ScrollDirection {
    /// Returns the opposite direction.
    #[inline]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }

    /// Returns the orientation axis.
    #[inline]
    pub const fn orientation(self) -> Orientation {
        match self {
            Self::Up | Self::Down => Orientation::Vertical,
            Self::Left | Self::Right => Orientation::Horizontal,
        }
    }

    /// Returns the sign (-1 or +1) for scroll offset calculation.
    #[inline]
    pub const fn sign(self) -> i32 {
        match self {
            Self::Up | Self::Left => -1,
            Self::Down | Self::Right => 1,
        }
    }
}

/// Unit for scroll operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollUnit {
    /// Single line/column
    #[default]
    Line,
    /// Page (viewport size)
    Page,
    /// Absolute pixel/cell value
    Absolute,
    /// Fraction of content size (0.0 to 1.0)
    Fraction,
}

/// Sticky scroll position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StickyPosition {
    /// Stick to top edge
    Top,
    /// Stick to bottom edge
    Bottom,
    /// Stick to left edge
    Left,
    /// Stick to right edge
    Right,
}

/// Sticky scroll state tracking.
#[derive(Debug, Clone, Copy, Default)]
pub struct StickyState {
    /// Whether sticky scroll is enabled
    pub enabled: bool,
    /// Initial sticky position (where content starts pinned)
    pub sticky_start: Option<StickyPosition>,
    /// Whether currently stuck to top
    pub at_top: bool,
    /// Whether currently stuck to bottom
    pub at_bottom: bool,
    /// Whether currently stuck to left
    pub at_left: bool,
    /// Whether currently stuck to right
    pub at_right: bool,
    /// Whether user has manually scrolled (breaks sticky behavior)
    pub has_manual_scroll: bool,
    /// Guard flag to prevent recursive updates
    is_applying: bool,
}

impl StickyState {
    /// Creates a new sticky state with the given start position.
    pub fn with_start(position: StickyPosition) -> Self {
        let mut state = Self {
            enabled: true,
            sticky_start: Some(position),
            ..Default::default()
        };
        // Initialize the appropriate sticky flag
        match position {
            StickyPosition::Top => state.at_top = true,
            StickyPosition::Bottom => state.at_bottom = true,
            StickyPosition::Left => state.at_left = true,
            StickyPosition::Right => state.at_right = true,
        }
        state
    }

    /// Resets the manual scroll flag.
    pub fn reset_manual_scroll(&mut self) {
        self.has_manual_scroll = false;
    }
}

/// Scroll acceleration configuration.
#[derive(Debug, Clone, Copy)]
pub struct ScrollAccelConfig {
    /// Size of velocity history window
    pub history_size: usize,
    /// Timeout before acceleration resets (milliseconds)
    pub streak_timeout_ms: u64,
    /// Minimum interval between ticks (milliseconds)
    pub min_tick_interval_ms: u64,
    /// Exponential curve amplitude
    pub amplitude: f32,
    /// Time constant for curve shape
    pub time_constant: f32,
    /// Maximum multiplier cap
    pub max_multiplier: f32,
}

impl Default for ScrollAccelConfig {
    fn default() -> Self {
        Self {
            history_size: 3,
            streak_timeout_ms: 150,
            min_tick_interval_ms: 6,
            amplitude: 0.8,
            time_constant: 3.0,
            max_multiplier: 6.0,
        }
    }
}

impl ScrollAccelConfig {
    /// Returns a config with no acceleration (linear scrolling).
    pub const fn linear() -> Self {
        Self {
            history_size: 1,
            streak_timeout_ms: 0,
            min_tick_interval_ms: 0,
            amplitude: 0.0,
            time_constant: 1.0,
            max_multiplier: 1.0,
        }
    }

    /// Returns a config optimized for macOS-style scrolling.
    pub const fn macos() -> Self {
        Self {
            history_size: 3,
            streak_timeout_ms: 150,
            min_tick_interval_ms: 6,
            amplitude: 0.8,
            time_constant: 3.0,
            max_multiplier: 6.0,
        }
    }
}

/// Scroll acceleration state.
#[derive(Debug, Clone)]
struct ScrollAcceleration {
    config: ScrollAccelConfig,
    last_tick_time_ms: u64,
    velocity_history: smallvec::SmallVec<[u64; 8]>,
}

impl Default for ScrollAcceleration {
    fn default() -> Self {
        Self::new(ScrollAccelConfig::default())
    }
}

impl ScrollAcceleration {
    /// Creates a new acceleration tracker with the given config.
    fn new(config: ScrollAccelConfig) -> Self {
        Self {
            config,
            last_tick_time_ms: 0,
            velocity_history: smallvec::SmallVec::new(),
        }
    }

    /// Processes a scroll tick and returns the acceleration multiplier.
    fn tick(&mut self, now_ms: u64) -> f32 {
        // Linear mode - no acceleration
        if self.config.amplitude <= 0.0 {
            return 1.0;
        }

        let dt = if self.last_tick_time_ms == 0 {
            u64::MAX
        } else {
            now_ms.saturating_sub(self.last_tick_time_ms)
        };

        // Reset streak if timeout or first tick
        if dt == u64::MAX || dt > self.config.streak_timeout_ms {
            self.last_tick_time_ms = now_ms;
            self.velocity_history.clear();
            return 1.0;
        }

        // Ignore ticks that are too close (likely terminal double-sends)
        if dt < self.config.min_tick_interval_ms {
            return 1.0;
        }

        self.last_tick_time_ms = now_ms;

        // Add to history, maintain window size
        self.velocity_history.push(dt);
        if self.velocity_history.len() > self.config.history_size {
            self.velocity_history.remove(0);
        }

        // Calculate average interval
        let avg_interval: f32 = self.velocity_history.iter().map(|&v| v as f32).sum::<f32>()
            / self.velocity_history.len() as f32;

        // Convert to velocity (faster ticks = higher velocity)
        // Reference: 100ms interval = velocity of 1.0
        let velocity = 100.0 / avg_interval;

        // Apply exponential curve: 1 + A * (exp(v/τ) - 1)
        let x = velocity / self.config.time_constant;
        let multiplier = 1.0 + self.config.amplitude * (x.exp() - 1.0);

        multiplier.min(self.config.max_multiplier)
    }

    /// Resets the acceleration state.
    fn reset(&mut self) {
        self.last_tick_time_ms = 0;
        self.velocity_history.clear();
    }
}

/// Sub-pixel scroll accumulator for smooth fractional scrolling.
#[derive(Debug, Clone, Copy, Default)]
struct ScrollAccumulator {
    x: f32,
    y: f32,
}

impl ScrollAccumulator {
    /// Adds to the accumulator and returns the integer scroll amount.
    fn accumulate(&mut self, dx: f32, dy: f32) -> (i32, i32) {
        self.x += dx;
        self.y += dy;

        let int_x = self.x.trunc() as i32;
        let int_y = self.y.trunc() as i32;

        if int_x != 0 {
            self.x -= int_x as f32;
        }
        if int_y != 0 {
            self.y -= int_y as f32;
        }

        (int_x, int_y)
    }

    /// Resets the accumulator.
    fn reset(&mut self) {
        self.x = 0.0;
        self.y = 0.0;
    }
}

/// ScrollBox configuration.
#[derive(Debug, Clone)]
pub struct ScrollBoxConfig {
    /// Enable vertical scrolling
    pub enable_vertical: bool,
    /// Enable horizontal scrolling
    pub enable_horizontal: bool,
    /// Vertical scrollbar visibility
    pub vertical_scrollbar: ScrollbarVisibility,
    /// Horizontal scrollbar visibility
    pub horizontal_scrollbar: ScrollbarVisibility,
    /// Scrollbar style
    pub scrollbar_style: ScrollbarStyle,
    /// Scroll acceleration config
    pub acceleration: ScrollAccelConfig,
    /// Line scroll amount (for keyboard/wheel)
    pub line_scroll: u32,
    /// Whether to enable sticky scroll
    pub sticky_scroll: Option<StickyPosition>,
    /// Whether to swap scroll axes with shift key
    pub shift_swaps_axes: bool,
}

impl Default for ScrollBoxConfig {
    fn default() -> Self {
        Self {
            enable_vertical: true,
            enable_horizontal: false,
            vertical_scrollbar: ScrollbarVisibility::Auto,
            horizontal_scrollbar: ScrollbarVisibility::Auto,
            scrollbar_style: ScrollbarStyle::default(),
            acceleration: ScrollAccelConfig::default(),
            line_scroll: 3,
            sticky_scroll: None,
            shift_swaps_axes: true,
        }
    }
}

/// A scrollable container widget.
///
/// ScrollBox manages a viewport over content that may be larger than the
/// visible area. It handles scroll state, scrollbar rendering, and
/// input events for navigation.
#[derive(Debug, Clone)]
pub struct ScrollBox {
    /// Viewport management
    viewport: Viewport,
    /// Vertical scrollbar
    vertical_scrollbar: Scrollbar,
    /// Horizontal scrollbar
    horizontal_scrollbar: Scrollbar,
    /// Sticky scroll state
    sticky: StickyState,
    /// Scroll acceleration
    acceleration: ScrollAcceleration,
    /// Sub-pixel accumulator
    accumulator: ScrollAccumulator,
    /// Configuration
    config: ScrollBoxConfig,
    /// Whether the scrollbox needs redraw
    needs_redraw: bool,
}

impl Default for ScrollBox {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollBox {
    /// Creates a new ScrollBox with default configuration.
    pub fn new() -> Self {
        Self::with_config(ScrollBoxConfig::default())
    }

    /// Creates a new ScrollBox with the given configuration.
    pub fn with_config(config: ScrollBoxConfig) -> Self {
        let mut vertical_scrollbar = Scrollbar::vertical();
        vertical_scrollbar.set_visibility(config.vertical_scrollbar);
        vertical_scrollbar.set_min_thumb_size(config.scrollbar_style.min_thumb_size);

        let mut horizontal_scrollbar = Scrollbar::horizontal();
        horizontal_scrollbar.set_visibility(config.horizontal_scrollbar);
        horizontal_scrollbar.set_min_thumb_size(config.scrollbar_style.min_thumb_size);

        let sticky = config
            .sticky_scroll
            .map(StickyState::with_start)
            .unwrap_or_default();

        Self {
            viewport: Viewport::default(),
            vertical_scrollbar,
            horizontal_scrollbar,
            sticky,
            acceleration: ScrollAcceleration::new(config.acceleration),
            accumulator: ScrollAccumulator::default(),
            config,
            needs_redraw: true,
        }
    }

    // -------------------------------------------------------------------------
    // Viewport management
    // -------------------------------------------------------------------------

    /// Returns the viewport.
    #[inline]
    pub const fn viewport(&self) -> &Viewport {
        &self.viewport
    }

    /// Returns a mutable reference to the viewport.
    #[inline]
    pub fn viewport_mut(&mut self) -> &mut Viewport {
        &mut self.viewport
    }

    /// Sets the viewport size.
    pub fn set_viewport_size(&mut self, width: u16, height: u16) {
        self.viewport.set_viewport_size(width, height);
        self.update_scrollbars();
        self.apply_sticky_if_needed();
        self.needs_redraw = true;
    }

    /// Sets the content size.
    pub fn set_content_size(&mut self, width: u16, height: u16) {
        self.viewport.set_content_size(width, height);
        self.update_scrollbars();
        self.apply_sticky_if_needed();
        self.needs_redraw = true;
    }

    /// Returns the viewport rect.
    #[inline]
    pub const fn viewport_rect(&self) -> Rect {
        self.viewport.viewport_rect()
    }

    /// Returns the content rect.
    #[inline]
    pub const fn content_rect(&self) -> Rect {
        self.viewport.content_rect()
    }

    /// Returns the current scroll offset.
    #[inline]
    pub const fn scroll_offset(&self) -> ScrollOffset {
        self.viewport.offset()
    }

    /// Returns the visible content rect.
    pub fn visible_content_rect(&self) -> Rect {
        self.viewport.visible_content_rect()
    }

    // -------------------------------------------------------------------------
    // Scroll position
    // -------------------------------------------------------------------------

    /// Returns the horizontal scroll position.
    #[inline]
    pub const fn scroll_x(&self) -> i32 {
        self.viewport.scroll_x()
    }

    /// Returns the vertical scroll position.
    #[inline]
    pub const fn scroll_y(&self) -> i32 {
        self.viewport.scroll_y()
    }

    /// Sets the horizontal scroll position.
    pub fn set_scroll_x(&mut self, x: i32) {
        self.mark_manual_scroll();
        self.viewport.set_scroll_x(x);
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
    }

    /// Sets the vertical scroll position.
    pub fn set_scroll_y(&mut self, y: i32) {
        self.mark_manual_scroll();
        self.viewport.set_scroll_y(y);
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
    }

    /// Sets both scroll positions.
    pub fn set_scroll(&mut self, x: i32, y: i32) {
        self.mark_manual_scroll();
        self.viewport.set_scroll(x, y);
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
    }

    /// Sets the scroll position from ratios (0.0 to 1.0).
    pub fn set_scroll_ratio(&mut self, x_ratio: f32, y_ratio: f32) {
        self.mark_manual_scroll();
        self.viewport.set_scroll_ratio_x(x_ratio);
        self.viewport.set_scroll_ratio_y(y_ratio);
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
    }

    // -------------------------------------------------------------------------
    // Scroll operations
    // -------------------------------------------------------------------------

    /// Scrolls by the given delta.
    pub fn scroll_by(&mut self, dx: i32, dy: i32) -> (i32, i32) {
        if dx == 0 && dy == 0 {
            return (0, 0);
        }

        self.mark_manual_scroll();
        let result = self.viewport.scroll_by(dx, dy);
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
        result
    }

    /// Scrolls by line(s) in the given direction.
    pub fn scroll_lines(&mut self, direction: ScrollDirection, lines: u32) {
        let amount = (lines * self.config.line_scroll) as i32 * direction.sign();
        match direction.orientation() {
            Orientation::Vertical => {
                self.scroll_by(0, amount);
            }
            Orientation::Horizontal => {
                self.scroll_by(amount, 0);
            }
        }
    }

    /// Scrolls by one page in the given direction.
    pub fn scroll_page(&mut self, direction: ScrollDirection) {
        self.mark_manual_scroll();
        match direction {
            ScrollDirection::Up => {
                self.viewport.page_up();
            }
            ScrollDirection::Down => {
                self.viewport.page_down();
            }
            ScrollDirection::Left => {
                self.viewport.page_left();
            }
            ScrollDirection::Right => {
                self.viewport.page_right();
            }
        }
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
    }

    /// Scrolls to the given position.
    pub fn scroll_to(&mut self, direction: ScrollDirection) {
        self.mark_manual_scroll();
        match direction {
            ScrollDirection::Up => self.viewport.scroll_to_top(),
            ScrollDirection::Down => self.viewport.scroll_to_bottom(),
            ScrollDirection::Left => self.viewport.scroll_to_left(),
            ScrollDirection::Right => self.viewport.scroll_to_right(),
        }
        self.update_scrollbars();
        self.update_sticky_state();
        self.needs_redraw = true;
    }

    /// Scrolls to the top.
    pub fn scroll_to_top(&mut self) {
        self.scroll_to(ScrollDirection::Up);
    }

    /// Scrolls to the bottom.
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_to(ScrollDirection::Down);
    }

    /// Scrolls to make a point visible.
    pub fn scroll_to_point(&mut self, x: i32, y: i32) -> bool {
        self.mark_manual_scroll();
        let scrolled = self.viewport.scroll_to_point(x, y);
        if scrolled {
            self.update_scrollbars();
            self.update_sticky_state();
            self.needs_redraw = true;
        }
        scrolled
    }

    /// Scrolls to make a rectangle visible.
    pub fn scroll_to_rect(&mut self, rect: &Rect) -> bool {
        self.mark_manual_scroll();
        let scrolled = self.viewport.scroll_to_rect(rect);
        if scrolled {
            self.update_scrollbars();
            self.update_sticky_state();
            self.needs_redraw = true;
        }
        scrolled
    }

    /// Scrolls to center a point in the viewport.
    pub fn scroll_to_center(&mut self, x: i32, y: i32) -> bool {
        self.mark_manual_scroll();
        let scrolled = self.viewport.scroll_to_center_point(x, y);
        if scrolled {
            self.update_scrollbars();
            self.update_sticky_state();
            self.needs_redraw = true;
        }
        scrolled
    }

    // -------------------------------------------------------------------------
    // Scroll with acceleration
    // -------------------------------------------------------------------------

    /// Handles a scroll event with acceleration.
    /// `now_ms` is the current timestamp in milliseconds.
    pub fn handle_scroll_event(
        &mut self,
        direction: ScrollDirection,
        delta: f32,
        now_ms: u64,
        shift_held: bool,
    ) {
        // Optionally swap axes with shift key
        let direction = if shift_held && self.config.shift_swaps_axes {
            match direction {
                ScrollDirection::Up => ScrollDirection::Left,
                ScrollDirection::Down => ScrollDirection::Right,
                ScrollDirection::Left => ScrollDirection::Up,
                ScrollDirection::Right => ScrollDirection::Down,
            }
        } else {
            direction
        };

        // Check if scrolling is enabled for this direction
        let can_scroll = match direction.orientation() {
            Orientation::Vertical => self.config.enable_vertical,
            Orientation::Horizontal => self.config.enable_horizontal,
        };

        if !can_scroll {
            return;
        }

        // Get acceleration multiplier
        let multiplier = self.acceleration.tick(now_ms);
        let scroll_amount = delta * multiplier * direction.sign() as f32;

        // Accumulate sub-pixel amounts
        let (int_x, int_y) = match direction.orientation() {
            Orientation::Vertical => self.accumulator.accumulate(0.0, scroll_amount),
            Orientation::Horizontal => self.accumulator.accumulate(scroll_amount, 0.0),
        };

        if int_x != 0 || int_y != 0 {
            self.scroll_by(int_x, int_y);
        }
    }

    /// Resets scroll acceleration state.
    pub fn reset_acceleration(&mut self) {
        self.acceleration.reset();
        self.accumulator.reset();
    }

    // -------------------------------------------------------------------------
    // Keyboard handling
    // -------------------------------------------------------------------------

    /// Handles a keyboard event for scrolling.
    /// Returns true if the event was handled.
    pub fn handle_key(&mut self, key: &str, ctrl: bool, _shift: bool) -> bool {
        match key {
            // Arrow keys
            "up" | "k" => {
                self.scroll_lines(ScrollDirection::Up, 1);
                true
            }
            "down" | "j" => {
                self.scroll_lines(ScrollDirection::Down, 1);
                true
            }
            "left" | "h" => {
                self.scroll_lines(ScrollDirection::Left, 1);
                true
            }
            "right" | "l" => {
                self.scroll_lines(ScrollDirection::Right, 1);
                true
            }

            // Page navigation
            "pageup" | "page_up" => {
                self.scroll_page(ScrollDirection::Up);
                true
            }
            "pagedown" | "page_down" => {
                self.scroll_page(ScrollDirection::Down);
                true
            }

            // Home/End
            "home" => {
                if ctrl {
                    self.scroll_to_top();
                } else {
                    self.scroll_to(ScrollDirection::Left);
                }
                true
            }
            "end" => {
                if ctrl {
                    self.scroll_to_bottom();
                } else {
                    self.scroll_to(ScrollDirection::Right);
                }
                true
            }

            // Ctrl+Home / Ctrl+End (vim gg/G)
            "g" if ctrl => {
                self.scroll_to_top();
                true
            }
            "G" => {
                self.scroll_to_bottom();
                true
            }

            _ => false,
        }
    }

    // -------------------------------------------------------------------------
    // Scrollbar access
    // -------------------------------------------------------------------------

    /// Returns the vertical scrollbar.
    #[inline]
    pub const fn vertical_scrollbar(&self) -> &Scrollbar {
        &self.vertical_scrollbar
    }

    /// Returns the horizontal scrollbar.
    #[inline]
    pub const fn horizontal_scrollbar(&self) -> &Scrollbar {
        &self.horizontal_scrollbar
    }

    /// Returns whether the vertical scrollbar is visible.
    #[inline]
    pub fn is_vertical_scrollbar_visible(&self) -> bool {
        self.config.enable_vertical && self.vertical_scrollbar.is_visible()
    }

    /// Returns whether the horizontal scrollbar is visible.
    #[inline]
    pub fn is_horizontal_scrollbar_visible(&self) -> bool {
        self.config.enable_horizontal && self.horizontal_scrollbar.is_visible()
    }

    /// Updates scrollbar state from viewport.
    fn update_scrollbars(&mut self) {
        // Update vertical scrollbar
        if self.config.enable_vertical {
            self.vertical_scrollbar.update_from_viewport(
                self.viewport.viewport_height(),
                self.viewport.content_height(),
                self.viewport.scroll_y(),
            );
        }

        // Update horizontal scrollbar
        if self.config.enable_horizontal {
            self.horizontal_scrollbar.update_from_viewport(
                self.viewport.viewport_width(),
                self.viewport.content_width(),
                self.viewport.scroll_x(),
            );
        }
    }

    // -------------------------------------------------------------------------
    // Sticky scroll
    // -------------------------------------------------------------------------

    /// Returns the sticky scroll state.
    #[inline]
    pub const fn sticky_state(&self) -> &StickyState {
        &self.sticky
    }

    /// Sets whether sticky scroll is enabled.
    pub fn set_sticky_enabled(&mut self, enabled: bool) {
        self.sticky.enabled = enabled;
        if enabled {
            self.apply_sticky_if_needed();
        }
    }

    /// Sets the sticky start position.
    pub fn set_sticky_start(&mut self, position: Option<StickyPosition>) {
        self.sticky.sticky_start = position;
        self.sticky.enabled = position.is_some();
        if let Some(pos) = position {
            self.sticky = StickyState::with_start(pos);
            self.apply_sticky_position(pos);
        }
    }

    /// Resets manual scroll flag, allowing sticky behavior to resume.
    pub fn reset_sticky(&mut self) {
        self.sticky.has_manual_scroll = false;
        self.apply_sticky_if_needed();
    }

    fn mark_manual_scroll(&mut self) {
        if !self.sticky.is_applying {
            // Only mark as manual scroll if there's meaningful scrollable content
            if self.viewport.can_scroll() {
                self.sticky.has_manual_scroll = true;
            }
        }
    }

    fn update_sticky_state(&mut self) {
        if !self.sticky.enabled {
            return;
        }

        self.sticky.at_top = self.viewport.is_at_top();
        self.sticky.at_bottom = self.viewport.is_at_bottom();
        self.sticky.at_left = self.viewport.is_at_left();
        self.sticky.at_right = self.viewport.is_at_right();
    }

    fn apply_sticky_if_needed(&mut self) {
        if !self.sticky.enabled || self.sticky.has_manual_scroll {
            return;
        }

        if let Some(position) = self.sticky.sticky_start {
            self.apply_sticky_position(position);
        }
    }

    fn apply_sticky_position(&mut self, position: StickyPosition) {
        self.sticky.is_applying = true;

        match position {
            StickyPosition::Top => {
                self.viewport.scroll_to_top();
                self.sticky.at_top = true;
                self.sticky.at_bottom = false;
            }
            StickyPosition::Bottom => {
                self.viewport.scroll_to_bottom();
                self.sticky.at_top = false;
                self.sticky.at_bottom = true;
            }
            StickyPosition::Left => {
                self.viewport.scroll_to_left();
                self.sticky.at_left = true;
                self.sticky.at_right = false;
            }
            StickyPosition::Right => {
                self.viewport.scroll_to_right();
                self.sticky.at_left = false;
                self.sticky.at_right = true;
            }
        }

        self.update_scrollbars();
        self.sticky.is_applying = false;
        self.needs_redraw = true;
    }

    /// Returns whether the scroll position is at the sticky start position.
    pub fn is_at_sticky_position(&self) -> bool {
        match self.sticky.sticky_start {
            None => false,
            Some(StickyPosition::Top) => self.viewport.is_at_top(),
            Some(StickyPosition::Bottom) => self.viewport.is_at_bottom(),
            Some(StickyPosition::Left) => self.viewport.is_at_left(),
            Some(StickyPosition::Right) => self.viewport.is_at_right(),
        }
    }

    // -------------------------------------------------------------------------
    // Visibility checks
    // -------------------------------------------------------------------------

    /// Checks if a point in content coordinates is visible.
    #[inline]
    pub fn is_point_visible(&self, x: i32, y: i32) -> bool {
        self.viewport.is_point_visible(x, y)
    }

    /// Checks if a rectangle in content coordinates is at least partially visible.
    #[inline]
    pub fn is_rect_visible(&self, rect: &Rect) -> bool {
        self.viewport.is_rect_visible(rect)
    }

    /// Checks if a rectangle in content coordinates is fully visible.
    #[inline]
    pub fn is_rect_fully_visible(&self, rect: &Rect) -> bool {
        self.viewport.is_rect_fully_visible(rect)
    }

    /// Converts content coordinates to viewport coordinates.
    #[inline]
    pub fn content_to_viewport(&self, x: i32, y: i32) -> (i32, i32) {
        self.viewport.content_to_viewport(x, y)
    }

    /// Converts viewport coordinates to content coordinates.
    #[inline]
    pub fn viewport_to_content(&self, x: i32, y: i32) -> (i32, i32) {
        self.viewport.viewport_to_content(x, y)
    }

    // -------------------------------------------------------------------------
    // State
    // -------------------------------------------------------------------------

    /// Returns whether the scrollbox needs redraw.
    #[inline]
    pub const fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Clears the needs_redraw flag.
    pub fn clear_redraw(&mut self) {
        self.needs_redraw = false;
    }

    /// Returns the configuration.
    #[inline]
    pub const fn config(&self) -> &ScrollBoxConfig {
        &self.config
    }
}

/// Builder for creating [`ScrollBox`] instances.
#[derive(Debug, Clone, Default)]
pub struct ScrollBoxBuilder {
    viewport_width: u16,
    viewport_height: u16,
    content_width: Option<u16>,
    content_height: Option<u16>,
    enable_vertical: bool,
    enable_horizontal: bool,
    vertical_scrollbar: ScrollbarVisibility,
    horizontal_scrollbar: ScrollbarVisibility,
    scrollbar_style: ScrollbarStyle,
    acceleration: Option<ScrollAccelConfig>,
    line_scroll: u32,
    sticky_scroll: Option<StickyPosition>,
    shift_swaps_axes: bool,
    initial_scroll: Option<(i32, i32)>,
}

impl ScrollBoxBuilder {
    /// Creates a new ScrollBox builder.
    pub fn new() -> Self {
        Self {
            enable_vertical: true,
            enable_horizontal: false,
            vertical_scrollbar: ScrollbarVisibility::Auto,
            horizontal_scrollbar: ScrollbarVisibility::Auto,
            scrollbar_style: ScrollbarStyle::default(),
            line_scroll: 3,
            shift_swaps_axes: true,
            ..Default::default()
        }
    }

    /// Sets the viewport size.
    pub fn viewport_size(mut self, width: u16, height: u16) -> Self {
        self.viewport_width = width;
        self.viewport_height = height;
        self
    }

    /// Sets the content size.
    pub fn content_size(mut self, width: u16, height: u16) -> Self {
        self.content_width = Some(width);
        self.content_height = Some(height);
        self
    }

    /// Enables or disables vertical scrolling.
    pub fn vertical(mut self, enabled: bool) -> Self {
        self.enable_vertical = enabled;
        self
    }

    /// Enables or disables horizontal scrolling.
    pub fn horizontal(mut self, enabled: bool) -> Self {
        self.enable_horizontal = enabled;
        self
    }

    /// Sets the vertical scrollbar visibility.
    pub fn vertical_scrollbar(mut self, visibility: ScrollbarVisibility) -> Self {
        self.vertical_scrollbar = visibility;
        self
    }

    /// Sets the horizontal scrollbar visibility.
    pub fn horizontal_scrollbar(mut self, visibility: ScrollbarVisibility) -> Self {
        self.horizontal_scrollbar = visibility;
        self
    }

    /// Sets both scrollbar visibilities.
    pub fn scrollbar_visibility(mut self, visibility: ScrollbarVisibility) -> Self {
        self.vertical_scrollbar = visibility;
        self.horizontal_scrollbar = visibility;
        self
    }

    /// Sets the scrollbar style.
    pub fn scrollbar_style(mut self, style: ScrollbarStyle) -> Self {
        self.scrollbar_style = style;
        self
    }

    /// Sets the scroll acceleration configuration.
    pub fn acceleration(mut self, config: ScrollAccelConfig) -> Self {
        self.acceleration = Some(config);
        self
    }

    /// Disables scroll acceleration (linear scrolling).
    pub fn no_acceleration(mut self) -> Self {
        self.acceleration = Some(ScrollAccelConfig::linear());
        self
    }

    /// Sets the line scroll amount.
    pub fn line_scroll(mut self, amount: u32) -> Self {
        self.line_scroll = amount;
        self
    }

    /// Sets the sticky scroll position.
    pub fn sticky(mut self, position: StickyPosition) -> Self {
        self.sticky_scroll = Some(position);
        self
    }

    /// Sets sticky scroll to bottom (common for chat/log views).
    pub fn sticky_bottom(self) -> Self {
        self.sticky(StickyPosition::Bottom)
    }

    /// Sets sticky scroll to top.
    pub fn sticky_top(self) -> Self {
        self.sticky(StickyPosition::Top)
    }

    /// Configures whether shift key swaps scroll axes.
    pub fn shift_swaps_axes(mut self, enabled: bool) -> Self {
        self.shift_swaps_axes = enabled;
        self
    }

    /// Sets the initial scroll position.
    pub fn initial_scroll(mut self, x: i32, y: i32) -> Self {
        self.initial_scroll = Some((x, y));
        self
    }

    /// Builds the ScrollBox.
    pub fn build(self) -> ScrollBox {
        let config = ScrollBoxConfig {
            enable_vertical: self.enable_vertical,
            enable_horizontal: self.enable_horizontal,
            vertical_scrollbar: self.vertical_scrollbar,
            horizontal_scrollbar: self.horizontal_scrollbar,
            scrollbar_style: self.scrollbar_style,
            acceleration: self.acceleration.unwrap_or_default(),
            line_scroll: self.line_scroll.max(1),
            sticky_scroll: self.sticky_scroll,
            shift_swaps_axes: self.shift_swaps_axes,
        };

        let mut scrollbox = ScrollBox::with_config(config);

        // Set viewport and content sizes
        let content_width = self.content_width.unwrap_or(self.viewport_width);
        let content_height = self.content_height.unwrap_or(self.viewport_height);

        scrollbox.viewport = Viewport::with_content(
            self.viewport_width,
            self.viewport_height,
            content_width,
            content_height,
        );

        // Set initial scroll position
        if let Some((x, y)) = self.initial_scroll {
            scrollbox.sticky.has_manual_scroll = true;
            scrollbox.viewport.set_scroll(x, y);
        }

        // Update scrollbar track lengths
        scrollbox
            .vertical_scrollbar
            .set_track_length(self.viewport_height.saturating_sub(
                if scrollbox.config.enable_horizontal && scrollbox.horizontal_scrollbar.is_visible()
                {
                    scrollbox.config.scrollbar_style.thickness
                } else {
                    0
                },
            ));

        scrollbox
            .horizontal_scrollbar
            .set_track_length(self.viewport_width.saturating_sub(
                if scrollbox.config.enable_vertical && scrollbox.vertical_scrollbar.is_visible() {
                    scrollbox.config.scrollbar_style.thickness
                } else {
                    0
                },
            ));

        scrollbox.update_scrollbars();

        // Apply sticky position if set and no manual scroll
        if !scrollbox.sticky.has_manual_scroll {
            scrollbox.apply_sticky_if_needed();
        }

        scrollbox
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scrollbox_creation() {
        let scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .build();

        assert_eq!(scrollbox.viewport.viewport_width(), 80);
        assert_eq!(scrollbox.viewport.viewport_height(), 24);
        assert_eq!(scrollbox.viewport.content_width(), 80);
        assert_eq!(scrollbox.viewport.content_height(), 100);
    }

    #[test]
    fn test_scrollbox_scroll_by() {
        let mut scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .build();

        let (dx, dy) = scrollbox.scroll_by(0, 10);
        assert_eq!(dy, 10);
        assert_eq!(scrollbox.scroll_y(), 10);
        assert_eq!(dx, 0);
    }

    #[test]
    fn test_scrollbox_scroll_clamping() {
        let mut scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .build();

        scrollbox.scroll_by(0, 1000);
        assert_eq!(scrollbox.scroll_y(), 76); // 100 - 24

        scrollbox.scroll_by(0, -1000);
        assert_eq!(scrollbox.scroll_y(), 0);
    }

    #[test]
    fn test_scrollbox_sticky_bottom() {
        let mut scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .sticky_bottom()
            .build();

        // Should start at bottom
        assert_eq!(scrollbox.scroll_y(), 76);
        assert!(scrollbox.is_at_sticky_position());

        // Scroll up manually
        scrollbox.scroll_by(0, -10);
        assert_eq!(scrollbox.scroll_y(), 66);
        assert!(scrollbox.sticky.has_manual_scroll);

        // Content grows
        scrollbox.set_content_size(80, 200);
        // Should NOT auto-scroll because manual scroll was used
        assert_eq!(scrollbox.scroll_y(), 66);

        // Reset sticky
        scrollbox.reset_sticky();
        // Now should jump to bottom
        assert_eq!(scrollbox.scroll_y(), 176); // 200 - 24
    }

    #[test]
    fn test_scrollbox_keyboard_navigation() {
        let mut scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .line_scroll(3)
            .build();

        assert!(scrollbox.handle_key("down", false, false));
        assert_eq!(scrollbox.scroll_y(), 3);

        assert!(scrollbox.handle_key("up", false, false));
        assert_eq!(scrollbox.scroll_y(), 0);

        assert!(scrollbox.handle_key("pagedown", false, false));
        assert_eq!(scrollbox.scroll_y(), 23); // viewport height - 1

        assert!(scrollbox.handle_key("end", true, false));
        assert_eq!(scrollbox.scroll_y(), 76);

        assert!(scrollbox.handle_key("home", true, false));
        assert_eq!(scrollbox.scroll_y(), 0);
    }

    #[test]
    fn test_scrollbox_visibility_check() {
        let scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .build();

        // Point at origin should be visible
        assert!(scrollbox.is_point_visible(0, 0));

        // Point below viewport should not be visible
        assert!(!scrollbox.is_point_visible(0, 50));

        // Rect partially in viewport
        let rect = Rect::new(0, 20, 10, 10);
        assert!(scrollbox.is_rect_visible(&rect));
        assert!(!scrollbox.is_rect_fully_visible(&rect));
    }

    #[test]
    fn test_scroll_acceleration() {
        let mut accel = ScrollAcceleration::new(ScrollAccelConfig::macos());

        // First tick - no acceleration
        let mult1 = accel.tick(0);
        assert_eq!(mult1, 1.0);

        // Second tick after long delay - reset, no acceleration
        let mult2 = accel.tick(200);
        assert_eq!(mult2, 1.0);

        // Rapid ticks should build up acceleration
        let mult3 = accel.tick(250); // 50ms later
        let mult4 = accel.tick(300); // 50ms later
        let mult5 = accel.tick(350); // 50ms later

        // Acceleration should be increasing
        assert!(mult4 >= mult3);
        assert!(mult5 >= mult4);
    }

    #[test]
    fn test_scroll_accumulator() {
        let mut accum = ScrollAccumulator::default();

        // Small fractional amounts should accumulate
        let (x1, y1) = accum.accumulate(0.3, 0.3);
        assert_eq!((x1, y1), (0, 0));

        let (x2, y2) = accum.accumulate(0.3, 0.3);
        assert_eq!((x2, y2), (0, 0));

        let (x3, y3) = accum.accumulate(0.3, 0.3);
        assert_eq!((x3, y3), (0, 0));

        // Fourth addition should push over 1.0
        let (x4, y4) = accum.accumulate(0.3, 0.3);
        assert_eq!((x4, y4), (1, 1));
    }

    #[test]
    fn test_scrollbox_coordinate_conversion() {
        let mut scrollbox = ScrollBoxBuilder::new()
            .viewport_size(80, 24)
            .content_size(80, 100)
            .build();

        scrollbox.set_scroll(0, 20);

        // Content (0, 20) should map to viewport (0, 0)
        let (vx, vy) = scrollbox.content_to_viewport(0, 20);
        assert_eq!((vx, vy), (0, 0));

        // Viewport (0, 0) should map to content (0, 20)
        let (cx, cy) = scrollbox.viewport_to_content(0, 0);
        assert_eq!((cx, cy), (0, 20));
    }
}
