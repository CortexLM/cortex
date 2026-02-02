//! Models modal for selecting AI models.
//!
//! This modal displays available AI models and allows the user to select one.
//! Models are grouped by provider with section headers for easy navigation.

use cortex_core::style::{BORDER, CYAN_PRIMARY, SURFACE_0, TEXT, TEXT_DIM, TEXT_MUTED, VOID};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;
use std::collections::BTreeMap;

use crate::modal::{CancelBehavior, Modal, ModalAction, ModalResult, render_section_header};
use crate::widgets::{ActionBar, SelectionItem, SelectionList, SelectionResult};

// ============================================================================
// MODEL INFO
// ============================================================================

/// Information about a model for display in the modal.
#[derive(Debug, Clone)]
pub struct ModelInfo {
    /// Model identifier (e.g., "anthropic/claude-opus-4-20250514").
    pub id: String,
    /// Display name.
    pub name: String,
    /// Provider name (e.g., "Anthropic", "OpenAI").
    pub provider: String,
    /// Context window size in tokens.
    pub context_length: Option<u32>,
    /// Optional description.
    pub description: Option<String>,
    /// Whether this is the currently active model.
    pub is_current: bool,
    /// Credit multiplier for input tokens.
    pub credit_multiplier_input: Option<String>,
    /// Credit multiplier for output tokens.
    pub credit_multiplier_output: Option<String>,
    /// Price version for price verification.
    pub price_version: Option<i32>,
}

impl ModelInfo {
    /// Creates a new ModelInfo.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        provider: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            provider: provider.into(),
            context_length: None,
            description: None,
            is_current: false,
            credit_multiplier_input: None,
            credit_multiplier_output: None,
            price_version: None,
        }
    }

    /// Sets the context length.
    pub fn with_context_length(mut self, tokens: u32) -> Self {
        self.context_length = Some(tokens);
        self
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Marks this model as the current selection.
    pub fn with_current(mut self, is_current: bool) -> Self {
        self.is_current = is_current;
        self
    }

    /// Formats the context length for display (e.g., "200k context").
    fn format_context(&self) -> Option<String> {
        self.context_length.map(|tokens| {
            if tokens >= 1_000_000 {
                format!("{}M context", tokens / 1_000_000)
            } else if tokens >= 1_000 {
                format!("{}k context", tokens / 1_000)
            } else {
                format!("{} context", tokens)
            }
        })
    }

    /// Formats the pricing info for display (e.g., "1.5x in / 7.5x out").
    fn format_pricing(&self) -> Option<String> {
        match (
            &self.credit_multiplier_input,
            &self.credit_multiplier_output,
        ) {
            (Some(input), Some(output)) => Some(format!("{}x in / {}x out", input, output)),
            (Some(input), None) => Some(format!("{}x in", input)),
            (None, Some(output)) => Some(format!("{}x out", output)),
            (None, None) => None,
        }
    }
}

// ============================================================================
// MODELS MODAL
// ============================================================================

/// Modal for selecting an AI model.
pub struct ModelsModal {
    /// Available models.
    models: Vec<ModelInfo>,
    /// Selection list widget.
    list: SelectionList,
    /// Currently active model ID.
    _current_model: Option<String>,
    /// Provider groups for rendering (provider name -> list of model indices).
    provider_groups: Vec<(String, Vec<usize>)>,
    /// Whether to show provider headers (false if only one provider).
    show_provider_headers: bool,
}

