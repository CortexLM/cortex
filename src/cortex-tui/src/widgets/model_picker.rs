//! Model Picker Widget
//!
//! A floating modal for selecting AI models with search filtering.
//!
//! ## Usage
//!
//! ```ignore
//! use cortex_tui::widgets::{ModelPicker, ModelPickerState};
//!
//! let mut state = ModelPickerState::new();
//! state.load_models(&provider_manager);
//!
//! let widget = ModelPicker::new(&state);
//! frame.render_widget(widget, area);
//! ```

use crate::providers::models::{ModelInfo, get_models_for_provider, get_popular_models};
use cortex_core::style::{
    BORDER_FOCUS, CYAN_PRIMARY, GREEN, ORANGE, SURFACE_0, TEXT, TEXT_DIM, TEXT_MUTED, VOID,
};
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Clear, Scrollbar, ScrollbarOrientation, ScrollbarState, Widget,
};
use unicode_segmentation::UnicodeSegmentation;

// ============================================================
// MODEL ITEM
// ============================================================

/// A model item for display in the picker.
#[derive(Debug, Clone)]
pub struct ModelItem {
    /// Model ID (e.g., "anthropic/claude-opus-4-20250514")
    pub id: String,
    /// Display name (e.g., "Claude Opus 4")
    pub name: String,
    /// Provider name
    pub provider: String,
    /// Context window size
    pub context_window: Option<u32>,
    /// Whether this is the current model
    pub is_current: bool,
    /// Whether this is a popular/recommended model
    pub is_popular: bool,
}

impl ModelItem {
    /// Create from a ModelInfo.
    pub fn from_info(info: &ModelInfo, current_model: &str) -> Self {
        Self {
            id: info.id.clone(),
            name: info.name.clone(),
            provider: info.provider.clone(),
            context_window: Some(info.context_window),
            is_current: info.id == current_model,
            is_popular: false,
        }
    }

    /// Mark as popular.
    pub fn with_popular(mut self, popular: bool) -> Self {
        self.is_popular = popular;
        self
    }
}

// ============================================================
// PICKER STATE
// ============================================================

/// State for the model picker widget.
#[derive(Debug, Clone)]
pub struct ModelPickerState {
    /// All available models
    pub all_models: Vec<ModelItem>,
    /// Filtered models (based on search)
    pub filtered_models: Vec<ModelItem>,
    /// Currently selected index in filtered list
    pub selected: usize,
    /// Search query
    pub search_query: String,
    /// Scroll offset
    pub scroll_offset: usize,
    /// Current provider
    pub current_provider: String,
    /// Current model ID
    pub current_model: String,
    /// Whether to show all providers or just current
    pub show_all_providers: bool,
}

impl Default for ModelPickerState {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelPickerState {
    /// Create a new picker state.
    pub fn new() -> Self {
        Self {
            all_models: Vec::new(),
            filtered_models: Vec::new(),
            selected: 0,
            search_query: String::new(),
            scroll_offset: 0,
            current_provider: String::new(),
            current_model: String::new(),
            show_all_providers: false,
        }
    }

    /// Load models for the current provider.
    pub fn load_models(&mut self, provider: &str, current_model: &str) {
        self.current_provider = provider.to_string();
        self.current_model = current_model.to_string();

        // Get popular models first
        let popular_models: Vec<_> = get_popular_models()
            .into_iter()
            .map(|m| ModelItem::from_info(&m, current_model).with_popular(true))
            .collect();

        // Get provider-specific models
        let provider_models: Vec<_> = get_models_for_provider(provider)
            .into_iter()
            .filter(|m| !popular_models.iter().any(|p| p.id == m.id))
            .map(|m| ModelItem::from_info(&m, current_model))
            .collect();

        // Combine: popular first, then provider-specific
        self.all_models = popular_models;
        self.all_models.extend(provider_models);

        // Apply filter
        self.apply_filter();

        // Select current model if present
        if let Some(idx) = self.filtered_models.iter().position(|m| m.is_current) {
            self.selected = idx;
            self.ensure_visible();
        }
    }

