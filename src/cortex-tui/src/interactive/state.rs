//! Interactive input state management.
//!
//! Defines the state for interactive command selection within the input area.

use std::path::PathBuf;

/// Input mode - either normal text input or interactive selection.
#[derive(Debug, Clone, Default)]
pub enum InputMode {
    /// Normal text input mode.
    #[default]
    Normal,
    /// Interactive selection mode.
    Interactive(Box<InteractiveState>),
}

impl InputMode {
    /// Check if in interactive mode.
    pub fn is_interactive(&self) -> bool {
        matches!(self, InputMode::Interactive(_))
    }

    /// Get interactive state if in interactive mode.
    pub fn interactive(&self) -> Option<&InteractiveState> {
        match self {
            InputMode::Interactive(state) => Some(state),
            _ => None,
        }
    }

    /// Get mutable interactive state if in interactive mode.
    pub fn interactive_mut(&mut self) -> Option<&mut InteractiveState> {
        match self {
            InputMode::Interactive(state) => Some(state),
            _ => None,
        }
    }
}

/// A form field for inline configuration.
#[derive(Debug, Clone)]
pub struct InlineFormField {
    /// Field name/key.
    pub name: String,
    /// Display label.
    pub label: String,
    /// Current value.
    pub value: String,
    /// Placeholder text.
    pub placeholder: String,
    /// Whether this field is required.
    pub required: bool,
}

impl InlineFormField {
    /// Create a new form field.
    pub fn new(name: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            label: label.into(),
            value: String::new(),
            placeholder: String::new(),
            required: false,
        }
    }

    /// Set placeholder text.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    /// Mark as required.
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
}

/// Inline form state for configuration within the interactive panel.
#[derive(Debug, Clone)]
pub struct InlineFormState {
    /// Form title.
    pub title: String,
    /// Form fields.
    pub fields: Vec<InlineFormField>,
    /// Currently focused field index.
    pub focused_field: usize,
    /// Action identifier (e.g., "mcp-add").
    pub action_id: String,
}

impl InlineFormState {
    /// Create a new inline form.
    pub fn new(title: impl Into<String>, action_id: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            fields: Vec::new(),
            focused_field: 0,
            action_id: action_id.into(),
        }
    }

    /// Add a field to the form.
    pub fn with_field(mut self, field: InlineFormField) -> Self {
        self.fields.push(field);
        self
    }

    /// Get the currently focused field.
    pub fn focused(&self) -> Option<&InlineFormField> {
        self.fields.get(self.focused_field)
    }

    /// Get the currently focused field mutably.
    pub fn focused_mut(&mut self) -> Option<&mut InlineFormField> {
        self.fields.get_mut(self.focused_field)
    }

    /// Move focus to the next field.
    pub fn focus_next(&mut self) {
        if !self.fields.is_empty() {
            self.focused_field = (self.focused_field + 1) % self.fields.len();
        }
    }

    /// Move focus to the previous field.
    pub fn focus_prev(&mut self) {
        if !self.fields.is_empty() {
            if self.focused_field == 0 {
                self.focused_field = self.fields.len() - 1;
            } else {
                self.focused_field -= 1;
            }
        }
    }

    /// Check if the form is valid (all required fields filled).
    pub fn is_valid(&self) -> bool {
        self.fields
            .iter()
            .all(|f| !f.required || !f.value.is_empty())
    }

    /// Get field value by name.
    pub fn get_value(&self, name: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.value.as_str())
    }
}

/// A tab/page for navigation.
#[derive(Debug, Clone)]
pub struct NavTab {
    /// Tab identifier.
    pub id: String,
    /// Tab label.
    pub label: String,
}

