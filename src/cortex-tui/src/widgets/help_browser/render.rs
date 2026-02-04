//! Help browser widget rendering.

use cortex_core::style::{
    BORDER, BORDER_FOCUS, CYAN_PRIMARY, ELECTRIC_BLUE, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED, VOID,
};
use ratatui::prelude::*;
use ratatui::widgets::Widget;

use super::content::HelpContent;
use super::state::{HelpBrowserState, HelpFocus};
use super::utils::wrap_text;

// ============================================================
// STYLED LINE HELPER
// ============================================================

/// A line of text with styling.
struct StyledLine {
    text: String,
    style: Style,
}

impl StyledLine {
    fn new(text: String, style: Style) -> Self {
        Self { text, style }
    }
}

// ============================================================
// HELP BROWSER WIDGET
// ============================================================

/// Widget for displaying the help browser.
pub struct HelpBrowser<'a> {
    state: &'a HelpBrowserState,
}

impl<'a> HelpBrowser<'a> {
    /// Creates a new help browser widget.
    ///
    /// # Arguments
    /// * `state` - Reference to the help browser state
    pub fn new(state: &'a HelpBrowserState) -> Self {
        Self { state }
    }

    /// Renders the background and border.
    fn render_background(&self, area: Rect, buf: &mut Buffer) {
        // Fill background
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_bg(VOID);
                }
            }
        }

        // Draw border
        let border_style = Style::default().fg(BORDER_FOCUS);

        // Corners
        buf.set_string(area.x, area.y, "+", border_style);
        buf.set_string(area.x + area.width - 1, area.y, "+", border_style);
        buf.set_string(area.x, area.y + area.height - 1, "+", border_style);
        buf.set_string(
            area.x + area.width - 1,
            area.y + area.height - 1,
            "+",
            border_style,
        );

        // Horizontal borders
        for x in area.x + 1..area.x + area.width - 1 {
            buf.set_string(x, area.y, "-", border_style);
            buf.set_string(x, area.y + area.height - 1, "-", border_style);
        }

        // Vertical borders
        for y in area.y + 1..area.y + area.height - 1 {
            buf.set_string(area.x, y, "|", border_style);
            buf.set_string(area.x + area.width - 1, y, "|", border_style);
        }
    }

    /// Renders the header with title.
    fn render_header(&self, area: Rect, buf: &mut Buffer) {
        let title = " Help ";
        let close = "[X]";

        // Title
        let title_x = area.x + 2;
        buf.set_string(
            title_x,
            area.y,
            title,
            Style::default()
                .fg(CYAN_PRIMARY)
                .add_modifier(Modifier::BOLD),
        );

        // Close button
        let close_x = area.x + area.width - close.len() as u16 - 2;
        buf.set_string(close_x, area.y, close, Style::default().fg(TEXT_MUTED));

        // Separator below header
        let sep_y = area.y + 1;
        buf.set_string(area.x, sep_y, "+", Style::default().fg(BORDER));
        for x in area.x + 1..area.x + area.width - 1 {
            buf.set_string(x, sep_y, "-", Style::default().fg(BORDER));
        }
        buf.set_string(
            area.x + area.width - 1,
            sep_y,
            "+",
            Style::default().fg(BORDER),
        );
    }

    /// Renders the sidebar with section navigation.
    fn render_sidebar(&self, area: Rect, buf: &mut Buffer) {
        let is_focused = self.state.focus == HelpFocus::Sidebar;

        for (i, section) in self.state.sections.iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.y + area.height {
                break;
            }

            let is_selected = i == self.state.selected_section;
            let prefix = if is_selected { "> " } else { "  " };

            let style = if is_selected && is_focused {
                Style::default().fg(VOID).bg(CYAN_PRIMARY)
            } else if is_selected {
                Style::default().fg(CYAN_PRIMARY)
            } else {
                Style::default().fg(TEXT_DIM)
            };

            // Clear line with style background
            for x in area.x..area.x + area.width {
                if let Some(cell) = buf.cell_mut((x, y)) {
                    cell.set_char(' ').set_style(style);
                }
            }

            let text = format!("{}{}", prefix, section.title);
            let truncated: String = text.chars().take(area.width as usize).collect();
            buf.set_string(area.x, y, &truncated, style);
        }
    }

    /// Renders the content pane.
    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        let Some(section) = self.state.current_section() else {
            return;
        };
        let mut y = area.y;
        let scroll = self.state.content_scroll;
        let mut line_idx = 0;

        for content in &section.content {
            if y >= area.y + area.height {
                break;
            }

            let lines = self.render_content_item(content, area.width);

            for line in lines {
                if line_idx >= scroll {
                    if y >= area.y + area.height {
                        break;
                    }
                    let truncated: String = line.text.chars().take(area.width as usize).collect();
                    buf.set_string(area.x, y, &truncated, line.style);
                    y += 1;
                }
                line_idx += 1;
            }
        }
    }

    /// Renders a single content item into styled lines.
    fn render_content_item(&self, content: &HelpContent, width: u16) -> Vec<StyledLine> {
        match content {
            HelpContent::Title(text) => {
                vec![StyledLine::new(
                    text.clone(),
                    Style::default()
                        .fg(CYAN_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )]
            }
            HelpContent::Paragraph(text) => wrap_text(text, width as usize)
                .into_iter()
                .map(|s| StyledLine::new(s, Style::default().fg(TEXT)))
                .collect(),
            HelpContent::KeyBinding { key, description } => {
                let line = format!("{:<20} {}", key, description);
                vec![StyledLine::new(line, Style::default().fg(TEXT_DIM))]
            }
            HelpContent::Command {
                name,
                description,
                usage,
            } => {
                vec![
                    StyledLine::new(
                        format!("{:<15} {}", name, description),
                        Style::default().fg(TEXT),
                    ),
                    StyledLine::new(
                        format!("               Usage: {}", usage),
                        Style::default().fg(TEXT_MUTED),
                    ),
                ]
            }
            HelpContent::List(items) => items
                .iter()
                .map(|item| StyledLine::new(format!("  * {}", item), Style::default().fg(TEXT_DIM)))
                .collect(),
            HelpContent::Code(code) => {
                vec![StyledLine::new(
                    format!("  {}", code),
                    Style::default().fg(ELECTRIC_BLUE).bg(SURFACE_1),
                )]
            }
            HelpContent::Separator => {
                let line = "-".repeat(width as usize);
                vec![StyledLine::new(line, Style::default().fg(BORDER))]
            }
        }
    }

    /// Renders the footer with key hints.
    fn render_footer(&self, area: Rect, buf: &mut Buffer) {
        let y = area.y + area.height - 2;

        // Separator above footer
        buf.set_string(area.x, y, "+", Style::default().fg(BORDER));
        for x in area.x + 1..area.x + area.width - 1 {
            buf.set_string(x, y, "-", Style::default().fg(BORDER));
        }
        buf.set_string(area.x + area.width - 1, y, "+", Style::default().fg(BORDER));

        // Footer text
        let footer_y = area.y + area.height - 1;
        let hints = "[/] Search  [Tab] Switch pane  [Esc] Close";

        // Clear footer line
        for x in area.x + 1..area.x + area.width - 1 {
            if let Some(cell) = buf.cell_mut((x, footer_y)) {
                cell.set_char(' ').set_bg(VOID);
            }
        }

        // Center the hints
        let hint_x = area.x + (area.width.saturating_sub(hints.len() as u16)) / 2;
        buf.set_string(hint_x, footer_y, hints, Style::default().fg(TEXT_MUTED));
    }

    /// Renders the vertical separator between sidebar and content.
    fn render_separator(&self, x: u16, start_y: u16, end_y: u16, buf: &mut Buffer) {
        for y in start_y..end_y {
            buf.set_string(x, y, "|", Style::default().fg(BORDER));
        }
    }
}

