//! Toast Notification System
//!
//! Provides temporary notification messages (success, warning, error, info)
//! displayed in configurable screen positions with auto-dismiss and fade animation.
//!
//! ## Usage
//!
//! ```ignore
//! use cortex_tui_components::toast::{Toast, ToastLevel, ToastManager, ToastWidget, ToastPosition};
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

use crate::color_scheme::ColorScheme;
use ratatui::prelude::*;
use ratatui::widgets::Widget;
use std::time::{Duration, Instant};

// ============================================================
// TOAST LEVEL
// ============================================================

/// The severity/type level of a toast notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToastLevel {
    /// Success notification - positive outcome
    Success,
    /// Info notification - informational message
    #[default]
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

    /// Returns the color associated with this toast level using the given color scheme.
    pub fn color(&self, scheme: &ColorScheme) -> Color {
        match self {
            ToastLevel::Success => scheme.success,
            ToastLevel::Info => scheme.info,
            ToastLevel::Warning => scheme.warning,
            ToastLevel::Error => scheme.error,
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
#[derive(Default)]
pub struct ToastManager {
    /// Active toasts (newest first)
    toasts: Vec<Toast>,
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
    pub fn push(&mut self, toast: Toast) -> u64 {
        // Toast notifications disabled
        let _ = toast;
        0
    }

    /// Adds a success toast and returns its ID.
    pub fn success(&mut self, message: impl Into<String>) -> u64 {
        // Toast notifications disabled
        let _ = message;
        0
    }

    /// Adds an info toast and returns its ID.
    pub fn info(&mut self, message: impl Into<String>) -> u64 {
        // Toast notifications disabled
        let _ = message;
        0
    }

    /// Adds a warning toast and returns its ID.
    pub fn warning(&mut self, message: impl Into<String>) -> u64 {
        // Toast notifications disabled
        let _ = message;
        0
    }

    /// Adds an error toast and returns its ID.
    pub fn error(&mut self, message: impl Into<String>) -> u64 {
        // Toast notifications disabled
        let _ = message;
        0
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
        for toast in &mut self.toasts {
            toast.tick();
        }
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
    pub fn has_visible(&self) -> bool {
        !self.toasts.is_empty()
    }

    /// Returns the total number of active toasts.
    pub fn len(&self) -> usize {
        self.toasts.len()
    }
}

// ============================================================
// TOAST WIDGET
// ============================================================

/// Widget for rendering toast notifications.
///
/// Renders all visible toasts from the manager at the configured position.
pub struct ToastWidget<'a> {
    manager: &'a ToastManager,
    terminal_width: u16,
    terminal_height: u16,
    colors: ColorScheme,
}

impl<'a> ToastWidget<'a> {
    /// Creates a new ToastWidget for the given manager.
    pub fn new(manager: &'a ToastManager) -> Self {
        Self {
            manager,
            terminal_width: 80,
            terminal_height: 24,
            colors: ColorScheme::default(),
        }
    }

    /// Sets the terminal size for proper positioning.
    pub fn terminal_size(mut self, width: u16, height: u16) -> Self {
        self.terminal_width = width;
        self.terminal_height = height;
        self
    }

    /// Sets a custom color scheme.
    pub fn with_colors(mut self, colors: ColorScheme) -> Self {
        self.colors = colors;
        self
    }

    fn render_toast(&self, toast: &Toast, x: u16, y: u16, width: u16, buf: &mut Buffer) {
        let color = toast.level.color(&self.colors);
        let icon = toast.level.icon();

        let faded_color = if toast.fade_progress > 0.0 {
            interpolate_color(color, self.colors.surface, toast.fade_progress)
        } else {
            color
        };

        let faded_text = if toast.fade_progress > 0.0 {
            interpolate_color(self.colors.text, self.colors.surface, toast.fade_progress)
        } else {
            self.colors.text
        };

        let bg_style = Style::default().bg(self.colors.surface_alt);

        for dx in 0..width {
            let cell_x = x + dx;
            if cell_x < buf.area.width
                && let Some(cell) = buf.cell_mut(Position::new(cell_x, y))
            {
                cell.set_style(bg_style);
                cell.set_char(' ');
            }
        }

        if x < buf.area.width {
            buf.set_string(
                x,
                y,
                "|",
                Style::default().fg(faded_color).bg(self.colors.surface_alt),
            );
        }

        let icon_x = x + 2;
        if icon_x + 3 < buf.area.width {
            buf.set_string(
                icon_x,
                y,
                icon,
                Style::default().fg(faded_color).bg(self.colors.surface_alt),
            );
        }

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
                Style::default().fg(faded_text).bg(self.colors.surface_alt),
            );
        }

        let right_x = x + width.saturating_sub(1);
        if right_x < buf.area.width {
            buf.set_string(
                right_x,
                y,
                "|",
                Style::default().fg(faded_color).bg(self.colors.surface_alt),
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

        let toast_width = 50
            .min((area.width as f32 * 0.4) as u16)
            .max(20)
            .min(area.width.saturating_sub(2));

        let x = match self.manager.position() {
            ToastPosition::TopRight | ToastPosition::BottomRight => {
                area.width.saturating_sub(toast_width + 1)
            }
            ToastPosition::TopLeft | ToastPosition::BottomLeft => 1,
        };

        for (i, toast) in toasts.iter().enumerate() {
            let y = match self.manager.position() {
                ToastPosition::TopRight | ToastPosition::TopLeft => 1 + i as u16,
                ToastPosition::BottomRight | ToastPosition::BottomLeft => {
                    area.height.saturating_sub(2 + i as u16)
                }
            };

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

fn interpolate_color(from: Color, to: Color, progress: f32) -> Color {
    let progress = progress.clamp(0.0, 1.0);
    let (from_r, from_g, from_b) = extract_rgb(from);
    let (to_r, to_g, to_b) = extract_rgb(to);

    let r = lerp(from_r as f32, to_r as f32, progress) as u8;
    let g = lerp(from_g as f32, to_g as f32, progress) as u8;
    let b = lerp(from_b as f32, to_b as f32, progress) as u8;

    Color::Rgb(r, g, b)
}

fn extract_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (128, 128, 128),
    }
}

#[inline]
fn lerp(start: f32, end: f32, t: f32) -> f32 {
    start + (end - start) * t
}

// ============================================================
// WIDGET IMPL FOR TOAST (standalone rendering)
// ============================================================

impl Widget for &Toast {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 5 || area.height < 1 {
            return;
        }

        let scheme = ColorScheme::default();
        let color = self.level.color(&scheme);
        let icon = self.level.icon();

        // Render: | [icon] message |
        let mut x = area.x;

        // Left border
        if x < area.right() {
            buf.set_string(x, area.y, "|", Style::default().fg(color));
            x += 1;
        }

        // Space
        if x < area.right() {
            buf.set_string(x, area.y, " ", Style::default());
            x += 1;
        }

        // Icon
        let icon_len = icon.len() as u16;
        if x + icon_len <= area.right() {
            buf.set_string(x, area.y, icon, Style::default().fg(color));
            x += icon_len;
        }

        // Space
        if x < area.right() {
            buf.set_string(x, area.y, " ", Style::default());
            x += 1;
        }

        // Message (truncated if needed)
        let remaining_width = area.right().saturating_sub(x + 2) as usize; // -2 for " |" at end
        if remaining_width > 0 {
            let msg = if self.message.len() > remaining_width {
                if remaining_width > 3 {
                    format!("{}...", &self.message[..remaining_width - 3])
                } else {
                    self.message.chars().take(remaining_width).collect()
                }
            } else {
                self.message.clone()
            };
            buf.set_string(x, area.y, &msg, Style::default().fg(scheme.text));
            x += msg.len() as u16;
        }

        // Padding to right border
        while x < area.right().saturating_sub(1) {
            buf.set_string(x, area.y, " ", Style::default());
            x += 1;
        }

        // Right border
        if x < area.right() {
            buf.set_string(x, area.y, "|", Style::default().fg(color));
        }
    }
}

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
    fn test_toast_manager_push() {
        // Toast notifications are disabled - verifying no-op behavior
        let mut manager = ToastManager::new();
        let id1 = manager.success("First");
        let id2 = manager.info("Second");
        assert_eq!(manager.len(), 0);
        assert!(manager.is_empty());
        assert_eq!(id1, 0);
        assert_eq!(id2, 0);
    }

    #[test]
    fn test_toast_manager_visible_limit() {
        // Toast notifications are disabled - verifying no-op behavior
        let mut manager = ToastManager::new().with_max_visible(2);
        manager.success("First");
        manager.info("Second");
        manager.warning("Third");
        let visible = manager.visible();
        assert_eq!(visible.len(), 0);
        assert!(manager.is_empty());
    }

    #[test]
    fn test_toast_persistent() {
        let toast = Toast::info("Persistent").persistent();
        assert!(!toast.is_expired());
    }
}