impl NavTab {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// State for interactive selection mode.
#[derive(Debug, Clone)]
pub struct InteractiveState {
    /// Title shown above the list.
    pub title: String,
    /// Items to select from.
    pub items: Vec<InteractiveItem>,
    /// Currently selected index.
    pub selected: usize,
    /// Action to perform when an item is selected.
    pub action: InteractiveAction,
    /// Whether search/filtering is enabled.
    pub searchable: bool,
    /// Current search query.
    pub search_query: String,
    /// Filtered item indices (if searching).
    pub filtered_indices: Vec<usize>,
    /// Whether multi-selection is enabled.
    pub multi_select: bool,
    /// Indices of checked items (for multi-select).
    pub checked: Vec<usize>,
    /// Maximum visible items.
    pub max_visible: usize,
    /// Scroll offset for long lists.
    pub scroll_offset: usize,
    /// Custom key hints (key, label) pairs.
    pub hints: Option<Vec<(String, String)>>,
    /// Inline form state (for configuration within the panel).
    pub inline_form: Option<InlineFormState>,
    /// Currently hovered item index (from mouse).
    pub hovered: Option<usize>,
    /// Stored click zones for hit testing (calculated during render).
    pub click_zones: Vec<(ratatui::layout::Rect, usize)>,
    /// Navigation tabs (optional).
    pub tabs: Vec<NavTab>,
    /// Currently active tab index.
    pub active_tab: usize,
    /// Currently hovered tab index (from mouse).
    pub hovered_tab: Option<usize>,
    /// Click zones for tabs (calculated during render).
    pub tab_click_zones: Vec<(ratatui::layout::Rect, usize)>,
}

impl InteractiveState {
    /// Create a new interactive state.
    pub fn new(
        title: impl Into<String>,
        items: Vec<InteractiveItem>,
        action: InteractiveAction,
    ) -> Self {
        let filtered_indices: Vec<usize> = (0..items.len()).collect();
        Self {
            title: title.into(),
            items,
            selected: 0,
            action,
            searchable: false,
            search_query: String::new(),
            filtered_indices,
            multi_select: false,
            checked: Vec::new(),
            max_visible: 10,
            scroll_offset: 0,
            hints: None,
            inline_form: None,
            hovered: None,
            click_zones: Vec::new(),
            tabs: Vec::new(),
            active_tab: 0,
            hovered_tab: None,
            tab_click_zones: Vec::new(),
        }
    }

    /// Set navigation tabs.
    pub fn with_tabs(mut self, tabs: Vec<NavTab>) -> Self {
        self.tabs = tabs;
        self
    }

    /// Switch to next tab.
    pub fn next_tab(&mut self) {
        if !self.tabs.is_empty() {
            self.active_tab = (self.active_tab + 1) % self.tabs.len();
            self.selected = 0;
            self.scroll_offset = 0;
        }
    }

    /// Switch to previous tab.
    pub fn prev_tab(&mut self) {
        if !self.tabs.is_empty() {
            if self.active_tab == 0 {
                self.active_tab = self.tabs.len() - 1;
            } else {
                self.active_tab -= 1;
            }
            self.selected = 0;
            self.scroll_offset = 0;
        }
    }

    /// Get active tab id.
    pub fn active_tab_id(&self) -> Option<&str> {
        self.tabs.get(self.active_tab).map(|t| t.id.as_str())
    }

    /// Check if inline form is active.
    pub fn is_form_active(&self) -> bool {
        self.inline_form.is_some()
    }

    /// Get the inline form state.
    pub fn form(&self) -> Option<&InlineFormState> {
        self.inline_form.as_ref()
    }

    /// Get the inline form state mutably.
    pub fn form_mut(&mut self) -> Option<&mut InlineFormState> {
        self.inline_form.as_mut()
    }

    /// Open an inline form.
    pub fn open_form(&mut self, form: InlineFormState) {
        self.inline_form = Some(form);
    }

    /// Close the inline form and return to list mode.
    pub fn close_form(&mut self) {
        self.inline_form = None;
    }

    /// Enable search/filtering.
    pub fn with_search(mut self) -> Self {
        self.searchable = true;
        self
    }

    /// Enable multi-selection.
    pub fn with_multi_select(mut self) -> Self {
        self.multi_select = true;
        self
    }

    /// Set max visible items.
    pub fn with_max_visible(mut self, max: usize) -> Self {
        self.max_visible = max;
        self
    }

    /// Set custom key hints.
    pub fn with_hints(mut self, hints: Vec<(String, String)>) -> Self {
        self.hints = Some(hints);
        self
    }