    /// Toggle showing all providers.
    pub fn toggle_all_providers(&mut self) {
        self.show_all_providers = !self.show_all_providers;
        // Reload models when toggling
        let provider = self.current_provider.clone();
        let model = self.current_model.clone();
        self.load_models(&provider, &model);
    }

    /// Apply search filter.
    fn apply_filter(&mut self) {
        if self.search_query.is_empty() {
            self.filtered_models = self.all_models.clone();
        } else {
            let query = self.search_query.to_lowercase();
            self.filtered_models = self
                .all_models
                .iter()
                .filter(|m| {
                    m.name.to_lowercase().contains(&query)
                        || m.id.to_lowercase().contains(&query)
                        || m.provider.to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
        }

        // Reset selection
        self.selected = 0;
        self.scroll_offset = 0;
    }

    /// Get the currently selected model.
    pub fn selected_model(&self) -> Option<&ModelItem> {
        self.filtered_models.get(self.selected)
    }

    /// Move selection up with wrap-around.
    pub fn select_prev(&mut self) {
        if self.filtered_models.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        } else {
            // Wrap to last item
            self.selected = self.filtered_models.len() - 1;
        }
        self.ensure_visible();
    }

    /// Move selection down with wrap-around.
    pub fn select_next(&mut self) {
        if self.filtered_models.is_empty() {
            return;
        }
        if self.selected + 1 < self.filtered_models.len() {
            self.selected += 1;
        } else {
            // Wrap to first item
            self.selected = 0;
        }
        self.ensure_visible();
    }

    /// Ensure selected item is visible.
    fn ensure_visible(&mut self) {
        let visible_height = 12; // Approximate visible items
        if self.selected < self.scroll_offset {
            self.scroll_offset = self.selected;
        } else if self.selected >= self.scroll_offset + visible_height {
            self.scroll_offset = self.selected.saturating_sub(visible_height - 1);
        }
    }

    /// Handle character input for search.
    pub fn handle_char(&mut self, c: char) {
        self.search_query.push(c);
        self.apply_filter();
    }

    /// Handle backspace for search.
    /// Uses grapheme-aware deletion for proper Unicode/emoji support.
    pub fn handle_backspace(&mut self) {
        // Pop the last grapheme cluster instead of last char
        let graphemes: Vec<&str> = self.search_query.graphemes(true).collect();
        if !graphemes.is_empty() {
            self.search_query = graphemes[..graphemes.len() - 1].concat();
        }
        self.apply_filter();
    }

    /// Clear search.
    pub fn clear_search(&mut self) {
        self.search_query.clear();
        self.apply_filter();
    }
}

// ============================================================
// MODEL PICKER WIDGET
// ============================================================

/// Model picker modal widget.
pub struct ModelPicker<'a> {
    state: &'a ModelPickerState,
}

impl<'a> ModelPicker<'a> {
    /// Create a new model picker widget.
    pub fn new(state: &'a ModelPickerState) -> Self {
        Self { state }
    }

    /// Calculate the modal area centered in the terminal.
    fn modal_area(&self, area: Rect) -> Rect {
        let width = 80.min(area.width.saturating_sub(4));
        let height = 24.min(area.height.saturating_sub(4));
        let x = (area.width.saturating_sub(width)) / 2;
        let y = (area.height.saturating_sub(height)) / 2;
        Rect::new(x, y, width, height)
    }
}

impl Widget for ModelPicker<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let modal_area = self.modal_area(area);

        // Clear background
        Clear.render(modal_area, buf);

        // Draw modal background
        for y in modal_area.y..modal_area.bottom() {
            for x in modal_area.x..modal_area.right() {
                buf[(x, y)].set_bg(SURFACE_0);
            }
        }

