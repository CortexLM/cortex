//! Toast Notification System
//!
//! Provides temporary notification messages (success, warning, error, info)
//! displayed in configurable screen positions with auto-dismiss and fade animation.
//!
//! ## Usage
//!
//! ```ignore
//! use cortex_tui::widgets::{Toast, ToastLevel, ToastManager, ToastWidget};
//!
//! let mut manager = ToastManager::new()
//!     .with_max_visible(5)
//!     .with_position(ToastPosition::TopRight);
//!
//! manager.success("Session saved");
//! manager.warning("API rate limit warning");
//! manager.error("Connection failed");
//!
//! // In render loop:
//! manager.tick();
//! let widget = ToastWidget::new(&manager).terminal_size(width, height);
//! frame.render_widget(widget, area);
//! ```

use cortex_core::style::{ERROR, INFO, SUCCESS, SURFACE_0, SURFACE_1, TEXT, WARNING};
use ratatui::prelude::*;
use ratatui::widgets::Widget;
use std::time::{Duration, Instant};

// ============================================================
// TOAST LEVEL
// ============================================================

/// The severity/type level of a toast notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastLevel {
    /// Success notification - positive outcome
    Success,
    /// Info notification - informational message
    Info,
    /// Warning notification - potential issue
    Warning,
    /// Error notification - failure or problem
    Error,
}

impl ToastLevel {
    /// Returns the ASCII icon for this toast level.
    ///
    /// Icons are ASCII-only for terminal compatibility:
    /// - Success: `[+]`
    /// - Info: `[i]`
    /// - Warning: `[!]`
    /// - Error: `[x]`
    pub fn icon(&self) -> &'static str {
        match self {
            ToastLevel::Success => "[+]",
            ToastLevel::Info => "[i]",
            ToastLevel::Warning => "[!]",
            ToastLevel::Error => "[x]",
        }
    }

    /// Returns the color associated with this toast level.
    pub fn color(&self) -> Color {
        match self {
            ToastLevel::Success => SUCCESS,
            ToastLevel::Info => INFO,
            ToastLevel::Warning => WARNING,
            ToastLevel::Error => ERROR,
        }
    }

    /// Returns the default display duration in milliseconds for this level.
    ///
    /// More severe levels stay visible longer:
    /// - Success: 3000ms
    /// - Info: 4000ms
    /// - Warning: 5000ms
    /// - Error: 7000ms
    pub fn default_duration_ms(&self) -> u64 {
        match self {
            ToastLevel::Success => 3000,
            ToastLevel::Info => 4000,
            ToastLevel::Warning => 5000,
            ToastLevel::Error => 7000,
        }
    }
}

// ============================================================
// TOAST
// ============================================================

/// A single toast notification message.
///
/// Toasts have a level, message, creation time, and duration.
/// They can be persistent (never auto-dismiss) or fade out after their duration.
#[derive(Debug, Clone)]
pub struct Toast {
    /// Unique identifier for this toast
    pub id: u64,
    /// Severity level of the toast
    pub level: ToastLevel,
    /// Message text to display
    pub message: String,
    /// When the toast was created
    pub created_at: Instant,
    /// How long the toast should be visible
    pub duration: Duration,
    /// If true, toast never auto-dismisses
    pub persistent: bool,
    /// Fade animation progress (0.0 = visible, 1.0 = fully faded)
    pub fade_progress: f32,
}

impl Toast {
    /// Creates a new toast with the given level and message.
    ///
    /// The duration is set to the default for the level.
    pub fn new(level: ToastLevel, message: impl Into<String>) -> Self {
        Self {
            id: 0, // Will be set by ToastManager
            level,
            message: message.into(),
            created_at: Instant::now(),
            duration: Duration::from_millis(level.default_duration_ms()),
            persistent: false,
            fade_progress: 0.0,
        }
    }