impl ModelsModal {
    /// Creates a new ModelsModal.
    pub fn new(models: Vec<ModelInfo>, current_model: Option<String>) -> Self {
        // Group models by provider
        let mut groups_map: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (idx, model) in models.iter().enumerate() {
            groups_map
                .entry(model.provider.clone())
                .or_default()
                .push(idx);
        }
        let provider_groups: Vec<(String, Vec<usize>)> = groups_map.into_iter().collect();
        let show_provider_headers = provider_groups.len() > 1;

        // Convert ModelInfo to SelectionItems
        let items: Vec<SelectionItem> = models
            .iter()
            .map(|model| {
                let is_current = current_model
                    .as_ref()
                    .is_some_and(|current| current == &model.id);

                // Build description: "Provider | 200k context | 1.5x in / 7.5x out"
                let mut desc_parts = vec![model.provider.clone()];
                if let Some(context) = model.format_context() {
                    desc_parts.push(context);
                }
                if let Some(pricing) = model.format_pricing() {
                    desc_parts.push(pricing);
                }
                let description = desc_parts.join(" | ");

                SelectionItem::new(&model.name)
                    .with_description(description)
                    .with_current(is_current)
            })
            .collect();

        let list = SelectionList::new(items)
            .with_searchable(true)
            .with_max_visible(12);

        Self {
            models,
            list,
            _current_model: current_model,
            provider_groups,
            show_provider_headers,
        }
    }

    /// Gets the currently selected model info.
    pub fn selected_model(&self) -> Option<&ModelInfo> {
        self.list
            .selected_index()
            .and_then(|idx| self.models.get(idx))
    }

    /// Builds the action bar for the modal.
    fn build_action_bar(&self) -> ActionBar {
        ActionBar::new().with_standard_hints()
    }

    /// Renders the search bar.
    fn render_search_bar(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let query = self.list.search_query();
        let icon = "> ";
        let bracket_open = "[";
        let bracket_close = "]";

        let display_text = if query.is_empty() {
            "Filter models..."
        } else {
            query
        };

        let text_style = if query.is_empty() {
            Style::default().fg(TEXT_MUTED)
        } else {
            Style::default().fg(CYAN_PRIMARY)
        };

        // Build the search line
        let mut col = area.x + 1;
        buf.set_string(col, area.y, icon, Style::default());
        col += 3; // icon width

        buf.set_string(col, area.y, bracket_open, Style::default().fg(BORDER));
        col += 1;

        buf.set_string(col, area.y, display_text, text_style);
        col += display_text.len() as u16;

        buf.set_string(col, area.y, bracket_close, Style::default().fg(BORDER));
    }

