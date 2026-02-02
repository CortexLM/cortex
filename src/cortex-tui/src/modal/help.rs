//! Help Modal
//!
//! A modal that displays help content including keyboard shortcuts,
//! slash commands, and navigation instructions with scroll support.

use super::{CancelBehavior, Modal, ModalResult};
use cortex_core::style::{CYAN_PRIMARY, TEXT, TEXT_DIM};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

// ============================================================================
// HELP SECTION & ITEM
// ============================================================================

/// A single help item with a key/command and description.
#[derive(Debug, Clone)]
pub struct HelpItem {
    /// The key or command (e.g., "Ctrl+K" or "/model")
    pub key: String,
    /// Description of what it does
    pub description: String,
}

impl HelpItem {
    /// Create a new help item.
    pub fn new(key: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            description: description.into(),
        }
    }
}

/// A section of help content with a title and items.
#[derive(Debug, Clone)]
pub struct HelpSection {
    /// Section title (e.g., "Keyboard Shortcuts")
    pub title: String,
    /// Items in this section
    pub items: Vec<HelpItem>,
}

impl HelpSection {
    /// Create a new help section.
    pub fn new(title: impl Into<String>, items: Vec<HelpItem>) -> Self {
        Self {
            title: title.into(),
            items,
        }
    }
}

// ============================================================================
// HELP MODAL
// ============================================================================

/// A help modal displaying scrollable help content.
pub struct HelpModal {
    /// Help sections to display
    sections: Vec<HelpSection>,
    /// Current scroll offset
    scroll_offset: usize,
    /// Total lines of content
    total_lines: usize,
    /// Cached visible height for scrolling
    visible_height: usize,
}

impl HelpModal {
    /// Create a new help modal with default sections.
    pub fn new() -> Self {
        let sections = Self::default_sections();
        let total_lines = Self::calculate_total_lines(&sections);
        Self {
            sections,
            scroll_offset: 0,
            total_lines,
            visible_height: 20, // Default, updated on render
        }
    }

    /// Create a help modal with an optional topic filter.
    /// If topic is Some, shows help for that specific topic.
    /// If topic is None, shows all help sections.
    pub fn with_topic(topic: Option<String>) -> Self {
        let sections = if let Some(ref topic_str) = topic {
            Self::sections_for_topic(topic_str)
        } else {
            Self::default_sections()
        };
        let total_lines = Self::calculate_total_lines(&sections);
        Self {
            sections,
            scroll_offset: 0,
            total_lines,
            visible_height: 20,
        }
    }

    /// Create a help modal with custom sections.
    pub fn with_sections(sections: Vec<HelpSection>) -> Self {
        let total_lines = Self::calculate_total_lines(&sections);
        Self {
            sections,
            scroll_offset: 0,
            total_lines,
            visible_height: 20,
        }
    }

    /// Get help sections for a specific topic.
    fn sections_for_topic(topic: &str) -> Vec<HelpSection> {
        match topic.to_lowercase().as_str() {
            "keys" | "keyboard" | "shortcuts" => vec![HelpSection::new(
                "Keyboard Shortcuts",
                vec![
                    HelpItem::new("Ctrl+K", "Open command palette"),
                    HelpItem::new("Ctrl+M", "Change model"),
                    HelpItem::new("Ctrl+S", "View sessions"),
                    HelpItem::new("Ctrl+N", "New session"),
                    HelpItem::new("Ctrl+T", "View transcript"),
                    HelpItem::new("Esc", "Cancel/interrupt"),
                    HelpItem::new("Tab", "Navigate autocomplete"),
                    HelpItem::new("?", "Show this help"),
                ],
            )],
            "commands" | "slash" => vec![HelpSection::new(
                "Slash Commands",
                vec![
                    HelpItem::new("/model", "Change AI model"),
                    HelpItem::new("/mcp", "Manage MCP servers"),
                    HelpItem::new("/sessions", "View sessions"),
                    HelpItem::new("/settings", "Open settings"),
                    HelpItem::new("/export", "Export session"),
                    HelpItem::new("/clear", "Clear context"),
                    HelpItem::new("/help", "Show help"),
                    HelpItem::new("/approval", "Set approval mode"),
                    HelpItem::new("/logs", "Set log level"),
                ],
            )],
            "navigation" | "nav" => vec![HelpSection::new(
                "Navigation",
                vec![
                    HelpItem::new("Up/Down", "Move selection"),
                    HelpItem::new("Enter", "Confirm/select"),
                    HelpItem::new("Tab", "Next field"),
                    HelpItem::new("Shift+Tab", "Previous field"),
                    HelpItem::new("PgUp/PgDn", "Scroll page"),
                    HelpItem::new("Home/End", "Go to top/bottom"),
                ],
            )],
            _ => Self::default_sections(),
        }
    }