    /// Creates a success toast.
    pub fn success(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Success, message)
    }

    /// Creates an info toast.
    pub fn info(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Info, message)
    }

    /// Creates a warning toast.
    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Warning, message)
    }

    /// Creates an error toast.
    pub fn error(message: impl Into<String>) -> Self {
        Self::new(ToastLevel::Error, message)
    }

    /// Sets a custom duration for the toast.
    pub fn with_duration(mut self, duration: Duration) -> Self {
        self.duration = duration;
        self
    }

    /// Makes the toast persistent (never auto-dismisses).
    pub fn persistent(mut self) -> Self {
        self.persistent = true;
        self
    }

    /// Returns true if the toast has exceeded its display duration.
    pub fn is_expired(&self) -> bool {
        if self.persistent {
            return false;
        }
        self.created_at.elapsed() >= self.duration
    }

    /// Returns the remaining time before the toast expires.
    pub fn remaining(&self) -> Duration {
        if self.persistent {
            return Duration::MAX;
        }
        self.duration.saturating_sub(self.created_at.elapsed())
    }

    /// Updates the fade progress based on remaining time.
    ///
    /// The fade animation occurs during the last 500ms of the toast's lifetime.
    pub fn tick(&mut self) {
        if self.persistent {
            self.fade_progress = 0.0;
            return;
        }

        let remaining = self.remaining();
        let fade_duration = Duration::from_millis(500);

        if remaining <= fade_duration {
            // Calculate fade progress (0.0 at 500ms remaining, 1.0 at 0ms)
            let remaining_ms = remaining.as_millis() as f32;
            let fade_ms = fade_duration.as_millis() as f32;
            self.fade_progress = 1.0 - (remaining_ms / fade_ms);
        } else {
            self.fade_progress = 0.0;
        }
    }
}

// ============================================================
// TOAST POSITION
// ============================================================

/// Position where toasts are displayed on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastPosition {
    /// Top-right corner (default)
    #[default]
    TopRight,
    /// Top-left corner
    TopLeft,
    /// Bottom-right corner
    BottomRight,
    /// Bottom-left corner
    BottomLeft,
}

// ============================================================
// TOAST MANAGER
// ============================================================

/// Manages a collection of toast notifications.
///
/// The manager handles:
/// - Adding and removing toasts
/// - Updating fade animations
/// - Limiting visible toasts
/// - Positioning toasts on screen
pub struct ToastManager {
    /// Active toasts (newest first)
    toasts: Vec<Toast>,
    /// Counter for generating unique toast IDs
    next_id: u64,
    /// Maximum number of toasts to display at once
    max_visible: usize,
    /// Screen position for toast display
    position: ToastPosition,
}

impl ToastManager {
    /// Creates a new ToastManager with default settings.
    ///
    /// Defaults:
    /// - max_visible: 5
    /// - position: TopRight
    pub fn new() -> Self {
        Self {
            toasts: Vec::new(),
            next_id: 1,
            max_visible: 5,
            position: ToastPosition::TopRight,
        }
    }

    /// Sets the maximum number of visible toasts.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    /// Sets the screen position for toast display.
    pub fn with_position(mut self, pos: ToastPosition) -> Self {
        self.position = pos;
        self
    }

    /// Returns the current position setting.
    pub fn position(&self) -> ToastPosition {
        self.position
    }

    /// Adds a toast to the manager and returns its ID.
    pub fn push(&mut self, mut toast: Toast) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        toast.id = id;
        // Insert at the beginning (newest first)
        self.toasts.insert(0, toast);
        id
    }

    /// Adds a success toast and returns its ID.
    pub fn success(&mut self, message: impl Into<String>) -> u64 {
        self.push(Toast::success(message))
    }

    /// Adds an info toast and returns its ID.
    pub fn info(&mut self, message: impl Into<String>) -> u64 {
        self.push(Toast::info(message))
    }

    /// Adds a warning toast and returns its ID.
    pub fn warning(&mut self, message: impl Into<String>) -> u64 {
        self.push(Toast::warning(message))
    }

    /// Adds an error toast and returns its ID.
    pub fn error(&mut self, message: impl Into<String>) -> u64 {
        self.push(Toast::error(message))
    }

    /// Removes a specific toast by ID.
    pub fn dismiss(&mut self, id: u64) {
        self.toasts.retain(|t| t.id != id);
    }

    /// Removes all toasts.
    pub fn clear(&mut self) {
        self.toasts.clear();
    }

    /// Updates all toasts: removes expired ones and updates fade progress.
    ///
    /// Should be called on each frame/tick of the application.
    pub fn tick(&mut self) {
        // Update fade progress for all toasts
        for toast in &mut self.toasts {
            toast.tick();
        }

        // Remove fully expired toasts
        self.toasts.retain(|t| !t.is_expired());
    }

    /// Returns visible toasts (newest first, limited by max_visible).
    pub fn visible(&self) -> Vec<&Toast> {
        self.toasts.iter().take(self.max_visible).collect()
    }

    /// Returns true if there are no active toasts.
    pub fn is_empty(&self) -> bool {
        self.toasts.is_empty()
    }

    /// Returns true if there are any visible toasts.
    /// Used for render optimization - we only need to redraw when toasts are visible.
    pub fn has_visible(&self) -> bool {
        !self.toasts.is_empty()
    }

    /// Returns the total number of active toasts.
    pub fn len(&self) -> usize {
        self.toasts.len()
    }
}

