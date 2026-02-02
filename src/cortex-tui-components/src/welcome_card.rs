//! Welcome Card components for the Cortex TUI welcome screen.
//!
//! Provides reusable card widgets for displaying welcome messages and user info.

use crate::borders::ROUNDED_BORDER;
use crate::mascot::MASCOT_MINIMAL_LINES;
use cortex_core::style::{CYAN_PRIMARY, TEXT, TEXT_DIM};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Widget};
use unicode_width::UnicodeWidthStr;

/// Trait for components that can generate scrollable lines.
pub trait ToLines {
    /// Generate styled lines for scrollable rendering.
    fn to_lines(&self, width: u16) -> Vec<Line<'static>>;
}

/// A welcome card with mascot, greeting, and tips.
///
/// # Example
/// ```rust,ignore
/// use cortex_tui_components::welcome_card::WelcomeCard;
///
/// let card = WelcomeCard::new()
///     .user_name("Mathis")
///     .subtitle("Your AI-powered coding assistant.")
///     .version("1.0.0")
///     .tips(&["Send /help for commands", "Use Tab for autocomplete"]);
/// ```
pub struct WelcomeCard<'a> {
    user_name: Option<&'a str>,
    subtitle: Option<&'a str>,
    version: Option<&'a str>,
    tips: Vec<&'a str>,
    accent_color: Color,
    text_color: Color,
    dim_color: Color,
    border_color: Color,
}

impl<'a> WelcomeCard<'a> {
    /// Create a new welcome card.
    pub fn new() -> Self {
        Self {
            user_name: None,
            subtitle: None,
            version: None,
            tips: Vec::new(),
            accent_color: CYAN_PRIMARY,
            text_color: TEXT,
            dim_color: TEXT_DIM,
            border_color: CYAN_PRIMARY,
        }
    }

    /// Set the user name for the greeting.
    pub fn user_name(mut self, name: &'a str) -> Self {
        self.user_name = Some(name);
        self
    }

    /// Set the subtitle text.
    pub fn subtitle(mut self, text: &'a str) -> Self {
        self.subtitle = Some(text);
        self
    }

    /// Set the version string for the title.
    pub fn version(mut self, version: &'a str) -> Self {
        self.version = Some(version);
        self
    }

    /// Set the tips to display.
    pub fn tips(mut self, tips: &[&'a str]) -> Self {
        self.tips = tips.to_vec();
        self
    }

    /// Set the accent color.
    pub fn accent_color(mut self, color: Color) -> Self {
        self.accent_color = color;
        self
    }

    /// Set the text color.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set the dim/muted text color.
    pub fn dim_color(mut self, color: Color) -> Self {
        self.dim_color = color;
        self
    }

    /// Set the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Calculate the required height for this card.
    pub fn required_height(&self) -> u16 {
        // Border top (1) + empty (1) + mascot (4) + empty (1) + tips + empty (1) + border bottom (1)
        let base_height = 9_u16;
        let tips_height = self.tips.len() as u16;
        base_height + tips_height
    }
}

impl Default for WelcomeCard<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl ToLines for WelcomeCard<'_> {
    fn to_lines(&self, width: u16) -> Vec<Line<'static>> {
        let w = (width as usize).max(40); // Adaptive width, minimum 40
        let bs = Style::default().fg(self.border_color);
        let inner = w - 2; // width between │ and │

        let title = if let Some(v) = self.version {
            format!(" Cortex CLI v{} ", v)
        } else {
            " Cortex CLI ".to_string()
        };

        let greeting = if let Some(name) = self.user_name {
            format!("Welcome back {}!", name)
        } else {
            "Welcome!".to_string()
        };
        let subtitle = self.subtitle.unwrap_or("Your AI-powered coding assistant.");

        let mut lines: Vec<Line<'static>> = Vec::new();

        // Top border
        let top = format!(
            "╭─{}{}╮",
            title,
            "─".repeat(w.saturating_sub(title.width() + 3))
        );
        lines.push(Line::from(Span::styled(top, bs)));

