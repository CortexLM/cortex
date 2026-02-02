//! Card container component.
//!
//! Provides a bordered container with optional title and key hints.

use crate::borders::{BorderStyle, RoundedBorder};
use crate::key_hints::KeyHintsBar;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

/// A bordered card container.
///
/// Cards provide a consistent visual container for content with:
/// - Rounded borders
/// - Optional title
/// - Optional key hints footer
/// - Consistent spacing
pub struct Card<'a> {
    title: Option<&'a str>,
    border_style: BorderStyle,
    focused: bool,
    key_hints: Vec<(&'static str, &'static str)>,
}

impl<'a> Card<'a> {
    /// Create a new card.
    pub fn new() -> Self {
        Self {
            title: None,
            border_style: BorderStyle::Rounded,
            focused: false,
            key_hints: Vec::new(),
        }
    }

    /// Set the card title.
    pub fn title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    /// Set whether the card has focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Set the border style.
    pub fn border(mut self, style: BorderStyle) -> Self {
        self.border_style = style;
        self
    }

    /// Add key hints to display at the bottom.
    pub fn key_hints(mut self, hints: Vec<(&'static str, &'static str)>) -> Self {
        self.key_hints = hints;
        self
    }

    /// Calculate the inner content area.
    pub fn inner(&self, area: Rect) -> Rect {
        let mut inner = RoundedBorder::new().style(self.border_style).inner(area);

        // Reserve space for key hints if present
        if !self.key_hints.is_empty() && inner.height > 1 {
            inner.height = inner.height.saturating_sub(1);
        }

        inner
    }
}

impl Default for Card<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for Card<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 5 {
            return;
        }

        // Render border
        let border = RoundedBorder::new()
            .focused(self.focused)
            .style(self.border_style);

        if let Some(title) = self.title {
            border.title(title).render(area, buf);
        } else {
            border.render(area, buf);
        }

        // Render key hints at bottom if present
        if !self.key_hints.is_empty() {
            let inner = RoundedBorder::new().style(self.border_style).inner(area);
            let hints_area = Rect::new(
                inner.x,
                inner.y + inner.height.saturating_sub(1),
                inner.width,
                1,
            );

            if hints_area.height > 0 {
                KeyHintsBar::from_tuples(&self.key_hints).render(hints_area, buf);
            }
        }
    }
}

/// Builder for creating cards with content.
pub struct CardBuilder<'a> {
    card: Card<'a>,
}

impl<'a> CardBuilder<'a> {
    /// Create a new card builder.
    pub fn new() -> Self {
        Self { card: Card::new() }
    }

    /// Set the card title.
    pub fn title(mut self, title: &'a str) -> Self {
        self.card = self.card.title(title);
        self
    }

    /// Set whether the card has focus.
    pub fn focused(mut self, focused: bool) -> Self {
        self.card = self.card.focused(focused);
        self
    }

    /// Set the border style.
    pub fn border(mut self, style: BorderStyle) -> Self {
        self.card = self.card.border(style);
        self
    }

    /// Add key hints.
    pub fn key_hints(mut self, hints: Vec<(&'static str, &'static str)>) -> Self {
        self.card = self.card.key_hints(hints);
        self
    }

    /// Build the card.
    pub fn build(self) -> Card<'a> {
        self.card
    }
}

impl Default for CardBuilder<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_card_builder() {
        let card = Card::new().title("Test").focused(true);
        assert_eq!(card.title, Some("Test"));
        assert!(card.focused);
    }

    #[test]
    fn test_card_inner_area() {
        let card = Card::new();
        let area = Rect::new(0, 0, 20, 10);
        let inner = card.inner(area);

        // Should account for borders
        assert!(inner.width < area.width);
        assert!(inner.height < area.height);
    }

    #[test]
    fn test_card_with_hints() {
        let card = Card::new().key_hints(vec![("Enter", "Select")]);
        let area = Rect::new(0, 0, 20, 10);
        let inner = card.inner(area);

        // Should reserve space for hints
        assert!(inner.height < area.height - 2);
    }
}