impl Default for ToastManager {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// TOAST WIDGET
// ============================================================

/// Widget for rendering toast notifications.
///
/// Renders all visible toasts from the manager at the configured position.
pub struct ToastWidget<'a> {
    /// Reference to the toast manager
    manager: &'a ToastManager,
    /// Terminal width for positioning
    terminal_width: u16,
    /// Terminal height for positioning
    terminal_height: u16,
}

impl<'a> ToastWidget<'a> {
    /// Creates a new ToastWidget for the given manager.
    pub fn new(manager: &'a ToastManager) -> Self {
        Self {
            manager,
            terminal_width: 80,
            terminal_height: 24,
        }
    }

    /// Sets the terminal size for proper positioning.
    pub fn terminal_size(mut self, width: u16, height: u16) -> Self {
        self.terminal_width = width;
        self.terminal_height = height;
        self
    }

    /// Renders a single toast at the specified position.
    fn render_toast(&self, toast: &Toast, x: u16, y: u16, width: u16, buf: &mut Buffer) {
        let color = toast.level.color();
        let icon = toast.level.icon();

        // Apply fade effect to colors
        let faded_color = if toast.fade_progress > 0.0 {
            interpolate_color(color, SURFACE_0, toast.fade_progress)
        } else {
            color
        };

        let faded_text = if toast.fade_progress > 0.0 {
            interpolate_color(TEXT, SURFACE_0, toast.fade_progress)
        } else {
            TEXT
        };

        // Background style
        let bg_style = Style::default().bg(SURFACE_1);

        // Fill background
        for dx in 0..width {
            let cell_x = x + dx;
            if cell_x < buf.area.width
                && let Some(cell) = buf.cell_mut(Position::new(cell_x, y))
            {
                cell.set_style(bg_style);
                cell.set_char(' ');
            }
        }

        // Left border accent
        if x < buf.area.width {
            buf.set_string(x, y, "|", Style::default().fg(faded_color).bg(SURFACE_1));
        }

        // Icon
        let icon_x = x + 2;
        if icon_x + 3 < buf.area.width {
            buf.set_string(
                icon_x,
                y,
                icon,
                Style::default().fg(faded_color).bg(SURFACE_1),
            );
        }

        // Message (truncated if needed)
        let msg_x = x + 6;
        let msg_width = width.saturating_sub(8) as usize;
        if msg_width > 0 && msg_x < buf.area.width {
            let message = if toast.message.len() > msg_width {
                if msg_width > 3 {
                    format!("{}...", &toast.message[..msg_width - 3])
                } else {
                    toast.message.chars().take(msg_width).collect()
                }
            } else {
                toast.message.clone()
            };
            buf.set_string(
                msg_x,
                y,
                &message,
                Style::default().fg(faded_text).bg(SURFACE_1),
            );
        }

        // Right border
        let right_x = x + width.saturating_sub(1);
        if right_x < buf.area.width {
            buf.set_string(
                right_x,
                y,
                "|",
                Style::default().fg(faded_color).bg(SURFACE_1),
            );
        }
    }
}

