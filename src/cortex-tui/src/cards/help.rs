//! Help Card
//!
//! A modal card that displays searchable help content including
//! keyboard shortcuts, slash commands, and navigation instructions.

use super::{CancellationEvent, CardResult, CardView};
use cortex_core::style::{CYAN_PRIMARY, SURFACE_0, SURFACE_1, TEXT, TEXT_DIM, TEXT_MUTED};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

// ============================================================
// HELP SECTION & ITEM
// ============================================================

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

// ============================================================
// HELP CARD
// ============================================================

/// A help card displaying searchable help content.
pub struct HelpCard {
    /// Help sections to display
    sections: Vec<HelpSection>,
    /// Current scroll offset
    scroll_offset: usize,
    /// Search query for filtering
    search_query: String,
    /// Filtered items as (section_idx, item_idx) pairs
    filtered_items: Vec<(usize, usize)>,
    /// Total height needed for content
    total_height: usize,
    /// Whether search input is active
    search_active: bool,
}

impl HelpCard {
    /// Create a new help card with default sections.
    pub fn new() -> Self {
        let sections = Self::default_sections();
        let mut card = Self {
            sections,
            scroll_offset: 0,
            search_query: String::new(),
            filtered_items: Vec::new(),
            total_height: 0,
            search_active: false,
        };
        card.rebuild_filtered();
        card
    }

    /// Create a help card with custom sections.
    pub fn with_sections(sections: Vec<HelpSection>) -> Self {
        let mut card = Self {
            sections,
            scroll_offset: 0,
            search_query: String::new(),
            filtered_items: Vec::new(),
            total_height: 0,
            search_active: false,
        };
        card.rebuild_filtered();
        card
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

    /// Rebuild the filtered items list based on the current search query.
    fn rebuild_filtered(&mut self) {
        self.filtered_items.clear();

        let query_lower = self.search_query.to_lowercase();

        for (section_idx, section) in self.sections.iter().enumerate() {
            for (item_idx, item) in section.items.iter().enumerate() {
                if self.search_query.is_empty()
                    || item.key.to_lowercase().contains(&query_lower)
                    || item.description.to_lowercase().contains(&query_lower)
                {
                    self.filtered_items.push((section_idx, item_idx));
                }
            }
        }

        // Recalculate total height
        self.total_height = self.calculate_content_height();

        // Ensure scroll offset is valid
        if self.scroll_offset > self.total_height.saturating_sub(1) {
            self.scroll_offset = self.total_height.saturating_sub(1);
        }
    }

    /// Calculate the total height needed for filtered content.
    fn calculate_content_height(&self) -> usize {
        if self.filtered_items.is_empty() {
            return 1; // "No matches" message
        }

        let mut height = 0;
        let mut current_section: Option<usize> = None;

        for &(section_idx, _) in &self.filtered_items {
            // Add section header if new section
            if current_section != Some(section_idx) {
                if current_section.is_some() {
                    height += 1; // Blank line between sections
                }
                height += 1; // Section header
                current_section = Some(section_idx);
            }
            height += 1; // Item line
        }

        height
    }

    /// Scroll up by one line.
    fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    /// Scroll down by one line.
    fn scroll_down(&mut self, visible_height: usize) {
        let max_offset = self.total_height.saturating_sub(visible_height);
        if self.scroll_offset < max_offset {
            self.scroll_offset += 1;
        }
    }

    /// Scroll up by a page.
    fn page_up(&mut self, visible_height: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(visible_height);
    }

    /// Scroll down by a page.
    fn page_down(&mut self, visible_height: usize) {
        let max_offset = self.total_height.saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + visible_height).min(max_offset);
    }
}

