//! Minimalist status indicator widget
//!
//! Shows a spinner, animated header text with shimmer effect, elapsed time,
//! and optional details. Used during long-running operations to provide
//! visual feedback to the user.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::ui::colors::AdaptiveColors;
use crate::ui::consts::{SPINNER_INTERVAL_MS, STREAMING_SPINNER_FRAMES};
use crate::ui::shimmer::{elapsed_since_start, shimmer_spans};

/// Prefix for the details line (tree branch character).
const DETAILS_PREFIX: &str = "  \u{2514} "; // "  └ "

/// Status indicator widget showing spinner, header, elapsed time, and optional details.
///
/// # Example
///
/// ```ignore
/// let indicator = StatusIndicator::new("Working")
///     .with_details("Reading src/main.rs")
///     .with_interrupt_hint(true);
/// ```
pub struct StatusIndicator {
    /// Animated header text (e.g., "Working", "Analyzing code").
    header: String,
    /// Optional details line shown below the header.
    details: Option<String>,
    /// Whether to show the interrupt hint (e.g., "Esc to interrupt").
    show_interrupt_hint: bool,
    /// Pre-computed elapsed seconds (when using external timing).
    elapsed_secs: u64,
}

impl StatusIndicator {
    /// Create a new status indicator with the given header text.
    ///
    /// # Arguments
    ///
    /// * `header` - The main status text (e.g., "Working", "Analyzing code")
    pub fn new(header: impl Into<String>) -> Self {
        Self {
            header: header.into(),
            details: None,
            show_interrupt_hint: false,
            elapsed_secs: 0,
        }
    }

    /// Set the elapsed time in seconds (from external source like StreamingState).
    ///
    /// # Arguments
    ///
    /// * `secs` - The elapsed seconds to display
    pub fn with_elapsed_secs(mut self, secs: u64) -> Self {
        self.elapsed_secs = secs;
        self
    }

    /// Add details text to show below the header.
    ///
    /// # Arguments
    ///
    /// * `details` - The details text (e.g., "Reading src/main.rs")
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Set whether to show the interrupt hint.
    ///
    /// # Arguments
    ///
    /// * `show` - Whether to show the hint
    pub fn with_interrupt_hint(mut self, show: bool) -> Self {
        self.show_interrupt_hint = show;
        self
    }

    /// Update the header text.
    ///
    /// # Arguments
    ///
    /// * `header` - The new header text
    pub fn update_header(&mut self, header: impl Into<String>) {
        self.header = header.into();
    }

    /// Update the details text.
    ///
    /// # Arguments
    ///
    /// * `details` - The new details text, or None to clear
    pub fn update_details(&mut self, details: Option<String>) {
        self.details = details.filter(|d| !d.is_empty());
    }

    /// Get the current spinner frame based on elapsed time.
    /// Uses a "breathing" animation with frames ordered by visual weight.
    /// The ping-pong pattern is baked into STREAMING_SPINNER_FRAMES.
    fn spinner_frame(&self) -> char {
        let elapsed_ms = elapsed_since_start().as_millis() as u64;
        let frame_index =
            (elapsed_ms / SPINNER_INTERVAL_MS) as usize % STREAMING_SPINNER_FRAMES.len();
        STREAMING_SPINNER_FRAMES[frame_index]
    }
}

/// Format elapsed seconds into a compact human-friendly string.
///
/// # Examples
///
/// ```ignore
/// assert_eq!(fmt_elapsed_compact(0), "0s");
/// assert_eq!(fmt_elapsed_compact(59), "59s");
/// assert_eq!(fmt_elapsed_compact(90), "1m 30s");
/// assert_eq!(fmt_elapsed_compact(3690), "1h 01m 30s");
/// ```
pub fn fmt_elapsed_compact(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    if secs < 3600 {
        let minutes = secs / 60;
        let seconds = secs % 60;
        return format!("{minutes}m {seconds:02}s");
    }
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    format!("{hours}h {minutes:02}m {seconds:02}s")
}