        // Empty line
        lines.push(Line::from(vec![
            Span::styled("│", bs),
            Span::raw(" ".repeat(inner)),
            Span::styled("│", bs),
        ]));

        // Mascot lines (4 lines)
        for (i, m) in MASCOT_MINIMAL_LINES.iter().enumerate() {
            let m_width = m.width();

            let spans = match i {
                0 => {
                    let used = 1 + m_width + 2 + greeting.width();
                    let pad = inner.saturating_sub(used);
                    vec![
                        Span::styled("│", bs),
                        Span::raw(" "),
                        Span::styled(m.to_string(), Style::default().fg(self.accent_color)),
                        Span::raw("  "),
                        Span::styled(
                            greeting.clone(),
                            Style::default()
                                .fg(self.text_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" ".repeat(pad)),
                        Span::styled("│", bs),
                    ]
                }
                1 => {
                    let used = 1 + m_width + 2 + subtitle.width();
                    let pad = inner.saturating_sub(used);
                    vec![
                        Span::styled("│", bs),
                        Span::raw(" "),
                        Span::styled(m.to_string(), Style::default().fg(self.accent_color)),
                        Span::raw("  "),
                        Span::styled(subtitle.to_string(), Style::default().fg(self.dim_color)),
                        Span::raw(" ".repeat(pad)),
                        Span::styled("│", bs),
                    ]
                }
                _ => {
                    let used = 1 + m_width;
                    let pad = inner.saturating_sub(used);
                    vec![
                        Span::styled("│", bs),
                        Span::raw(" "),
                        Span::styled(m.to_string(), Style::default().fg(self.accent_color)),
                        Span::raw(" ".repeat(pad)),
                        Span::styled("│", bs),
                    ]
                }
            };
            lines.push(Line::from(spans));
        }

        // Empty line
        lines.push(Line::from(vec![
            Span::styled("│", bs),
            Span::raw(" ".repeat(inner)),
            Span::styled("│", bs),
        ]));

        // Tips
        for tip in &self.tips {
            let tip_width = tip.width();
            let pad = inner.saturating_sub(1 + tip_width);
            lines.push(Line::from(vec![
                Span::styled("│", bs),
                Span::raw(" "),
                Span::styled(tip.to_string(), Style::default().fg(self.dim_color)),
                Span::raw(" ".repeat(pad)),
                Span::styled("│", bs),
            ]));
        }

        // Bottom border
        lines.push(Line::from(Span::styled(
            format!("╰{}╯", "─".repeat(w - 2)),
            bs,
        )));

        lines
    }
}

impl Widget for WelcomeCard<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 8 || area.width < 40 {
            return;
        }

        // Render border with title using custom border color
        let title = if let Some(v) = self.version {
            format!(" Cortex CLI v{} ", v)
        } else {
            " Cortex CLI ".to_string()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(self.border_color))
            .title(title)
            .title_style(Style::default().fg(self.border_color));

        let inner = block.inner(area);
        block.render(area, buf);
        let mut y = inner.y;

        // Empty line
        y += 1;

        // Render mascot with greeting on the side
        let greeting = if let Some(name) = self.user_name {
            format!("Welcome back {}!", name)
        } else {
            "Welcome!".to_string()
        };
        let subtitle = self.subtitle.unwrap_or("Your AI-powered coding assistant.");

        for (i, mascot_line) in MASCOT_MINIMAL_LINES.iter().enumerate() {
            if y >= inner.y + inner.height {
                break;
            }

            // Render mascot part
            buf.set_string(
                inner.x,
                y,
                *mascot_line,
                Style::default().fg(self.accent_color),
            );

            // Render text next to mascot (lines 1 and 2 only)
            let text_x = inner.x + 14; // After mascot width
            match i {
                0 => {
                    // Greeting line
                    buf.set_string(
                        text_x,
                        y,
                        &greeting,
                        Style::default()
                            .fg(self.text_color)
                            .add_modifier(Modifier::BOLD),
                    );
                }
                1 => {
                    // Subtitle line
                    buf.set_string(text_x, y, subtitle, Style::default().fg(self.dim_color));
                }
                _ => {}
            }

            y += 1;
        }

        // Empty line
        y += 1;

        // Render tips
        for tip in &self.tips {
            if y >= inner.y + inner.height {
                break;
            }
            buf.set_string(inner.x + 1, y, *tip, Style::default().fg(self.dim_color));
            y += 1;
        }
    }
}

