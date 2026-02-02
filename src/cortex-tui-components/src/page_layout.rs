//! Page layout components for consistent command UI styling.
//!
//! Provides reusable components for building command pages with tabs, badges, and sections.
//!
//! # Example
//! ```ignore
//! let page = PageLayout::new()
//!     .top_border()
//!     .navbar(Navbar::new()
//!         .item("Settings", true)
//!         .item("Status", false)
//!         .item("Config", false))
//!     .section(InfoSection::new()
//!         .add("Version", "1.0.0"))
//!     .footer("Esc to cancel");
//! ```

use cortex_core::style::{CYAN_PRIMARY, TEXT, TEXT_DIM, TEXT_MUTED, WARNING};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

/// A badge with background color for highlighting text.
#[derive(Debug, Clone)]
pub struct Badge {
    text: String,
    bg_color: Color,
    fg_color: Color,
}

impl Badge {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            bg_color: CYAN_PRIMARY,
            fg_color: Color::Black,
        }
    }

    pub fn bg(mut self, color: Color) -> Self {
        self.bg_color = color;
        self
    }

    pub fn fg(mut self, color: Color) -> Self {
        self.fg_color = color;
        self
    }

    pub fn to_span(&self) -> Span<'static> {
        Span::styled(
            format!(" {} ", self.text),
            Style::default().fg(self.fg_color).bg(self.bg_color),
        )
    }
}

/// A navigation bar item.
#[derive(Debug, Clone)]
pub struct NavItem {
    pub label: String,
    pub active: bool,
}

impl NavItem {
    pub fn new(label: impl Into<String>, active: bool) -> Self {
        Self {
            label: label.into(),
            active,
        }
    }
}

/// Navigation bar with items and optional hint.
#[derive(Debug, Clone)]
pub struct Navbar {
    items: Vec<NavItem>,
    hint: Option<String>,
}

impl Navbar {
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            hint: None,
        }
    }

    pub fn item(mut self, label: impl Into<String>, active: bool) -> Self {
        self.items.push(NavItem::new(label, active));
        self
    }

    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Alias for `item()` for backward compatibility.
    pub fn tab(self, label: impl Into<String>, active: bool) -> Self {
        self.item(label, active)
    }

    pub fn to_line(&self) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();

        spans.push(Span::raw(" "));

        for (i, item) in self.items.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled("   ", Style::default().fg(TEXT_DIM)));
            }

            if item.active {
                spans.push(Span::styled(
                    format!(" {} ", item.label),
                    Style::default()
                        .fg(Color::Black)
                        .bg(CYAN_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    item.label.clone(),
                    Style::default().fg(TEXT_DIM),
                ));
            }
        }

        if let Some(ref hint) = self.hint {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(hint.clone(), Style::default().fg(TEXT_MUTED)));
        }

        Line::from(spans)
    }
}

impl Default for Navbar {
    fn default() -> Self {
        Self::new()
    }
}

// Keep PageTab and PageHeader as aliases for compatibility
pub type PageTab = NavItem;
pub type PageHeader = Navbar;

/// An info item (label: value pair).
#[derive(Debug, Clone)]
pub struct InfoItem {
    pub label: String,
    pub value: String,
    pub value_style: Style,
    pub icon: Option<char>,
}

impl InfoItem {
    pub fn new(label: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            value_style: Style::default().fg(TEXT),
            icon: None,
        }
    }

    pub fn style(mut self, style: Style) -> Self {
        self.value_style = style;
        self
    }

    pub fn icon(mut self, icon: char) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn warning(mut self) -> Self {
        self.icon = Some('⚠');
        self.value_style = Style::default().fg(WARNING);
        self
    }

    pub fn success(mut self) -> Self {
        self.icon = Some('✔');
        self.value_style = Style::default().fg(cortex_core::style::SUCCESS);
        self
    }

    pub fn to_line(&self) -> Line<'static> {
        let mut spans: Vec<Span<'static>> = Vec::new();

        spans.push(Span::raw("  "));

        if let Some(icon) = self.icon {
            spans.push(Span::styled(format!("{} ", icon), self.value_style));
        }

        if !self.label.is_empty() {
            spans.push(Span::styled(
                format!("{}: ", self.label),
                Style::default().fg(TEXT_DIM),
            ));
        }

        spans.push(Span::styled(self.value.clone(), self.value_style));

        Line::from(spans)
    }
}