impl Default for HelpCard {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// CARDVIEW IMPLEMENTATION
// ============================================================

impl CardView for HelpCard {
    fn title(&self) -> &str {
        "Help"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Content height + 1 for search bar (when active)
        let content_height = self.total_height as u16;
        let search_bar_height = 1;
        let total = content_height + search_bar_height;
        total.min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 10 {
            return;
        }

        // Split area: content area and search bar at bottom
        let search_bar_height = 1;
        let content_height = area.height.saturating_sub(search_bar_height);
        let content_area = Rect::new(area.x, area.y, area.width, content_height);
        let search_area = Rect::new(
            area.x,
            area.y + content_height,
            area.width,
            search_bar_height,
        );

        // Render content
        self.render_content(content_area, buf);

        // Render search bar
        self.render_search_bar(search_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> CardResult {
        // Assume a reasonable visible height for scrolling
        let visible_height = 20usize;

        match key.code {
            // Close card
            KeyCode::Esc => {
                if self.search_active && !self.search_query.is_empty() {
                    // Clear search first
                    self.search_query.clear();
                    self.search_active = false;
                    self.rebuild_filtered();
                    CardResult::Continue
                } else if self.search_active {
                    // Exit search mode
                    self.search_active = false;
                    CardResult::Continue
                } else {
                    CardResult::Close
                }
            }

            // Focus search
            KeyCode::Char('/') if !self.search_active => {
                self.search_active = true;
                CardResult::Continue
            }

            // Navigation
            KeyCode::Up | KeyCode::Char('k') if !self.search_active => {
                self.scroll_up();
                CardResult::Continue
            }
            KeyCode::Down | KeyCode::Char('j') if !self.search_active => {
                self.scroll_down(visible_height);
                CardResult::Continue
            }
            KeyCode::PageUp => {
                self.page_up(visible_height);
                CardResult::Continue
            }
            KeyCode::PageDown => {
                self.page_down(visible_height);
                CardResult::Continue
            }

            // Search input when search is active
            KeyCode::Char(c) if self.search_active => {
                self.search_query.push(c);
                self.rebuild_filtered();
                CardResult::Continue
            }
            KeyCode::Backspace if self.search_active => {
                self.search_query.pop();
                self.rebuild_filtered();
                CardResult::Continue
            }

            // Clear search with Ctrl+U
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_query.clear();
                self.rebuild_filtered();
                CardResult::Continue
            }

            // Enter to close when not in search
            KeyCode::Enter if !self.search_active => CardResult::Close,

            _ => CardResult::Continue,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.search_active {
            vec![
                ("Type", "search"),
                ("Esc", "clear/exit"),
                ("Enter", "close"),
            ]
        } else {
            vec![
                ("\u{2191}\u{2193}", "scroll"),
                ("/", "search"),
                ("Esc", "close"),
            ]
        }
    }

    fn on_cancel(&mut self) -> CancellationEvent {
        if self.search_active {
            if !self.search_query.is_empty() {
                self.search_query.clear();
                self.rebuild_filtered();
            } else {
                self.search_active = false;
            }
            CancellationEvent::Handled
        } else {
            CancellationEvent::NotHandled
        }
    }

    fn is_complete(&self) -> bool {
        false
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Search help...")
    }
}

impl HelpCard {
    /// Render the help content.
    fn render_content(&self, area: Rect, buf: &mut Buffer) {
        if self.filtered_items.is_empty() {
            // Show "No matches" message
            let msg = "No matches";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
            return;
        }

        // Build lines to render
        let mut lines: Vec<ContentLine> = Vec::new();
        let mut current_section: Option<usize> = None;

        for &(section_idx, item_idx) in &self.filtered_items {
            // Add section header if new section
            if current_section != Some(section_idx) {
                if current_section.is_some() {
                    lines.push(ContentLine::Blank);
                }
                if let Some(section) = self.sections.get(section_idx) {
                    lines.push(ContentLine::Header(section.title.clone()));
                }
                current_section = Some(section_idx);
            }

            // Add item
            if let Some(section) = self.sections.get(section_idx)
                && let Some(item) = section.items.get(item_idx)
            {
                lines.push(ContentLine::Item {
                    key: item.key.clone(),
                    description: item.description.clone(),
                });
            }
        }

        // Render visible lines
        let visible_height = area.height as usize;
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
                    // Key in primary color, description in text
                    let key_width = 14u16; // Fixed width for alignment
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

    /// Render the search bar.
    fn render_search_bar(&self, area: Rect, buf: &mut Buffer) {
        // Background
        for x in area.x..area.right() {
            buf[(x, area.y)].set_bg(SURFACE_1);
        }

        let x = area.x + 1;

        // Search icon
        let icon_style = if self.search_active {
            Style::default().fg(CYAN_PRIMARY).bg(SURFACE_1)
        } else {
            Style::default().fg(TEXT_DIM).bg(SURFACE_1)
        };
        buf.set_string(x, area.y, "/", icon_style);

        // Search query or placeholder
        let display_text = if self.search_query.is_empty() {
            "type to search...".to_string()
        } else {
            self.search_query.clone()
        };

        let text_style = if self.search_query.is_empty() {
            Style::default().fg(TEXT_MUTED).bg(SURFACE_1)
        } else {
            Style::default().fg(TEXT).bg(SURFACE_1)
        };

        buf.set_string(x + 2, area.y, &display_text, text_style);

        // Cursor when search is active
        if self.search_active {
            let cursor_x = x + 2 + self.search_query.len() as u16;
            if cursor_x < area.right().saturating_sub(1) {
                buf[(cursor_x, area.y)].set_bg(CYAN_PRIMARY);
                buf[(cursor_x, area.y)].set_fg(SURFACE_0);
            }
        }

        // Result count on the right
        let total_items: usize = self.sections.iter().map(|s| s.items.len()).sum();
        let count_str = format!("{}/{}", self.filtered_items.len(), total_items);
        let count_x = area.right().saturating_sub(count_str.len() as u16 + 1);
        if count_x > x + display_text.len() as u16 + 4 {
            buf.set_string(
                count_x,
                area.y,
                &count_str,
                Style::default().fg(TEXT_DIM).bg(SURFACE_1),
            );
        }
    }
}

/// Internal enum for content lines.
enum ContentLine {
    Blank,
    Header(String),
    Item { key: String, description: String },
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_default_sections() {
        let card = HelpCard::new();
        assert_eq!(card.sections.len(), 3);
        assert_eq!(card.sections[0].title, "Keyboard Shortcuts");
        assert_eq!(card.sections[1].title, "Slash Commands");
        assert_eq!(card.sections[2].title, "Navigation");
    }

    #[test]
    fn test_with_sections() {
        let sections = vec![HelpSection::new(
            "Custom",
            vec![HelpItem::new("Test", "A test item")],
        )];
        let card = HelpCard::with_sections(sections);
        assert_eq!(card.sections.len(), 1);
        assert_eq!(card.sections[0].title, "Custom");
    }

    #[test]
    fn test_title() {
        let card = HelpCard::new();
        assert_eq!(card.title(), "Help");
    }

    #[test]
    fn test_is_searchable() {
        let card = HelpCard::new();
        assert!(card.is_searchable());
    }

    #[test]
    fn test_search_filtering() {
        let mut card = HelpCard::new();
        let initial_count = card.filtered_items.len();

        // Search for "model"
        card.search_query = "model".to_string();
        card.rebuild_filtered();

        // Should have fewer items
        assert!(card.filtered_items.len() < initial_count);
        assert!(!card.filtered_items.is_empty());
    }

    #[test]
    fn test_search_no_matches() {
        let mut card = HelpCard::new();
        card.search_query = "xyznonexistent".to_string();
        card.rebuild_filtered();
        assert_eq!(card.filtered_items.len(), 0);
    }

    #[test]
    fn test_scroll_bounds() {
        let mut card = HelpCard::new();
        card.total_height = 100;
        card.scroll_offset = 0;

        // Scroll up at top should stay at 0
        card.scroll_up();
        assert_eq!(card.scroll_offset, 0);

        // Scroll down
        card.scroll_down(20);
        assert_eq!(card.scroll_offset, 1);

        // Page down
        card.page_down(20);
        assert!(card.scroll_offset > 1);
    }

    #[test]
    fn test_escape_clears_search_first() {
        let mut card = HelpCard::new();
        card.search_active = true;
        card.search_query = "test".to_string();

        let result = card.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // Should clear search, not close
        assert!(matches!(result, CardResult::Continue));
        assert!(card.search_query.is_empty());
    }

    #[test]
    fn test_escape_closes_when_no_search() {
        let mut card = HelpCard::new();

        let result = card.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        assert!(matches!(result, CardResult::Close));
    }

    #[test]
    fn test_slash_activates_search() {
        let mut card = HelpCard::new();
        assert!(!card.search_active);

        card.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));

        assert!(card.search_active);
    }

    #[test]
    fn test_typing_in_search() {
        let mut card = HelpCard::new();
        card.search_active = true;

        card.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));
        card.handle_key(KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE));
        card.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        card.handle_key(KeyEvent::new(KeyCode::Char('t'), KeyModifiers::NONE));

        assert_eq!(card.search_query, "test");
    }

    #[test]
    fn test_backspace_in_search() {
        let mut card = HelpCard::new();
        card.search_active = true;
        card.search_query = "test".to_string();

        card.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));

        assert_eq!(card.search_query, "tes");
    }

    #[test]
    fn test_key_hints_change_with_search() {
        let mut card = HelpCard::new();

        let hints_normal = card.key_hints();
        assert!(hints_normal.iter().any(|(k, _)| *k == "/"));

        card.search_active = true;
        let hints_search = card.key_hints();
        assert!(hints_search.iter().any(|(k, _)| *k == "Type"));
    }
}