/// A card displaying key-value information pairs.
///
/// # Example
/// ```rust,ignore
/// use cortex_tui_components::welcome_card::InfoCard;
///
/// let card = InfoCard::new()
///     .add("Directory", "~/projects")
///     .add("User", "user@email.com")
///     .add("Model", "claude-3");
/// ```
pub struct InfoCard<'a> {
    items: Vec<(&'a str, String)>,
    dim_color: Color,
    text_color: Color,
    border_color: Color,
}

impl<'a> InfoCard<'a> {
    /// Create a new info card.
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            dim_color: TEXT_DIM,
            text_color: TEXT,
            border_color: CYAN_PRIMARY,
        }
    }

    /// Add a label-value pair.
    pub fn add(mut self, label: &'a str, value: impl Into<String>) -> Self {
        self.items.push((label, value.into()));
        self
    }

    /// Set the dim color for labels.
    pub fn dim_color(mut self, color: Color) -> Self {
        self.dim_color = color;
        self
    }

    /// Set the text color for values.
    pub fn text_color(mut self, color: Color) -> Self {
        self.text_color = color;
        self
    }

    /// Set the border color.
    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    /// Calculate the required height for this card.
    pub fn required_height(&self) -> u16 {
        // Border (2) + items
        2 + self.items.len() as u16
    }
}

impl Default for InfoCard<'_> {
    fn default() -> Self {
        Self::new()
    }
}

impl ToLines for InfoCard<'_> {
    fn to_lines(&self, width: u16) -> Vec<Line<'static>> {
        let w = width as usize;
        let bs = Style::default().fg(self.border_color);
        let inner = w.saturating_sub(4);

        let mut lines: Vec<Line<'static>> = Vec::new();

        // Top border
        lines.push(Line::from(Span::styled(
            format!("╭{}╮", "─".repeat(w.saturating_sub(2))),
            bs,
        )));

        // Content lines
        for (label, value) in &self.items {
            if label.is_empty() && value.is_empty() {
                // Empty row
                lines.push(Line::from(vec![
                    Span::styled("│", bs),
                    Span::raw(" ".repeat(w.saturating_sub(2))),
                    Span::styled("│", bs),
                ]));
            } else {
                let lbl = format!("{}: ", label);
                let avail = inner.saturating_sub(lbl.len());
                let val = if value.len() > avail {
                    format!("{}...", &value[..avail.saturating_sub(3)])
                } else {
                    value.clone()
                };
                let fill = inner.saturating_sub(lbl.len() + val.len());

                lines.push(Line::from(vec![
                    Span::styled("│ ", bs),
                    Span::styled(lbl, Style::default().fg(self.dim_color)),
                    Span::styled(val, Style::default().fg(self.text_color)),
                    Span::raw(" ".repeat(fill)),
                    Span::styled(" │", bs),
                ]));
            }
        }

        // Bottom border
        lines.push(Line::from(Span::styled(
            format!("╰{}╯", "─".repeat(w.saturating_sub(2))),
            bs,
        )));

        lines
    }
}

impl Widget for InfoCard<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        // Render border with custom color
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(ROUNDED_BORDER)
            .border_style(Style::default().fg(self.border_color));

        let inner = block.inner(area);
        block.render(area, buf);
        let content_width = inner.width.saturating_sub(2) as usize;

        for (i, (label, value)) in self.items.iter().enumerate() {
            let y = inner.y + i as u16;
            if y >= inner.y + inner.height {
                break;
            }

            let label_with_colon = format!("{}: ", label);
            let max_value_len = content_width.saturating_sub(label_with_colon.len());
            let truncated_value = if value.len() > max_value_len {
                format!("{}...", &value[..max_value_len.saturating_sub(3)])
            } else {
                value.clone()
            };

            // Render label
            buf.set_string(
                inner.x + 1,
                y,
                &label_with_colon,
                Style::default().fg(self.dim_color),
            );

            // Render value
            buf.set_string(
                inner.x + 1 + label_with_colon.len() as u16,
                y,
                &truncated_value,
                Style::default().fg(self.text_color),
            );
        }
    }
}

