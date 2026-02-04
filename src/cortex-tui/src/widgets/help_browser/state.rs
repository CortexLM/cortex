//! Help browser state management.

use unicode_segmentation::UnicodeSegmentation;

use super::content::{HelpSection, get_help_sections};

// ============================================================
// HELP FOCUS STATE
// ============================================================

/// Focus state for the help browser.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpFocus {
    /// Sidebar is focused
    #[default]
    Sidebar,
    /// Content pane is focused
    Content,
    /// Search input is focused
    Search,
}

// ============================================================
// HELP BROWSER STATE
// ============================================================

/// State for the help browser widget.
#[derive(Debug, Clone)]
pub struct HelpBrowserState {
    /// Available help sections
    pub sections: Vec<HelpSection>,
    /// Currently selected section index
    pub selected_section: usize,
    /// Content scroll offset
    pub content_scroll: usize,
    /// Search query string
    pub search_query: String,
    /// Whether search mode is active
    pub search_mode: bool,
    /// Current focus state
    pub focus: HelpFocus,
}

impl Default for HelpBrowserState {
    fn default() -> Self {
        Self::new()
    }
}

impl HelpBrowserState {
    /// Creates a new help browser state with default sections.
    pub fn new() -> Self {
        Self {
            sections: get_help_sections(),
            selected_section: 0,
            content_scroll: 0,
            search_query: String::new(),
            search_mode: false,
            focus: HelpFocus::Sidebar,
        }
    }

    /// Opens to a specific topic.
    ///
    /// # Arguments
    /// * `topic` - Optional topic id to navigate to
    pub fn with_topic(mut self, topic: Option<&str>) -> Self {
        if let Some(topic) = topic
            && let Some(idx) = self.sections.iter().position(|s| s.id == topic)
        {
            self.selected_section = idx;
        }
        self
    }

    /// Selects the previous section with wrap-around.
    pub fn select_prev(&mut self) {
        if self.sections.is_empty() {
            return;
        }
        if self.selected_section > 0 {
            self.selected_section -= 1;
        } else {
            // Wrap to last item
            self.selected_section = self.sections.len() - 1;
        }
        self.content_scroll = 0;
    }

    /// Selects the next section with wrap-around.
    pub fn select_next(&mut self) {
        if self.sections.is_empty() {
            return;
        }
        if self.selected_section < self.sections.len().saturating_sub(1) {
            self.selected_section += 1;
        } else {
            // Wrap to first item
            self.selected_section = 0;
        }
        self.content_scroll = 0;
    }

    /// Scrolls content up by one line.
    pub fn scroll_up(&mut self) {
        self.content_scroll = self.content_scroll.saturating_sub(1);
    }

    /// Scrolls content down by one line.
    pub fn scroll_down(&mut self) {
        self.content_scroll = self.content_scroll.saturating_add(1);
    }

    /// Scrolls content up by a page.
    ///
    /// # Arguments
    /// * `page_size` - Number of lines in a page
    pub fn page_up(&mut self, page_size: usize) {
        self.content_scroll = self.content_scroll.saturating_sub(page_size);
    }

    /// Scrolls content down by a page.
    ///
    /// # Arguments
    /// * `page_size` - Number of lines in a page
    pub fn page_down(&mut self, page_size: usize) {
        self.content_scroll = self.content_scroll.saturating_add(page_size);
    }

    /// Toggles focus between sidebar and content.
    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            HelpFocus::Sidebar => HelpFocus::Content,
            HelpFocus::Content => HelpFocus::Sidebar,
            HelpFocus::Search => HelpFocus::Sidebar,
        };
    }

    /// Toggles search mode.
    pub fn toggle_search(&mut self) {
        self.search_mode = !self.search_mode;
        if self.search_mode {
            self.focus = HelpFocus::Search;
        } else {
            self.focus = HelpFocus::Sidebar;
            self.search_query.clear();
        }
    }

    /// Returns the currently selected section.
    ///
    /// Returns `None` if the sections vector is empty.
    pub fn current_section(&self) -> Option<&HelpSection> {
        if self.sections.is_empty() {
            return None;
        }
        self.sections.get(self.selected_section)
    }

    /// Handles character input for search.
    ///
    /// # Arguments
    /// * `c` - Character to add to search query
    pub fn search_input(&mut self, c: char) {
        if self.search_mode {
            self.search_query.push(c);
        }
    }

    /// Handles backspace for search.
    /// Uses grapheme-aware deletion for proper Unicode/emoji support.
    pub fn search_backspace(&mut self) {
        if self.search_mode {
            // Pop the last grapheme cluster instead of last char
            let graphemes: Vec<&str> = self.search_query.graphemes(true).collect();
            if !graphemes.is_empty() {
                self.search_query = graphemes[..graphemes.len() - 1].concat();
            }
        }
    }
}