    /// Get the default help sections.
    fn default_sections() -> Vec<HelpSection> {
        vec![
            HelpSection::new(
                "Keyboard Shortcuts",
                vec![
                    HelpItem::new("Ctrl+K", "Open command palette"),
                    HelpItem::new("Ctrl+M", "Change model"),
                    HelpItem::new("Ctrl+S", "View sessions"),
                    HelpItem::new("Ctrl+N", "New session"),
                    HelpItem::new("Ctrl+T", "View transcript"),
                    HelpItem::new("Esc", "Cancel/interrupt"),
                    HelpItem::new("?", "Show this help"),
                ],
            ),
            HelpSection::new(
                "Slash Commands",
                vec![
                    HelpItem::new("/model", "Change AI model"),
                    HelpItem::new("/mcp", "Manage MCP servers"),
                    HelpItem::new("/sessions", "View sessions"),
                    HelpItem::new("/settings", "Open settings"),
                    HelpItem::new("/export", "Export session"),
                    HelpItem::new("/clear", "Clear context"),
                    HelpItem::new("/help", "Show help"),
                ],
            ),
            HelpSection::new(
                "Navigation",
                vec![
                    HelpItem::new("\u{2191}\u{2193}", "Move selection"),
                    HelpItem::new("Enter", "Confirm/select"),
                    HelpItem::new("Tab", "Next field"),
                    HelpItem::new("Shift+Tab", "Previous field"),
                ],
            ),
        ]
    }

    /// Calculate total lines needed for all sections.
    fn calculate_total_lines(sections: &[HelpSection]) -> usize {
        let mut lines = 0;
        for (i, section) in sections.iter().enumerate() {
            if i > 0 {
                lines += 1; // Blank line between sections
            }
            lines += 1; // Section header
            lines += section.items.len(); // Items
        }
        lines
    }

    /// Scroll up by one line.
    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll down by one line.
    fn scroll_down(&mut self) {
        let max_offset = self.total_lines.saturating_sub(self.visible_height);
        if self.scroll_offset < max_offset {
            self.scroll_offset += 1;
        }
    }

    /// Scroll up by a page.
    fn page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.visible_height);
    }

    /// Scroll down by a page.
    fn page_down(&mut self) {
        let max_offset = self.total_lines.saturating_sub(self.visible_height);
        self.scroll_offset = (self.scroll_offset + self.visible_height).min(max_offset);
    }

    /// Build content lines for rendering.
    fn build_lines(&self) -> Vec<ContentLine> {
        let mut lines = Vec::new();
        for (i, section) in self.sections.iter().enumerate() {
            if i > 0 {
                lines.push(ContentLine::Blank);
            }
            lines.push(ContentLine::Header(section.title.clone()));
            for item in &section.items {
                lines.push(ContentLine::Item {
                    key: item.key.clone(),
                    description: item.description.clone(),
                });
            }
        }
        lines
    }
}

impl Default for HelpModal {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// MODAL IMPLEMENTATION
// ============================================================================

impl Modal for HelpModal {
    fn title(&self) -> &str {
        "Help"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        let content_height = self.total_lines as u16;
        // Add some padding, cap at max
        (content_height + 2).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 10 {
            return;
        }

        // Update visible height for scroll calculations (we can't mutate self here,
        // but scroll methods use the stored value which gets updated via handle_key)
        let visible_height = area.height as usize;

        let lines = self.build_lines();

        // Render visible lines
        let start = self.scroll_offset;
        let end = (start + visible_height).min(lines.len());

        for (row_offset, line) in lines.iter().skip(start).take(end - start).enumerate() {
            let y = area.y + row_offset as u16;
            if y >= area.bottom() {
                break;
            }

            match line {
                ContentLine::Blank => {
                    // Empty line, nothing to render
                }
                ContentLine::Header(title) => {
                    // Section header in cyan
                    buf.set_string(area.x + 1, y, title, Style::default().fg(CYAN_PRIMARY));
                }
                ContentLine::Item { key, description } => {
                    // Key in primary color, description in dim text
                    let key_width = 14u16;
                    let key_display = format!("{:width$}", key, width = key_width as usize);

                    buf.set_string(area.x + 2, y, &key_display, Style::default().fg(TEXT));

                    let desc_x = area.x + 2 + key_width + 1;
                    let desc_max_width = area.width.saturating_sub(key_width + 4) as usize;
                    let desc_display = if description.len() > desc_max_width {
                        format!("{}...", &description[..desc_max_width.saturating_sub(3)])
                    } else {
                        description.clone()
                    };

                    buf.set_string(desc_x, y, &desc_display, Style::default().fg(TEXT_DIM));
                }
            }
        }

        // Scroll indicators
        if lines.len() > visible_height {
            // Up indicator
            if self.scroll_offset > 0 {
                let x = area.right().saturating_sub(2);
                buf.set_string(x, area.y, "\u{25B2}", Style::default().fg(TEXT_DIM));
            }

            // Down indicator
            if self.scroll_offset + visible_height < lines.len() {
                let x = area.right().saturating_sub(2);
                let y = area.bottom().saturating_sub(1);
                buf.set_string(x, y, "\u{25BC}", Style::default().fg(TEXT_DIM));
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match key.code {
            // Close modal
            KeyCode::Esc | KeyCode::Char('q') => ModalResult::Close,
            KeyCode::Enter => ModalResult::Close,

            // Navigation
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll_up();
                ModalResult::Continue
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll_down();
                ModalResult::Continue
            }
            KeyCode::PageUp => {
                self.page_up();
                ModalResult::Continue
            }
            KeyCode::PageDown => {
                self.page_down();
                ModalResult::Continue
            }
            KeyCode::Home => {
                self.scroll_offset = 0;
                ModalResult::Continue
            }
            KeyCode::End => {
                let max_offset = self.total_lines.saturating_sub(self.visible_height);
                self.scroll_offset = max_offset;
                ModalResult::Continue
            }

            _ => ModalResult::Continue,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("\u{2191}\u{2193}", "scroll"),
            ("PgUp/PgDn", "page"),
            ("Esc/q", "close"),
        ]
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        CancelBehavior::Close
    }
}

// ============================================================================
// CONTENT LINE
// ============================================================================

/// Internal enum for content lines.
enum ContentLine {
    Blank,
    Header(String),
    Item { key: String, description: String },
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn test_new_creates_default_sections() {
        let modal = HelpModal::new();
        assert_eq!(modal.sections.len(), 3);
        assert_eq!(modal.sections[0].title, "Keyboard Shortcuts");
        assert_eq!(modal.sections[1].title, "Slash Commands");
        assert_eq!(modal.sections[2].title, "Navigation");
    }