impl Widget for StatusIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }

        let colors = AdaptiveColors::default();
        let elapsed_str = fmt_elapsed_compact(self.elapsed_secs);

        // Build the header line: "⠹ Working · Analyzing code (45s • Esc to interrupt)"
        let mut spans: Vec<Span<'_>> = Vec::with_capacity(8);

        // Spinner
        spans.push(Span::styled(
            self.spinner_frame().to_string(),
            ratatui::style::Style::default().fg(colors.accent),
        ));
        spans.push(Span::raw(" "));

        // Header with shimmer effect
        spans.extend(shimmer_spans(&self.header));

        // Elapsed time and optional interrupt hint
        spans.push(Span::raw(" "));
        if self.show_interrupt_hint {
            spans.push(Span::raw(format!("({elapsed_str} \u{2022} ")).dim());
            spans.push(Span::styled(
                "Esc",
                ratatui::style::Style::default().fg(colors.text),
            ));
            spans.push(Span::raw(" to interrupt)").dim());
        } else {
            spans.push(Span::raw(format!("({elapsed_str})")).dim());
        }

        // Render the header line
        let header_line = Line::from(spans);
        let header_y = area.y;
        buf.set_line(area.x, header_y, &header_line, area.width);

        // Render details line if present and there's room
        if let Some(details) = &self.details
            && area.height > 1
        {
            let details_line = Line::from(vec![
                Span::raw(DETAILS_PREFIX).dim(),
                Span::raw(details.as_str()).dim(),
            ]);
            buf.set_line(area.x, header_y + 1, &details_line, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fmt_elapsed_compact_seconds() {
        assert_eq!(fmt_elapsed_compact(0), "0s");
        assert_eq!(fmt_elapsed_compact(1), "1s");
        assert_eq!(fmt_elapsed_compact(59), "59s");
    }

    #[test]
    fn test_fmt_elapsed_compact_minutes() {
        assert_eq!(fmt_elapsed_compact(60), "1m 00s");
        assert_eq!(fmt_elapsed_compact(61), "1m 01s");
        assert_eq!(fmt_elapsed_compact(90), "1m 30s");
        assert_eq!(fmt_elapsed_compact(3599), "59m 59s");
    }

    #[test]
    fn test_fmt_elapsed_compact_hours() {
        assert_eq!(fmt_elapsed_compact(3600), "1h 00m 00s");
        assert_eq!(fmt_elapsed_compact(3661), "1h 01m 01s");
        assert_eq!(fmt_elapsed_compact(3690), "1h 01m 30s");
        assert_eq!(fmt_elapsed_compact(7322), "2h 02m 02s");
    }

    #[test]
    fn test_new_status_indicator() {
        let indicator = StatusIndicator::new("Working");
        assert_eq!(indicator.header, "Working");
        assert!(indicator.details.is_none());
        assert!(!indicator.show_interrupt_hint);
        assert_eq!(indicator.elapsed_secs, 0);
    }

    #[test]
    fn test_with_details() {
        let indicator = StatusIndicator::new("Working").with_details("Reading file");
        assert_eq!(indicator.details, Some("Reading file".to_string()));
    }

    #[test]
    fn test_with_interrupt_hint() {
        let indicator = StatusIndicator::new("Working").with_interrupt_hint(true);
        assert!(indicator.show_interrupt_hint);
    }

    #[test]
    fn test_with_elapsed_secs() {
        let indicator = StatusIndicator::new("Working").with_elapsed_secs(42);
        assert_eq!(indicator.elapsed_secs, 42);
    }

    #[test]
    fn test_update_header() {
        let mut indicator = StatusIndicator::new("Working");
        indicator.update_header("Analyzing");
        assert_eq!(indicator.header, "Analyzing");
    }

    #[test]
    fn test_update_details() {
        let mut indicator = StatusIndicator::new("Working");
        indicator.update_details(Some("Reading file".to_string()));
        assert_eq!(indicator.details, Some("Reading file".to_string()));

        indicator.update_details(None);
        assert!(indicator.details.is_none());

        indicator.update_details(Some("".to_string()));
        assert!(indicator.details.is_none());
    }

    #[test]
    fn test_spinner_frame_cycles() {
        let indicator = StatusIndicator::new("Working");
        let frame = indicator.spinner_frame();
        assert!(STREAMING_SPINNER_FRAMES.contains(&frame));
    }
}