    /// Renders the models grouped by provider.
    fn render_grouped_models(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let selected_idx = self.list.selected_index();
        let search_query = self.list.search_query().to_lowercase();
        let mut y = area.y;

        // If searching, use flat list rendering (no grouping)
        if !search_query.is_empty() {
            self.render_flat_filtered_list(area, buf, &search_query, selected_idx);
            return;
        }

        // Render models grouped by provider
        for (provider, model_indices) in &self.provider_groups {
            // Check if any models in this group should be visible
            if y >= area.bottom() {
                break;
            }

            // Render provider header if multiple providers
            if self.show_provider_headers && y < area.bottom() {
                render_section_header(Rect::new(area.x, y, area.width, 1), buf, provider);
                y += 1;
            }

            // Render models in this group
            for &model_idx in model_indices {
                if y >= area.bottom() {
                    break;
                }

                if let Some(model) = self.models.get(model_idx) {
                    let is_selected = selected_idx == Some(model_idx);
                    self.render_model_row(area.x, y, area.width, buf, model, is_selected);
                    y += 1;
                }
            }
        }

        // Empty state
        if self.models.is_empty() {
            let msg = "No models available";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Renders a flat filtered list when searching.
    fn render_flat_filtered_list(
        &self,
        area: Rect,
        buf: &mut Buffer,
        search_query: &str,
        selected_idx: Option<usize>,
    ) {
        let mut y = area.y;

        for (idx, model) in self.models.iter().enumerate() {
            if y >= area.bottom() {
                break;
            }

            // Filter by search query
            if !model.name.to_lowercase().contains(search_query)
                && !model.provider.to_lowercase().contains(search_query)
            {
                continue;
            }

            let is_selected = selected_idx == Some(idx);
            self.render_model_row(area.x, y, area.width, buf, model, is_selected);
            y += 1;
        }

        // Empty state for no matches
        if y == area.y {
            let msg = "No matches";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Renders a single model row.
    /// Format: "> model-name                    200k ctx    (current)"
    fn render_model_row(
        &self,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        model: &ModelInfo,
        is_selected: bool,
    ) {
        // Determine styles
        let (bg, fg, prefix_fg) = if is_selected {
            (CYAN_PRIMARY, VOID, VOID)
        } else {
            (SURFACE_0, TEXT, CYAN_PRIMARY)
        };

        // Clear the line with background
        for col in x..x.saturating_add(width) {
            buf[(col, y)].set_bg(bg);
        }

        let mut col = x;

        // Selection prefix: ">" for selected, " " for others
        let prefix = if is_selected { ">" } else { " " };
        buf.set_string(col, y, prefix, Style::default().fg(prefix_fg).bg(bg));
        col += 2;

        // Model name
        let name_style = Style::default().fg(fg).bg(bg);
        let max_name_len = 35.min(width.saturating_sub(30) as usize);
        let truncated_name = if model.name.len() > max_name_len && max_name_len > 3 {
            format!("{}...", &model.name[..max_name_len.saturating_sub(3)])
        } else {
            model.name.clone()
        };
        buf.set_string(col, y, &truncated_name, name_style);

        // Context length (right-aligned area)
        let ctx_str = if let Some(ctx) = model.context_length {
            format!("{}k ctx", ctx / 1000)
        } else {
            String::new()
        };

        // Current marker
        let current_marker = if model.is_current { "(current)" } else { "" };

        // Calculate positions for right-aligned elements
        let right_section = format!("{}    {}", ctx_str, current_marker);
        let right_x = x + width.saturating_sub(right_section.len() as u16 + 2);

        if !ctx_str.is_empty() && right_x > col + truncated_name.len() as u16 + 2 {
            let ctx_style = if is_selected {
                Style::default().fg(VOID).bg(bg)
            } else {
                Style::default().fg(TEXT_DIM).bg(bg)
            };
            buf.set_string(right_x, y, &ctx_str, ctx_style);
        }

        if !current_marker.is_empty() {
            let marker_x = x + width.saturating_sub(current_marker.len() as u16 + 2);
            let marker_style = if is_selected {
                Style::default().fg(VOID).bg(bg)
            } else {
                Style::default().fg(TEXT_DIM).bg(bg)
            };
            buf.set_string(marker_x, y, current_marker, marker_style);
        }
    }
}

impl Modal for ModelsModal {
    fn title(&self) -> &str {
        "Models"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Base height for list items + search bar + action bar + provider headers
        let model_count = self.models.len() as u16;
        let header_count = if self.show_provider_headers {
            self.provider_groups.len() as u16
        } else {
            0
        };
        let content_height = model_count + header_count + 3; // +3 for search bar, action bar, padding

        // Clamp between min 6 and max 14, respecting max_height
        content_height.clamp(6, 14).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 3 || area.width < 10 {
            return;
        }

        // Layout: search bar at top, models in middle, action bar at bottom
        let search_height = 1u16;
        let action_bar_height = 1u16;
        let content_height = area
            .height
            .saturating_sub(search_height + action_bar_height);

        let search_area = Rect::new(area.x, area.y, area.width, search_height);
        let content_area = Rect::new(area.x, area.y + search_height, area.width, content_height);
        let action_area = Rect::new(
            area.x,
            area.y + search_height + content_height,
            area.width,
            action_bar_height,
        );

        // Render search bar
        self.render_search_bar(search_area, buf);

        // Render grouped models
        self.render_grouped_models(content_area, buf);

        // Render action bar
        let action_bar = self.build_action_bar();
        (&action_bar).render(action_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        match key.code {
            KeyCode::Esc => {
                // Check if we should handle escape internally (e.g., clear search)
                match self.on_cancel() {
                    CancelBehavior::Handled => ModalResult::Continue,
                    CancelBehavior::Close => ModalResult::Close,
                }
            }
            KeyCode::Enter => {
                // First let the list handle it to get the selection
                if let SelectionResult::Selected(idx) = self.list.handle_key(key)
                    && let Some(model) = self.models.get(idx)
                {
                    return ModalResult::Action(ModalAction::SelectModel(model.id.clone()));
                }
                ModalResult::Continue
            }
            _ => {
                // Let the list handle navigation and search
                match self.list.handle_key(key) {
                    SelectionResult::Selected(idx) => {
                        if let Some(model) = self.models.get(idx) {
                            ModalResult::Action(ModalAction::SelectModel(model.id.clone()))
                        } else {
                            ModalResult::Continue
                        }
                    }
                    SelectionResult::Cancelled => ModalResult::Close,
                    SelectionResult::None => ModalResult::Continue,
                }
            }
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("\u{2191}\u{2193}", "navigate"),
            ("Enter", "select"),
            ("Esc", "close"),
            ("/", "filter"),
        ]
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        // If there's an active search, clear it first
        if !self.list.search_query().is_empty() {
            // Clear the search by sending Ctrl+U
            let clear_key = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL);
            self.list.handle_key(clear_key);
            CancelBehavior::Handled
        } else {
            CancelBehavior::Close
        }
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Filter models...")
    }
}

// ============================================================================
// CONVERSIONS
// ============================================================================

/// Convert from provider ModelInfo to modal ModelInfo.
impl From<crate::providers::ModelInfo> for ModelInfo {
    fn from(model: crate::providers::ModelInfo) -> Self {
        Self {
            id: model.id,
            name: model.name,
            provider: model.provider,
            context_length: Some(model.context_window),
            description: None,
            is_current: false,
            credit_multiplier_input: model.credit_multiplier_input,
            credit_multiplier_output: model.credit_multiplier_output,
            price_version: model.price_version,
        }
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_models() -> Vec<ModelInfo> {
        vec![
            ModelInfo::new("claude-opus-4", "Claude Opus 4", "Anthropic")
                .with_context_length(200_000)
                .with_current(false),
            ModelInfo::new("claude-sonnet-4", "Claude Sonnet 4", "Anthropic")
                .with_context_length(200_000)
                .with_current(true),
            ModelInfo::new("gpt-4o", "GPT-4o", "OpenAI")
                .with_context_length(128_000)
                .with_current(false),
            ModelInfo::new("gemini-2.5-pro", "Gemini 2.5 Pro", "Google")
                .with_context_length(1_000_000)
                .with_current(false),
        ]
    }

    #[test]
    fn test_new_modal() {
        let models = create_test_models();
        let modal = ModelsModal::new(models, Some("claude-sonnet-4".to_string()));

        assert_eq!(modal.title(), "Models");
        assert!(modal.is_searchable());
        assert_eq!(modal.search_placeholder(), Some("Filter models..."));
    }

    #[test]
    fn test_desired_height() {
        let models = create_test_models();
        let modal = ModelsModal::new(models, None);

        // With 4 models + 2 padding = 6, clamped to min 5
        let height = modal.desired_height(20, 80);
        assert!(height >= 5);
        assert!(height <= 12);
    }

    #[test]
    fn test_key_hints() {
        let models = create_test_models();
        let modal = ModelsModal::new(models, None);

        let hints = modal.key_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
    }

    #[test]
    fn test_escape_closes() {
        let models = create_test_models();
        let mut modal = ModelsModal::new(models, None);

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_enter_selects() {
        let models = create_test_models();
        let mut modal = ModelsModal::new(models, Some("claude-sonnet-4".to_string()));

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        // Should select the current model (claude-sonnet-4)
        if let ModalResult::Action(ModalAction::SelectModel(id)) = result {
            assert_eq!(id, "claude-sonnet-4");
        } else {
            panic!("Expected SelectModel action");
        }
    }

    #[test]
    fn test_navigation() {
        let models = create_test_models();
        let mut modal = ModelsModal::new(models, Some("claude-sonnet-4".to_string()));

        // Move down
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        modal.handle_key(down);

        // Select (should be gpt-4o now)
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(enter);

        if let ModalResult::Action(ModalAction::SelectModel(id)) = result {
            assert_eq!(id, "gpt-4o");
        } else {
            panic!("Expected SelectModel action");
        }
    }

    #[test]
    fn test_format_context() {
        let model = ModelInfo::new("test", "Test", "Test").with_context_length(200_000);
        assert_eq!(model.format_context(), Some("200k context".to_string()));

        let model = ModelInfo::new("test", "Test", "Test").with_context_length(1_000_000);
        assert_eq!(model.format_context(), Some("1M context".to_string()));

        let model = ModelInfo::new("test", "Test", "Test").with_context_length(500);
        assert_eq!(model.format_context(), Some("500 context".to_string()));
    }

    #[test]
    fn test_from_provider_model_info() {
        let provider_model =
            crate::providers::ModelInfo::new("claude-opus-4", "Claude Opus 4", "anthropic")
                .with_context(200_000);

        let modal_model: ModelInfo = provider_model.into();

        assert_eq!(modal_model.id, "claude-opus-4");
        assert_eq!(modal_model.name, "Claude Opus 4");
        assert_eq!(modal_model.provider, "anthropic");
        assert_eq!(modal_model.context_length, Some(200_000));
        assert!(!modal_model.is_current);
    }

    #[test]
    fn test_escape_clears_search_first() {
        let models = create_test_models();
        let mut modal = ModelsModal::new(models, None);

        // Type something to create a search query
        let char_key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        modal.handle_key(char_key);

        // First escape should clear search, not close
        let esc_key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = modal.handle_key(esc_key);
        assert!(matches!(result, ModalResult::Continue));

        // Second escape should close
        let result = modal.handle_key(esc_key);
        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_provider_grouping_multiple_providers() {
        let models = create_test_models(); // Has Anthropic, OpenAI, Google
        let modal = ModelsModal::new(models, None);

        // Should show headers when multiple providers
        assert!(modal.show_provider_headers);
        assert_eq!(modal.provider_groups.len(), 3); // Anthropic, Google, OpenAI (sorted)
    }

    #[test]
    fn test_provider_grouping_single_provider() {
        let models = vec![
            ModelInfo::new("claude-opus-4", "Claude Opus 4", "Anthropic")
                .with_context_length(200_000),
            ModelInfo::new("claude-sonnet-4", "Claude Sonnet 4", "Anthropic")
                .with_context_length(200_000),
        ];
        let modal = ModelsModal::new(models, None);

        // Should not show headers when only one provider
        assert!(!modal.show_provider_headers);
        assert_eq!(modal.provider_groups.len(), 1);
    }

    #[test]
    fn test_provider_groups_contain_correct_indices() {
        let models = create_test_models();
        let modal = ModelsModal::new(models, None);

        // Find the Anthropic group (should have indices 0, 1)
        let anthropic_group = modal
            .provider_groups
            .iter()
            .find(|(name, _)| name == "Anthropic");
        assert!(anthropic_group.is_some());
        let (_, indices) = anthropic_group.unwrap();
        assert_eq!(indices.len(), 2);
        assert!(indices.contains(&0)); // claude-opus-4
        assert!(indices.contains(&1)); // claude-sonnet-4
    }

    #[test]
    fn test_build_action_bar() {
        let models = create_test_models();
        let modal = ModelsModal::new(models, None);

        // Should build action bar with standard hints
        let _action_bar = modal.build_action_bar();
        // ActionBar is created successfully (basic smoke test)
    }
}
