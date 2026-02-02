use super::types::AutocompleteTrigger;

/// An item in the autocomplete list
#[derive(Debug, Clone)]
pub struct AutocompleteItem {
    pub value: String,
    pub label: String,
    pub description: String,
    pub icon: char,
    pub category: String,
}

impl AutocompleteItem {
    pub fn new(
        value: impl Into<String>,
        label: impl Into<String>,
        description: impl Into<String>,
    ) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
            description: description.into(),
            icon: ' ',
            category: String::new(),
        }
    }

    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon = icon;
        self
    }

    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = category.into();
        self
    }
}

/// State for the autocomplete popup
#[derive(Debug, Clone, Default)]
pub struct AutocompleteState {
    pub visible: bool,
    pub trigger: Option<AutocompleteTrigger>,
    pub query: String,
    pub trigger_position: usize,
    pub items: Vec<AutocompleteItem>,
    pub selected: usize,
    pub max_visible: usize,
    pub scroll_offset: usize,
}

impl AutocompleteState {
    pub fn new() -> Self {
        Self {
            visible: false,
            trigger: None,
            query: String::new(),
            trigger_position: 0,
            items: Vec::new(),
            selected: 0,
            max_visible: 10,
            scroll_offset: 0,
        }
    }

    /// Select the previous item in the list
    pub fn select_prev(&mut self) {
        if self.items.is_empty() {
            return;
        }
        if self.selected == 0 {
            // Wrap to end
            self.selected = self.items.len() - 1;
            // Scroll to show the last items
            if self.items.len() > self.max_visible {
                self.scroll_offset = self.items.len() - self.max_visible;
            }
        } else {
            self.selected -= 1;
            // Adjust scroll offset to keep selected item visible
            if self.selected < self.scroll_offset {
                self.scroll_offset = self.selected;
            }
        }
    }

    /// Select the next item in the list
    pub fn select_next(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = (self.selected + 1) % self.items.len();
        // Adjust scroll offset to keep selected item visible
        if self.selected == 0 {
            // Wrapped around to start
            self.scroll_offset = 0;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected - self.max_visible + 1;
        }
    }

    /// Get the completion text for the currently selected item
    pub fn completion_text(&self) -> Option<&str> {
        self.items
            .get(self.selected)
            .map(|item| item.value.as_str())
    }

    /// Hide the autocomplete popup and reset state
    pub fn hide(&mut self) {
        self.visible = false;
        self.items.clear();
        self.selected = 0;
        self.scroll_offset = 0;
        self.query.clear();
        self.trigger = None;
    }

    /// Check if there are any items in the list
    pub fn has_items(&self) -> bool {
        !self.items.is_empty()
    }

    /// Get the currently visible items based on scroll offset
    pub fn visible_items(&self) -> &[AutocompleteItem] {
        let start = self.scroll_offset;
        let end = (start + self.max_visible).min(self.items.len());
        &self.items[start..end]
    }

    /// Show the autocomplete popup with a trigger
    pub fn show(&mut self, trigger: AutocompleteTrigger, position: usize) {
        self.visible = true;
        self.trigger = Some(trigger);
        self.trigger_position = position;
        self.query.clear();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Set the filter query
    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Set the list of items
    pub fn set_items(&mut self, items: Vec<AutocompleteItem>) {
        self.items = items;
        self.selected = 0;
        self.scroll_offset = 0;
    }
}