        // Draw border
        let title = format!(" Select Model ({}) ", self.state.current_provider);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(CYAN_PRIMARY).bold())
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_FOCUS))
            .style(Style::default().bg(SURFACE_0));

        let inner = block.inner(modal_area);
        block.render(modal_area, buf);

        if inner.height < 5 {
            return;
        }

        // Layout: search bar at top, list below
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Search bar
                Constraint::Min(1),    // Model list
                Constraint::Length(2), // Help bar
            ])
            .split(inner);

        self.render_search_bar(chunks[0], buf);
        self.render_model_list(chunks[1], buf);
        self.render_help_bar(chunks[2], buf);
    }
}

impl ModelPicker<'_> {
    /// Render the search bar.
    fn render_search_bar(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(BORDER_FOCUS));

        let search_inner = block.inner(area);
        block.render(area, buf);

        // Search icon and input
        let x = search_inner.x;
        let y = search_inner.y;

        buf.set_string(x, y, " > ", Style::default().fg(CYAN_PRIMARY));

        let display_query = if self.state.search_query.is_empty() {
            "Type to search models...".to_string()
        } else {
            self.state.search_query.clone()
        };

        let query_style = if self.state.search_query.is_empty() {
            Style::default().fg(TEXT_MUTED)
        } else {
            Style::default().fg(TEXT)
        };

        buf.set_string(x + 3, y, &display_query, query_style);

        // Cursor
        let cursor_x = x + 3 + self.state.search_query.len() as u16;
        if cursor_x < search_inner.right() {
            buf[(cursor_x, y)].set_bg(CYAN_PRIMARY);
        }

        // Result count
        let count = format!("{} models", self.state.filtered_models.len());
        let count_x = search_inner.right().saturating_sub(count.len() as u16 + 1);
        buf.set_string(count_x, y, &count, Style::default().fg(TEXT_DIM));
    }

    /// Render the model list.
    fn render_model_list(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let visible_height = area.height as usize;
        let start = self.state.scroll_offset;
        let end = (start + visible_height).min(self.state.filtered_models.len());

        // Column widths
        let name_width = (area.width as usize).saturating_sub(30).max(20);

        for (i, model) in self.state.filtered_models[start..end].iter().enumerate() {
            let y = area.y + i as u16;
            if y >= area.bottom() {
                break;
            }

            let is_selected = start + i == self.state.selected;

            // Selection highlight
            let (bg, fg) = if is_selected {
                (CYAN_PRIMARY, VOID)
            } else {
                (SURFACE_0, TEXT)
            };

            // Clear line
            for x in area.x..area.right() {
                buf[(x, y)].set_bg(bg);
            }

            let x = area.x + 1;

            // Status indicators
            let status = if model.is_current {
                "*"
            } else if model.is_popular {
                "+"
            } else {
                " "
            };
            let status_color = if model.is_current {
                GREEN
            } else if model.is_popular {
                ORANGE
            } else {
                TEXT_MUTED
            };
            buf.set_string(x, y, status, Style::default().fg(status_color).bg(bg));

            // Model name
            let name = if model.name.len() > name_width {
                format!("{}...", &model.name[..name_width.saturating_sub(3)])
            } else {
                model.name.clone()
            };
            buf.set_string(x + 2, y, &name, Style::default().fg(fg).bg(bg));

            // Provider
            let provider_x = x + name_width as u16 + 3;
            if provider_x < area.right().saturating_sub(15) {
                let provider_style = if is_selected {
                    Style::default().fg(VOID).bg(bg)
                } else {
                    Style::default().fg(TEXT_DIM).bg(bg)
                };
                buf.set_string(provider_x, y, &model.provider, provider_style);
            }

            // Context size
            if let Some(ctx) = model.context_window {
                let ctx_str = format!("{}k", ctx / 1000);
                let ctx_x = area.right().saturating_sub(ctx_str.len() as u16 + 2);
                if ctx_x > provider_x + model.provider.len() as u16 {
                    let ctx_style = if is_selected {
                        Style::default().fg(VOID).bg(bg)
                    } else {
                        Style::default().fg(TEXT_MUTED).bg(bg)
                    };
                    buf.set_string(ctx_x, y, &ctx_str, ctx_style);
                }
            }
        }

        // Scrollbar if needed
        if self.state.filtered_models.len() > visible_height {
            let mut scrollbar_state =
                ScrollbarState::new(self.state.filtered_models.len()).position(self.state.selected);

            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .track_symbol(Some("│"))
                .thumb_symbol("█")
                .render(area, buf, &mut scrollbar_state);
        }

        // Empty state
        if self.state.filtered_models.is_empty() {
            let msg = if self.state.search_query.is_empty() {
                "No models available"
            } else {
                "No models match your search"
            };
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Render the help bar.
    fn render_help_bar(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 1 {
            return;
        }

        let y = area.y;
        let x = area.x + 1;

        // Legend
        buf.set_string(x, y, "* current  ", Style::default().fg(GREEN));
        buf.set_string(x + 11, y, "+ popular", Style::default().fg(ORANGE));

        // Help
        let help = "[Enter] select  [Esc] cancel  [Ctrl+L] clear search";
        let help_x = area.right().saturating_sub(help.len() as u16 + 1);
        buf.set_string(help_x, y, help, Style::default().fg(TEXT_MUTED));
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_picker_state_new() {
        let state = ModelPickerState::new();
        assert!(state.all_models.is_empty());
        assert!(state.filtered_models.is_empty());
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_model_picker_state_load() {
        // Models come from API only, no fallback
        let mut state = ModelPickerState::new();
        state.load_models("cortex", "anthropic/claude-opus-4-20250514");
        // Without API, models list will be empty
        assert!(state.all_models.is_empty());
    }

    #[test]
    fn test_model_picker_search() {
        // Models now come from API only
        // Test search functionality with manually added models
        let mut state = ModelPickerState::new();
        // Add test models manually
        state.all_models = vec![
            super::ModelItem {
                id: "anthropic/claude-opus-4".to_string(),
                name: "Claude Opus 4".to_string(),
                provider: "cortex".to_string(),
                context_window: Some(200000),
                is_current: false,
                is_popular: false,
            },
            super::ModelItem {
                id: "openai/gpt-4o".to_string(),
                name: "GPT-4o".to_string(),
                provider: "cortex".to_string(),
                context_window: Some(128000),
                is_current: false,
                is_popular: false,
            },
        ];
        state.filtered_models = state.all_models.clone();

        let initial_count = state.filtered_models.len();
        assert_eq!(initial_count, 2);

        state.handle_char('c');
        state.handle_char('l');
        state.handle_char('a');
        state.handle_char('u');
        state.handle_char('d');
        state.handle_char('e');

        // Should filter to Claude models
        assert!(state.filtered_models.len() < initial_count);
        assert!(
            state
                .filtered_models
                .iter()
                .all(|m| m.name.to_lowercase().contains("claude")
                    || m.id.to_lowercase().contains("claude"))
        );
    }

    #[test]
    fn test_model_picker_navigation() {
        // Test navigation with manually added models
        let mut state = ModelPickerState::new();
        state.all_models = vec![
            super::ModelItem {
                id: "model-1".to_string(),
                name: "Model 1".to_string(),
                provider: "cortex".to_string(),
                context_window: Some(100000),
                is_current: false,
                is_popular: false,
            },
            super::ModelItem {
                id: "model-2".to_string(),
                name: "Model 2".to_string(),
                provider: "cortex".to_string(),
                context_window: Some(100000),
                is_current: false,
                is_popular: false,
            },
        ];
        state.filtered_models = state.all_models.clone();

        assert_eq!(state.selected, 0);
        state.select_next();
        assert_eq!(state.selected, 1);
        state.select_prev();
        assert_eq!(state.selected, 0);
    }
}