    /// Get visible items based on current filter.
    pub fn visible_items(&self) -> Vec<(usize, &InteractiveItem)> {
        self.filtered_indices
            .iter()
            .filter_map(|&idx| self.items.get(idx).map(|item| (idx, item)))
            .collect()
    }

    /// Get currently selected item.
    pub fn selected_item(&self) -> Option<&InteractiveItem> {
        self.filtered_indices
            .get(self.selected)
            .and_then(|&idx| self.items.get(idx))
    }

    /// Get the real index of the selected item.
    pub fn selected_real_index(&self) -> Option<usize> {
        self.filtered_indices.get(self.selected).copied()
    }

    /// Move selection up, skipping separators and disabled items.
    pub fn select_prev(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let len = self.filtered_indices.len();
        let start = self.selected;
        loop {
            if self.selected == 0 {
                self.selected = len - 1;
            } else {
                self.selected -= 1;
            }
            // Stop if we've wrapped around completely
            if self.selected == start {
                break;
            }
            // Stop if we found a selectable item
            if let Some(item) = self.selected_item()
                && !item.is_separator
                && !item.disabled
            {
                break;
            }
        }
        self.ensure_visible();
    }

    /// Move selection down, skipping separators and disabled items.
    pub fn select_next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let len = self.filtered_indices.len();
        let start = self.selected;
        loop {
            self.selected = (self.selected + 1) % len;
            // Stop if we've wrapped around completely
            if self.selected == start {
                break;
            }
            // Stop if we found a selectable item
            if let Some(item) = self.selected_item()
                && !item.is_separator
                && !item.disabled
            {
                break;
            }
        }
        self.ensure_visible();
    }

    /// Ensure selected item is visible (adjust scroll).
    fn ensure_visible(&mut self) {
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + self.max_visible {
            self.scroll_offset = self.selected - self.max_visible + 1;
        }
    }

    /// Toggle check state for current item (multi-select).
    pub fn toggle_check(&mut self) {
        if !self.multi_select {
            return;
        }
        if let Some(real_idx) = self.selected_real_index() {
            if let Some(pos) = self.checked.iter().position(|&x| x == real_idx) {
                self.checked.remove(pos);
            } else {
                self.checked.push(real_idx);
            }
        }
    }

    /// Check if an item is checked.
    pub fn is_checked(&self, idx: usize) -> bool {
        self.checked.contains(&idx)
    }

    /// Update search query and filter items.
    pub fn update_search(&mut self, query: &str) {
        self.search_query = query.to_string();
        let query_lower = query.to_lowercase();

        if query.is_empty() {
            self.filtered_indices = (0..self.items.len()).collect();
        } else {
            self.filtered_indices = self
                .items
                .iter()
                .enumerate()
                .filter(|(_, item)| {
                    item.label.to_lowercase().contains(&query_lower)
                        || item
                            .description
                            .as_ref()
                            .map(|d| d.to_lowercase().contains(&query_lower))
                            .unwrap_or(false)
                })
                .map(|(idx, _)| idx)
                .collect();
        }

        // Reset selection if out of bounds
        if self.selected >= self.filtered_indices.len() {
            self.selected = 0;
        }
        self.scroll_offset = 0;
    }

    /// Add a character to search query.
    pub fn push_search_char(&mut self, c: char) {
        let mut query = self.search_query.clone();
        query.push(c);
        self.update_search(&query);
    }

    /// Remove last character from search query.
    pub fn pop_search_char(&mut self) {
        let mut query = self.search_query.clone();
        query.pop();
        self.update_search(&query);
    }

    /// Get checked items.
    pub fn checked_items(&self) -> Vec<&InteractiveItem> {
        self.checked
            .iter()
            .filter_map(|&idx| self.items.get(idx))
            .collect()
    }

    /// Set hover state from mouse position.
    /// Returns true if the hover state changed.
    pub fn set_hover(&mut self, filtered_idx: Option<usize>) -> bool {
        let old_hover = self.hovered;
        self.hovered = filtered_idx;
        old_hover != self.hovered
    }

    /// Clear hover state.
    pub fn clear_hover(&mut self) {
        self.hovered = None;
    }

