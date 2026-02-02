//! Basic spinner widget with optional label.

use crate::animation::Spinner;
use crate::style::ORANGE;
use ratatui::prelude::*;
use ratatui::widgets::Widget;

/// A basic loading spinner widget with optional label.
///
/// Renders the current frame of a [`Spinner`] animation with optional
/// accompanying text.
///
/// # Example
/// ```
/// use cortex_engine::animation::Spinner;
/// use cortex_engine::widgets::SpinnerWidget;
/// use ratatui::style::{Style, Color};
///
/// let mut spinner = Spinner::dots();
/// spinner.tick();
///
/// let widget = SpinnerWidget::new(&spinner)
///     .label("Loading...")
///     .style(Style::default().fg(Color::Yellow));
/// ```
pub struct SpinnerWidget<'a> {
    spinner: &'a Spinner,
    label: Option<&'a str>,
    style: Style,
}

impl<'a> SpinnerWidget<'a> {
    /// Creates a new spinner widget.
    ///
    /// # Arguments
    /// * `spinner` - Reference to the spinner animation state
    pub fn new(spinner: &'a Spinner) -> Self {
        Self {
            spinner,
            label: None,
            style: Style::default().fg(ORANGE),
        }
    }

    /// Sets the label to display after the spinner.
    ///
    /// # Arguments
    /// * `label` - Text to display after the spinner character
    pub fn label(mut self, label: &'a str) -> Self {
        self.label = Some(label);
        self
    }

    /// Sets the style for the spinner and label.
    ///
    /// # Arguments
    /// * `style` - Style to apply to both spinner and label
    pub fn style(mut self, style: Style) -> Self {
        self.style = style;
        self
    }
}

impl Widget for SpinnerWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let spinner_char = self.spinner.current();

        let text = if let Some(label) = self.label {
            format!("{} {}", spinner_char, label)
        } else {
            spinner_char.to_string()
        };

        // Render the text with style, truncating if necessary
        let max_width = area.width as usize;
        let display_text: String = text.chars().take(max_width).collect();

        buf.set_string(area.x, area.y, &display_text, self.style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    fn create_test_buffer(width: u16, height: u16) -> Buffer {
        Buffer::empty(Rect::new(0, 0, width, height))
    }

    #[test]
    fn test_spinner_widget_new() {
        let spinner = Spinner::dots();
        let widget = SpinnerWidget::new(&spinner);
        assert!(widget.label.is_none());
    }

    #[test]
    fn test_spinner_widget_with_label() {
        let spinner = Spinner::dots();
        let widget = SpinnerWidget::new(&spinner).label("Loading...");
        assert_eq!(widget.label, Some("Loading..."));
    }

    #[test]
    fn test_spinner_widget_render() {
        let spinner = Spinner::dots();
        let widget = SpinnerWidget::new(&spinner).label("Test");

        let mut buf = create_test_buffer(20, 1);
        let area = Rect::new(0, 0, 20, 1);
        widget.render(area, &mut buf);

        // Check that something was rendered (spinner char + space + label)
        let content: String = (0..20)
            .map(|x| buf.get(x, 0).symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Test"));
    }

    #[test]
    fn test_spinner_widget_empty_area() {
        let spinner = Spinner::dots();
        let widget = SpinnerWidget::new(&spinner);

        let mut buf = create_test_buffer(0, 0);
        let area = Rect::new(0, 0, 0, 0);
        // Should not panic
        widget.render(area, &mut buf);
    }
}
