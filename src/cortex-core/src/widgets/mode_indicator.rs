//! Mode indicator widget for displaying the current operation mode.
//!
//! This widget displays the current operation mode (Build/Plan/Spec) with
//! appropriate styling and icons.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

/// Operation mode for display purposes.
///
/// This mirrors the `OperationMode` from cortex-agents to avoid coupling
/// the core widget to the agents crate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum DisplayMode {
    /// Build mode - full access
    #[default]
    Build,
    /// Plan mode - read-only
    Plan,
    /// Spec mode - specification before build
    Spec,
}

impl DisplayMode {
    /// Get the display name.
    pub fn name(&self) -> &'static str {
        match self {
            DisplayMode::Build => "BUILD",
            DisplayMode::Plan => "PLAN",
            DisplayMode::Spec => "SPEC",
        }
    }

    /// Get the indicator emoji/icon.
    pub fn indicator(&self) -> &'static str {
        match self {
            DisplayMode::Build => "[B]",
            DisplayMode::Plan => "[P]",
            DisplayMode::Spec => "[S]",
        }
    }

    /// Get the color for this mode.
    pub fn color(&self) -> Color {
        match self {
            DisplayMode::Build => Color::Rgb(0, 255, 163), // Green
            DisplayMode::Plan => Color::Rgb(255, 200, 87), // Yellow/amber
            DisplayMode::Spec => Color::Rgb(139, 92, 246), // Purple
        }
    }

    /// Get a short description.
    pub fn description(&self) -> &'static str {
        match self {
            DisplayMode::Build => "Full access mode",
            DisplayMode::Plan => "Read-only mode",
            DisplayMode::Spec => "Specification mode",
        }
    }

    /// Cycle to next mode.
    pub fn next(&self) -> Self {
        match self {
            DisplayMode::Build => DisplayMode::Plan,
            DisplayMode::Plan => DisplayMode::Spec,
            DisplayMode::Spec => DisplayMode::Build,
        }
    }
}

/// Widget for displaying the current operation mode.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_core::widgets::ModeIndicator;
///
/// let indicator = ModeIndicator::new(DisplayMode::Build);
/// // Render with ratatui...
/// ```
pub struct ModeIndicator {
    /// Current mode to display
    mode: DisplayMode,
    /// Whether to show the icon
    show_icon: bool,
    /// Whether to show the hint text (Tab to switch)
    show_hint: bool,
    /// Custom style override
    style: Option<Style>,
}

impl ModeIndicator {
    /// Create a new mode indicator.
    pub fn new(mode: DisplayMode) -> Self {
        Self {
            mode,
            show_icon: true,
            show_hint: false,
            style: None,
        }
    }

    /// Show or hide the icon.
    pub fn with_icon(mut self, show: bool) -> Self {
        self.show_icon = show;
        self
    }

    /// Show or hide the hint text.
    pub fn with_hint(mut self, show: bool) -> Self {
        self.show_hint = show;
        self
    }

    /// Set a custom style.
    pub fn with_style(mut self, style: Style) -> Self {
        self.style = Some(style);
        self
    }

    /// Get the width needed to render this indicator.
    pub fn width(&self) -> u16 {
        let mut width = self.mode.name().len();
        if self.show_icon {
            width += 3; // icon + space
        }
        if self.show_hint {
            width += 12; // " [Tab:mode]"
        }
        width as u16
    }

    /// Build the display line.
    fn build_line(&self) -> Line<'static> {
        let color = self.mode.color();
        let base_style = self
            .style
            .unwrap_or_else(|| Style::default().fg(color).add_modifier(Modifier::BOLD));

        let mut spans = Vec::new();

        if self.show_icon {
            spans.push(Span::styled(
                format!("{} ", self.mode.indicator()),
                base_style,
            ));
        }

        spans.push(Span::styled(self.mode.name().to_string(), base_style));

        if self.show_hint {
            spans.push(Span::styled(
                " [Tab]",
                Style::default().fg(Color::Rgb(130, 154, 177)), // TEXT_DIM
            ));
        }

        Line::from(spans)
    }
}

impl Widget for ModeIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let line = self.build_line();
        let x = area.x;
        let y = area.y;

        buf.set_line(x, y, &line, area.width);
    }
}

/// Compact mode indicator for tight spaces.
///
/// Shows just the icon and a single letter.
pub struct CompactModeIndicator {
    mode: DisplayMode,
}

impl CompactModeIndicator {
    pub fn new(mode: DisplayMode) -> Self {
        Self { mode }
    }
}

impl Widget for CompactModeIndicator {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let color = self.mode.color();
        let style = Style::default().fg(color).add_modifier(Modifier::BOLD);

        let text = format!("{}", self.mode.indicator());
        let line = Line::from(Span::styled(text, style));

        buf.set_line(area.x, area.y, &line, area.width);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_display_mode_cycle() {
        let mode = DisplayMode::Build;
        assert_eq!(mode.next(), DisplayMode::Plan);
        assert_eq!(mode.next().next(), DisplayMode::Spec);
        assert_eq!(mode.next().next().next(), DisplayMode::Build);
    }

    #[test]
    fn test_display_mode_names() {
        assert_eq!(DisplayMode::Build.name(), "BUILD");
        assert_eq!(DisplayMode::Plan.name(), "PLAN");
        assert_eq!(DisplayMode::Spec.name(), "SPEC");
    }

    #[test]
    fn test_indicator_width() {
        let indicator = ModeIndicator::new(DisplayMode::Build);
        assert!(indicator.width() > 0);

        let with_hint = ModeIndicator::new(DisplayMode::Build).with_hint(true);
        assert!(with_hint.width() > indicator.width());
    }

    #[test]
    fn test_indicator_without_icon() {
        let indicator = ModeIndicator::new(DisplayMode::Build).with_icon(false);
        // Should still have some width for the text
        assert!(indicator.width() >= 5); // "BUILD" is 5 chars
    }
}