    /// Hit test a screen position against registered click zones.
    /// Returns the filtered index of the item at that position.
    pub fn hit_test(&self, x: u16, y: u16) -> Option<usize> {
        for (rect, filtered_idx) in &self.click_zones {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                // Check if this item is selectable
                if let Some(item) = self
                    .filtered_indices
                    .get(*filtered_idx)
                    .and_then(|&real_idx| self.items.get(real_idx))
                    && !item.is_separator
                    && !item.disabled
                {
                    return Some(*filtered_idx);
                }
            }
        }
        None
    }

    /// Get the hovered item if any.
    pub fn hovered_item(&self) -> Option<&InteractiveItem> {
        self.hovered
            .and_then(|h| self.filtered_indices.get(h))
            .and_then(|&idx| self.items.get(idx))
    }

    /// Select hovered item (for mouse click).
    /// Returns true if selection changed.
    pub fn select_hovered(&mut self) -> bool {
        if let Some(h) = self.hovered
            && h != self.selected
        {
            self.selected = h;
            self.ensure_visible();
            return true;
        }
        false
    }

    /// Hit test a screen position against tab click zones.
    /// Returns the tab index at that position.
    pub fn hit_test_tab(&self, x: u16, y: u16) -> Option<usize> {
        for (rect, tab_idx) in &self.tab_click_zones {
            if x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height {
                return Some(*tab_idx);
            }
        }
        None
    }

    /// Set hover state for tabs.
    /// Returns true if the hover state changed.
    pub fn set_tab_hover(&mut self, tab_idx: Option<usize>) -> bool {
        let old_hover = self.hovered_tab;
        self.hovered_tab = tab_idx;
        old_hover != self.hovered_tab
    }
}

/// A single item in the interactive list.
#[derive(Debug, Clone)]
pub struct InteractiveItem {
    /// Unique identifier for the item.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional description shown below/beside the label.
    pub description: Option<String>,
    /// Optional keyboard shortcut.
    pub shortcut: Option<char>,
    /// Optional icon character.
    pub icon: Option<char>,
    /// Whether the item is disabled.
    pub disabled: bool,
    /// Whether this is the current/active item.
    pub is_current: bool,
    /// Optional path (for file items).
    pub path: Option<PathBuf>,
    /// Optional metadata.
    pub metadata: Option<String>,
    /// Whether this is a separator (non-selectable).
    pub is_separator: bool,
}

impl InteractiveItem {
    /// Create a new interactive item.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            label: label.into(),
            id,
            description: None,
            shortcut: None,
            icon: None,
            disabled: false,
            is_current: false,
            path: None,
            metadata: None,
            is_separator: false,
        }
    }

    /// Set description.
    pub fn with_description(mut self, desc: impl Into<String>) -> Self {
        self.description = Some(desc.into());
        self
    }

    /// Set keyboard shortcut.
    pub fn with_shortcut(mut self, shortcut: char) -> Self {
        self.shortcut = Some(shortcut);
        self
    }

    /// Set icon.
    pub fn with_icon(mut self, icon: char) -> Self {
        self.icon = Some(icon);
        self
    }

    /// Mark as disabled.
    pub fn with_disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Mark as current.
    pub fn with_current(mut self, current: bool) -> Self {
        self.is_current = current;
        self
    }

    /// Set path.
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    /// Set metadata.
    pub fn with_metadata(mut self, metadata: impl Into<String>) -> Self {
        self.metadata = Some(metadata.into());
        self
    }

    /// Mark as separator (non-selectable visual divider).
    pub fn as_separator(mut self) -> Self {
        self.is_separator = true;
        self.disabled = true;
        self
    }
}