impl Widget for HelpBrowser<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 20 || area.height < 10 {
            return;
        }

        // Calculate centered modal area
        let width = 70.min(area.width.saturating_sub(4));
        let height = 30.min(area.height.saturating_sub(4));
        let x = area.x + (area.width.saturating_sub(width)) / 2;
        let y = area.y + (area.height.saturating_sub(height)) / 2;
        let help_area = Rect::new(x, y, width, height);

        // Render background and border
        self.render_background(help_area, buf);

        // Render header
        self.render_header(help_area, buf);

        // Calculate inner areas
        let inner_y = help_area.y + 2;
        let inner_height = help_area.height.saturating_sub(4);

        // Sidebar width (20 chars or 1/3 of width, whichever is smaller)
        let sidebar_width = 20.min(width / 3);
        let sidebar_area = Rect::new(help_area.x + 1, inner_y, sidebar_width, inner_height);

        // Content area
        let content_x = help_area.x + sidebar_width + 2;
        let content_width = width.saturating_sub(sidebar_width + 3);
        let content_area = Rect::new(content_x, inner_y, content_width, inner_height);

        // Render sidebar
        self.render_sidebar(sidebar_area, buf);

        // Render vertical separator
        let sep_x = help_area.x + sidebar_width + 1;
        self.render_separator(sep_x, inner_y, inner_y + inner_height, buf);

        // Render content
        self.render_content(content_area, buf);

        // Render footer
        self.render_footer(help_area, buf);
    }
}