/// Renders two info cards side by side.
///
/// # Example
/// ```rust,ignore
/// use cortex_tui_components::welcome_card::{InfoCardPair, InfoCard};
///
/// let left = InfoCard::new().add("Dir", "~/projects").add("User", "me@email.com");
/// let right = InfoCard::new().add("Model", "claude-3").add("Plan", "Pro");
///
/// InfoCardPair::new(left, right).render(area, buf);
/// ```
pub struct InfoCardPair<'a> {
    left: InfoCard<'a>,
    right: InfoCard<'a>,
    gap: u16,
    right_width: u16,
}

impl<'a> InfoCardPair<'a> {
    /// Create a new info card pair.
    pub fn new(left: InfoCard<'a>, right: InfoCard<'a>) -> Self {
        Self {
            left,
            right,
            gap: 2,
            right_width: 25,
        }
    }

    /// Set the gap between cards.
    pub fn gap(mut self, gap: u16) -> Self {
        self.gap = gap;
        self
    }

    /// Set the width of the right card.
    pub fn right_width(mut self, width: u16) -> Self {
        self.right_width = width;
        self
    }
}

impl ToLines for InfoCardPair<'_> {
    fn to_lines(&self, width: u16) -> Vec<Line<'static>> {
        let total_width = (width as usize).max(40); // Adaptive width

        if total_width < (self.right_width as usize + self.gap as usize + 20) {
            return self.left.to_lines(width);
        }

        let left_width = total_width.saturating_sub(self.right_width as usize + self.gap as usize);
        let left_lines = self.left.to_lines(left_width as u16);
        let right_lines = self.right.to_lines(self.right_width);
        let gap = " ".repeat(self.gap as usize);

        let mut lines: Vec<Line<'static>> = Vec::new();
        let max_len = left_lines.len().max(right_lines.len());

        for i in 0..max_len {
            let left_part = left_lines.get(i);
            let right_part = right_lines.get(i);

            let mut spans: Vec<Span<'static>> = Vec::new();

            if let Some(l) = left_part {
                spans.extend(l.spans.iter().cloned());
            } else {
                spans.push(Span::raw(" ".repeat(left_width)));
            }

            spans.push(Span::raw(gap.clone()));

            if let Some(r) = right_part {
                spans.extend(r.spans.iter().cloned());
            } else {
                spans.push(Span::raw(" ".repeat(self.right_width as usize)));
            }

            lines.push(Line::from(spans));
        }

        lines
    }
}

impl Widget for InfoCardPair<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < self.right_width + self.gap + 20 {
            // Not enough space, render left card only
            self.left.render(area, buf);
            return;
        }

        let left_width = area.width.saturating_sub(self.right_width + self.gap);

        let left_area = Rect::new(area.x, area.y, left_width, area.height);
        let right_area = Rect::new(
            area.x + left_width + self.gap,
            area.y,
            self.right_width,
            area.height,
        );

        self.left.render(left_area, buf);
        self.right.render(right_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_welcome_card_builder() {
        let card = WelcomeCard::new()
            .user_name("Test")
            .subtitle("Test subtitle")
            .version("1.0.0")
            .tips(&["Tip 1", "Tip 2"]);

        assert_eq!(card.user_name, Some("Test"));
        assert_eq!(card.subtitle, Some("Test subtitle"));
        assert_eq!(card.version, Some("1.0.0"));
        assert_eq!(card.tips.len(), 2);
    }

    #[test]
    fn test_info_card_builder() {
        let card = InfoCard::new()
            .add("Label1", "Value1")
            .add("Label2", "Value2");

        assert_eq!(card.items.len(), 2);
    }

    #[test]
    fn test_welcome_card_height() {
        let card = WelcomeCard::new().tips(&["Tip 1", "Tip 2"]);
        assert!(card.required_height() >= 10);
    }
}