/// Action to perform when an interactive selection is made.
#[derive(Debug, Clone)]
pub enum InteractiveAction {
    /// Set the provider.
    SetProvider,
    /// Set the model.
    SetModel,
    /// Set approval mode.
    SetApprovalMode,
    /// Add files to context.
    AddContextFiles,
    /// Remove files from context.
    RemoveContextFiles,
    /// Select/resume a session.
    SelectSession,
    /// Resume session from picker (startup).
    ResumeSession,
    /// Fork a session (create copy).
    ForkSession,
    /// MCP server action (add/remove/toggle).
    McpServerAction,
    /// Set log level.
    SetLogLevel,
    /// Browse and select files.
    BrowseFiles { base_path: PathBuf },
    /// Toggle or set a setting.
    ToggleSetting,
    /// Device login flow action.
    DeviceLogin,
    /// Already logged in confirmation action.
    AlreadyLoggedIn,
    /// Account action (logout, close, etc.).
    AccountAction,
    /// Billing action (refresh, manage, close, etc.).
    BillingAction,
    /// Custom action with identifier.
    Custom(String),
}

/// Result of an interactive selection.
#[derive(Debug, Clone)]
pub enum InteractiveResult {
    /// User selected an item.
    Selected {
        action: InteractiveAction,
        item_id: String,
        item_ids: Vec<String>, // For multi-select
    },
    /// User submitted an inline form.
    FormSubmitted {
        action_id: String,
        values: std::collections::HashMap<String, String>,
    },
    /// User cancelled.
    Cancelled,
    /// Continue (no action yet).
    Continue,
    /// Switch tab (direction: -1 for prev, 1 for next).
    SwitchTab { direction: i32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interactive_state_navigation() {
        let items = vec![
            InteractiveItem::new("1", "Item 1"),
            InteractiveItem::new("2", "Item 2"),
            InteractiveItem::new("3", "Item 3"),
        ];
        let mut state =
            InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()));

        assert_eq!(state.selected, 0);
        state.select_next();
        assert_eq!(state.selected, 1);
        state.select_next();
        assert_eq!(state.selected, 2);
        state.select_next();
        assert_eq!(state.selected, 0); // Wrap around
        state.select_prev();
        assert_eq!(state.selected, 2); // Wrap around backwards
    }

    #[test]
    fn test_interactive_search() {
        let items = vec![
            InteractiveItem::new("1", "Apple"),
            InteractiveItem::new("2", "Banana"),
            InteractiveItem::new("3", "Cherry"),
        ];
        let mut state =
            InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()))
                .with_search();

        state.update_search("an");
        assert_eq!(state.filtered_indices.len(), 1);
        assert_eq!(state.filtered_indices[0], 1); // Banana
    }

    #[test]
    fn test_multi_select() {
        let items = vec![
            InteractiveItem::new("1", "Item 1"),
            InteractiveItem::new("2", "Item 2"),
            InteractiveItem::new("3", "Item 3"),
        ];
        let mut state =
            InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()))
                .with_multi_select();

        state.toggle_check();
        assert!(state.is_checked(0));
        state.select_next();
        state.toggle_check();
        assert!(state.is_checked(1));
        assert_eq!(state.checked.len(), 2);
        state.toggle_check();
        assert!(!state.is_checked(1));
        assert_eq!(state.checked.len(), 1);
    }

    #[test]
    fn test_navigation_skips_separators() {
        let items = vec![
            InteractiveItem::new("1", "Item 1"),
            InteractiveItem::new("sep", "─────").as_separator(),
            InteractiveItem::new("2", "Item 2"),
            InteractiveItem::new("disabled", "Disabled").with_disabled(true),
            InteractiveItem::new("3", "Item 3"),
        ];
        let mut state =
            InteractiveState::new("Test", items, InteractiveAction::Custom("test".into()));

        // Should start at 0 (Item 1)
        assert_eq!(state.selected, 0);

        // select_next should skip separator (1) and go to Item 2 (2)
        state.select_next();
        assert_eq!(state.selected, 2); // Skipped separator at index 1

        // select_next should skip disabled item (3) and go to Item 3 (4)
        state.select_next();
        assert_eq!(state.selected, 4); // Skipped disabled at index 3

        // select_next should wrap around and go back to Item 1 (0)
        state.select_next();
        assert_eq!(state.selected, 0);

        // select_prev should wrap around to Item 3 (4)
        state.select_prev();
        assert_eq!(state.selected, 4);

        // select_prev should skip disabled (3) and go to Item 2 (2)
        state.select_prev();
        assert_eq!(state.selected, 2);
    }
}