impl Widget for ToastWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let toasts = self.manager.visible();
        if toasts.is_empty() {
            return;
        }

        // Calculate toast width (max 50 chars or 40% of terminal, min 20)
        let toast_width = 50
            .min((area.width as f32 * 0.4) as u16)
            .max(20)
            .min(area.width.saturating_sub(2));

        // Calculate X position based on alignment
        let x = match self.manager.position() {
            ToastPosition::TopRight | ToastPosition::BottomRight => {
                area.width.saturating_sub(toast_width + 1)
            }
            ToastPosition::TopLeft | ToastPosition::BottomLeft => 1,
        };

        // Render each toast
        for (i, toast) in toasts.iter().enumerate() {
            let y = match self.manager.position() {
                ToastPosition::TopRight | ToastPosition::TopLeft => {
                    // Start from y=1 and stack downward
                    1 + i as u16
                }
                ToastPosition::BottomRight | ToastPosition::BottomLeft => {
                    // Start from bottom and stack upward
                    area.height.saturating_sub(2 + i as u16)
                }
            };

            // Skip if out of bounds
            if y >= area.height || y == 0 {
                continue;
            }

            self.render_toast(toast, x, y, toast_width, buf);
        }
    }
}

// ============================================================
// COLOR INTERPOLATION
// ============================================================

/// Interpolates between two colors based on a progress value (0.0 to 1.0).
///
/// At progress 0.0, returns `from`. At progress 1.0, returns `to`.
fn interpolate_color(from: Color, to: Color, progress: f32) -> Color {
    let progress = progress.clamp(0.0, 1.0);

    // Extract RGB components
    let (from_r, from_g, from_b) = extract_rgb(from);
    let (to_r, to_g, to_b) = extract_rgb(to);

    // Linear interpolation
    let r = lerp(from_r as f32, to_r as f32, progress) as u8;
    let g = lerp(from_g as f32, to_g as f32, progress) as u8;
    let b = lerp(from_b as f32, to_b as f32, progress) as u8;

    Color::Rgb(r, g, b)
}

/// Extracts RGB components from a Color.
fn extract_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        // Fallback for non-RGB colors
        _ => (128, 128, 128),
    }
}