/// A section with optional title and info items.
#[derive(Debug, Clone)]
pub struct InfoSection {
    title: Option<String>,
    items: Vec<InfoItem>,
}

impl InfoSection {
    pub fn new() -> Self {
        Self {
            title: None,
            items: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn item(mut self, item: InfoItem) -> Self {
        self.items.push(item);
        self
    }

    pub fn add(mut self, label: impl Into<String>, value: impl Into<String>) -> Self {
        self.items.push(InfoItem::new(label, value));
        self
    }

    pub fn to_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();

        if let Some(ref title) = self.title {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    title.clone(),
                    Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
                ),
            ]));
        }

        for item in &self.items {
            lines.push(item.to_line());
        }

        lines
    }
}

impl Default for InfoSection {
    fn default() -> Self {
        Self::new()
    }
}

/// Complete page layout with navbar, sections, and footer.
#[derive(Debug, Clone)]
pub struct PageLayout {
    navbar: Option<Navbar>,
    sections: Vec<InfoSection>,
    footer_hint: Option<String>,
    border_color: Color,
    show_top_border: bool,
    show_bottom_border: bool,
}

impl PageLayout {
    pub fn new() -> Self {
        Self {
            navbar: None,
            sections: Vec::new(),
            footer_hint: None,
            border_color: CYAN_PRIMARY,
            show_top_border: true,
            show_bottom_border: false,
        }
    }

    pub fn navbar(mut self, navbar: Navbar) -> Self {
        self.navbar = Some(navbar);
        self
    }

    /// Alias for navbar() for compatibility
    pub fn header(self, header: PageHeader) -> Self {
        self.navbar(header)
    }

    pub fn section(mut self, section: InfoSection) -> Self {
        self.sections.push(section);
        self
    }

    pub fn footer(mut self, hint: impl Into<String>) -> Self {
        self.footer_hint = Some(hint.into());
        self
    }

    pub fn border_color(mut self, color: Color) -> Self {
        self.border_color = color;
        self
    }

    pub fn top_border(mut self, show: bool) -> Self {
        self.show_top_border = show;
        self
    }

    pub fn bottom_border(mut self, show: bool) -> Self {
        self.show_bottom_border = show;
        self
    }

    pub fn to_lines(&self, width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let w = width as usize;
        let border_style = Style::default().fg(self.border_color);

        // Top border (only if enabled)
        if self.show_top_border {
            lines.push(Line::from(Span::styled("─".repeat(w), border_style)));
        }

        // Navbar
        if let Some(ref navbar) = self.navbar {
            lines.push(navbar.to_line());
            lines.push(Line::from(""));
            lines.push(Line::from(""));
        }

        // Sections
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from("")); // Gap between sections
            }
            lines.extend(section.to_lines());
        }

        // Footer hint
        if let Some(ref hint) = self.footer_hint {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(hint.clone(), Style::default().fg(TEXT_MUTED)),
            ]));
        }

        // Bottom border (only if enabled)
        if self.show_bottom_border {
            lines.push(Line::from(Span::styled("─".repeat(w), border_style)));
        }

        lines
    }
}

impl Default for PageLayout {
    fn default() -> Self {
        Self::new()
    }
}

impl Widget for PageLayout {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = self.to_lines(area.width);

        for (i, line) in lines.iter().enumerate() {
            if i as u16 >= area.height {
                break;
            }
            buf.set_line(area.x, area.y + i as u16, line, area.width);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_badge() {
        let badge = Badge::new("Test");
        let span = badge.to_span();
        assert!(span.content.contains("Test"));
    }

    #[test]
    fn test_page_header() {
        let header = PageHeader::new()
            .tab("Settings", true)
            .tab("Status", false)
            .hint("(tab to cycle)");
        let line = header.to_line();
        assert!(!line.spans.is_empty());
    }

    #[test]
    fn test_info_section() {
        let section = InfoSection::new()
            .title("Info")
            .add("Version", "1.0.0")
            .add("Author", "Test");
        let lines = section.to_lines();
        assert_eq!(lines.len(), 3); // title + 2 items
    }
}