    #[test]
    fn test_with_sections() {
        let sections = vec![HelpSection::new(
            "Custom",
            vec![HelpItem::new("Test", "A test item")],
        )];
        let modal = HelpModal::with_sections(sections);
        assert_eq!(modal.sections.len(), 1);
        assert_eq!(modal.sections[0].title, "Custom");
    }

    #[test]
    fn test_title() {
        let modal = HelpModal::new();
        assert_eq!(modal.title(), "Help");
    }

    #[test]
    fn test_scroll_bounds() {
        let mut modal = HelpModal::new();
        modal.total_lines = 100;
        modal.visible_height = 20;
        modal.scroll_offset = 0;

        // Scroll up at top should stay at 0
        modal.scroll_up();
        assert_eq!(modal.scroll_offset, 0);

        // Scroll down
        modal.scroll_down();
        assert_eq!(modal.scroll_offset, 1);

        // Page down
        modal.page_down();
        assert!(modal.scroll_offset > 1);
    }

    #[test]
    fn test_escape_closes() {
        let mut modal = HelpModal::new();
        let result = modal.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_q_closes() {
        let mut modal = HelpModal::new();
        let result = modal.handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_enter_closes() {
        let mut modal = HelpModal::new();
        let result = modal.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_navigation_keys() {
        let mut modal = HelpModal::new();
        modal.total_lines = 100;
        modal.visible_height = 20;

        // Down arrow
        let result = modal.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Continue));
        assert_eq!(modal.scroll_offset, 1);

        // Up arrow
        let result = modal.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Continue));
        assert_eq!(modal.scroll_offset, 0);

        // j key (vim-style)
        let result = modal.handle_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Continue));
        assert_eq!(modal.scroll_offset, 1);

        // k key (vim-style)
        let result = modal.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Continue));
        assert_eq!(modal.scroll_offset, 0);
    }

    #[test]
    fn test_home_end_keys() {
        let mut modal = HelpModal::new();
        modal.total_lines = 100;
        modal.visible_height = 20;
        modal.scroll_offset = 50;

        // Home goes to top
        let result = modal.handle_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Continue));
        assert_eq!(modal.scroll_offset, 0);

        // End goes to bottom
        let result = modal.handle_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        assert!(matches!(result, ModalResult::Continue));
        assert_eq!(modal.scroll_offset, 80); // 100 - 20
    }

    #[test]
    fn test_key_hints() {
        let modal = HelpModal::new();
        let hints = modal.key_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(_, desc)| *desc == "scroll"));
        assert!(hints.iter().any(|(_, desc)| *desc == "close"));
    }

    #[test]
    fn test_build_lines() {
        let modal = HelpModal::new();
        let lines = modal.build_lines();

        // Should have headers and items
        assert!(!lines.is_empty());

        // First line should be a header
        assert!(matches!(lines[0], ContentLine::Header(_)));
    }

    #[test]
    fn test_calculate_total_lines() {
        let sections = vec![
            HelpSection::new(
                "Section 1",
                vec![HelpItem::new("a", "A"), HelpItem::new("b", "B")],
            ),
            HelpSection::new("Section 2", vec![HelpItem::new("c", "C")]),
        ];
        let total = HelpModal::calculate_total_lines(&sections);
        // Section 1: 1 header + 2 items = 3
        // Blank line = 1
        // Section 2: 1 header + 1 item = 2
        // Total = 6
        assert_eq!(total, 6);
    }
}