/// Linear interpolation between two values.
#[inline]
fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_toast_level_icons() {
        assert_eq!(ToastLevel::Success.icon(), "[+]");
        assert_eq!(ToastLevel::Info.icon(), "[i]");
        assert_eq!(ToastLevel::Warning.icon(), "[!]");
        assert_eq!(ToastLevel::Error.icon(), "[x]");
    }

    #[test]
    fn test_toast_level_colors() {
        assert_eq!(ToastLevel::Success.color(), SUCCESS);
        assert_eq!(ToastLevel::Info.color(), INFO);
        assert_eq!(ToastLevel::Warning.color(), WARNING);
        assert_eq!(ToastLevel::Error.color(), ERROR);
    }

    #[test]
    fn test_toast_level_durations() {
        assert_eq!(ToastLevel::Success.default_duration_ms(), 3000);
        assert_eq!(ToastLevel::Info.default_duration_ms(), 4000);
        assert_eq!(ToastLevel::Warning.default_duration_ms(), 5000);
        assert_eq!(ToastLevel::Error.default_duration_ms(), 7000);
    }

    #[test]
    fn test_toast_creation() {
        let toast = Toast::new(ToastLevel::Info, "Test message");
        assert_eq!(toast.level, ToastLevel::Info);
        assert_eq!(toast.message, "Test message");
        assert!(!toast.persistent);
        assert_eq!(toast.fade_progress, 0.0);
    }

    #[test]
    fn test_toast_convenience_constructors() {
        let success = Toast::success("Success!");
        assert_eq!(success.level, ToastLevel::Success);

        let info = Toast::info("Info!");
        assert_eq!(info.level, ToastLevel::Info);

        let warning = Toast::warning("Warning!");
        assert_eq!(warning.level, ToastLevel::Warning);

        let error = Toast::error("Error!");
        assert_eq!(error.level, ToastLevel::Error);
    }

    #[test]
    fn test_toast_builder() {
        let toast = Toast::info("Test")
            .with_duration(Duration::from_secs(10))
            .persistent();

        assert_eq!(toast.duration, Duration::from_secs(10));
        assert!(toast.persistent);
    }

    #[test]
    fn test_toast_persistent_never_expires() {
        let toast = Toast::info("Persistent").persistent();
        assert!(!toast.is_expired());
        assert_eq!(toast.remaining(), Duration::MAX);
    }

    #[test]
    fn test_toast_manager_creation() {
        let manager = ToastManager::new()
            .with_max_visible(10)
            .with_position(ToastPosition::BottomLeft);

        assert_eq!(manager.max_visible, 10);
        assert_eq!(manager.position(), ToastPosition::BottomLeft);
        assert!(manager.is_empty());
    }

    #[test]
    fn test_toast_manager_push() {
        let mut manager = ToastManager::new();

        let id1 = manager.success("First");
        let id2 = manager.info("Second");
        let id3 = manager.warning("Third");

        assert_eq!(manager.len(), 3);
        assert!(!manager.is_empty());

        // IDs should be unique and incrementing
        assert_ne!(id1, id2);
        assert_ne!(id2, id3);
        assert!(id2 > id1);
        assert!(id3 > id2);
    }

    #[test]
    fn test_toast_manager_dismiss() {
        let mut manager = ToastManager::new();

        let id1 = manager.success("First");
        let id2 = manager.info("Second");
        let id3 = manager.warning("Third");

        manager.dismiss(id2);

        assert_eq!(manager.len(), 2);

        // Check remaining toasts
        let visible = manager.visible();
        let ids: Vec<u64> = visible.iter().map(|t| t.id).collect();
        assert!(!ids.contains(&id2));
        assert!(ids.contains(&id1));
        assert!(ids.contains(&id3));
    }

    #[test]
    fn test_toast_manager_clear() {
        let mut manager = ToastManager::new();

        manager.success("First");
        manager.info("Second");
        manager.warning("Third");

        assert_eq!(manager.len(), 3);

        manager.clear();

        assert!(manager.is_empty());
        assert_eq!(manager.len(), 0);
    }

    #[test]
    fn test_toast_manager_visible_limit() {
        let mut manager = ToastManager::new().with_max_visible(2);

        manager.success("First");
        manager.info("Second");
        manager.warning("Third");

        let visible = manager.visible();
        assert_eq!(visible.len(), 2);

        // Should be newest first
        assert_eq!(visible[0].message, "Third");
        assert_eq!(visible[1].message, "Second");
    }

    #[test]
    fn test_toast_manager_newest_first() {
        let mut manager = ToastManager::new();

        manager.success("First");
        manager.info("Second");
        manager.warning("Third");

        let visible = manager.visible();
        assert_eq!(visible[0].message, "Third");
        assert_eq!(visible[1].message, "Second");
        assert_eq!(visible[2].message, "First");
    }

    #[test]
    fn test_color_interpolation() {
        let from = Color::Rgb(0, 0, 0);
        let to = Color::Rgb(100, 100, 100);

        let result = interpolate_color(from, to, 0.5);
        assert_eq!(result, Color::Rgb(50, 50, 50));

        let result_start = interpolate_color(from, to, 0.0);
        assert_eq!(result_start, Color::Rgb(0, 0, 0));

        let result_end = interpolate_color(from, to, 1.0);
        assert_eq!(result_end, Color::Rgb(100, 100, 100));
    }

    #[test]
    fn test_color_interpolation_clamping() {
        let from = Color::Rgb(0, 0, 0);
        let to = Color::Rgb(100, 100, 100);

        // Values outside 0-1 should be clamped
        let result_neg = interpolate_color(from, to, -0.5);
        assert_eq!(result_neg, Color::Rgb(0, 0, 0));

        let result_over = interpolate_color(from, to, 1.5);
        assert_eq!(result_over, Color::Rgb(100, 100, 100));
    }

    #[test]
    fn test_lerp() {
        assert_eq!(lerp(0.0, 100.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 100.0, 0.5), 50.0);
        assert_eq!(lerp(0.0, 100.0, 1.0), 100.0);
    }

    #[test]
    fn test_toast_position_default() {
        let pos = ToastPosition::default();
        assert_eq!(pos, ToastPosition::TopRight);
    }

    #[test]
    fn test_toast_widget_creation() {
        let manager = ToastManager::new();
        let widget = ToastWidget::new(&manager).terminal_size(120, 40);

        assert_eq!(widget.terminal_width, 120);
        assert_eq!(widget.terminal_height, 40);
    }
}
