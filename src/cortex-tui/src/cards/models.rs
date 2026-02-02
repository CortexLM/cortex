//! Models card for selecting AI models.
//!
//! This card displays available AI models and allows the user to select one.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

use crate::cards::{CancellationEvent, CardAction, CardResult, CardView};
use crate::widgets::{SelectionItem, SelectionList, SelectionResult};

// ============================================================
// MODEL INFO (card-specific)
// ============================================================

/// Information about a model for display in the card.
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
}

// ============================================================
// MODELS CARD
// ============================================================

/// Card for selecting an AI model.
pub struct ModelsCard {
    /// Available models.
    models: Vec<ModelInfo>,
    /// Selection list widget.
    list: SelectionList,
    /// Currently active model ID.
    _current_model: Option<String>,
    /// Whether a selection has been made.
    completed: bool,
}

impl ModelsCard {
    /// Creates a new ModelsCard.
    pub fn new(models: Vec<ModelInfo>, current_model: Option<String>) -> Self {
        // Convert ModelInfo to SelectionItems
        let items: Vec<SelectionItem> = models
            .iter()
            .map(|model| {
                let is_current = current_model
                    .as_ref()
                    .is_some_and(|current| current == &model.id);

                // Build description: "Provider | 200k context"
                let mut desc_parts = vec![model.provider.clone()];
                if let Some(context) = model.format_context() {
                    desc_parts.push(context);
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
            completed: false,
        }
    }

    /// Gets the currently selected model info.
    pub fn selected_model(&self) -> Option<&ModelInfo> {
        self.list
            .selected_index()
            .and_then(|idx| self.models.get(idx))
    }
}

impl CardView for ModelsCard {
    fn title(&self) -> &str {
        "Models"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Base height for list items + search bar + some padding
        let model_count = self.models.len() as u16;
        let content_height = model_count + 2; // +2 for search bar and padding

        // Clamp between min 5 and max 12, respecting max_height
        content_height.clamp(5, 12).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Render the selection list
        (&self.list).render(area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> CardResult {
        match key.code {
            KeyCode::Esc => CardResult::Close,
            KeyCode::Enter => {
                // First let the list handle it to get the selection
                if let SelectionResult::Selected(idx) = self.list.handle_key(key)
                    && let Some(model) = self.models.get(idx)
                {
                    self.completed = true;
                    return CardResult::Action(CardAction::SelectModel(model.id.clone()));
                }
                CardResult::Continue
            }
            _ => {
                // Let the list handle navigation and search
                match self.list.handle_key(key) {
                    SelectionResult::Selected(idx) => {
                        if let Some(model) = self.models.get(idx) {
                            self.completed = true;
                            CardResult::Action(CardAction::SelectModel(model.id.clone()))
                        } else {
                            CardResult::Continue
                        }
                    }
                    SelectionResult::Cancelled => CardResult::Close,
                    SelectionResult::None => CardResult::Continue,
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

    fn on_cancel(&mut self) -> CancellationEvent {
        // If there's an active search, clear it first
        if !self.list.search_query().is_empty() {
            // Clear the search by sending Ctrl+U
            let clear_key =
                KeyEvent::new(KeyCode::Char('u'), crossterm::event::KeyModifiers::CONTROL);
            self.list.handle_key(clear_key);
            CancellationEvent::Handled
        } else {
            CancellationEvent::NotHandled
        }
    }

    fn is_complete(&self) -> bool {
        self.completed
    }

    fn is_searchable(&self) -> bool {
        true
    }

    fn search_placeholder(&self) -> Option<&str> {
        Some("Filter models...")
    }
}

// ============================================================
// CONVERSIONS
// ============================================================

/// Convert from provider ModelInfo to card ModelInfo.
impl From<crate::providers::ModelInfo> for ModelInfo {
    fn from(model: crate::providers::ModelInfo) -> Self {
        Self {
            id: model.id,
            name: model.name,
            provider: model.provider,
            context_length: Some(model.context_window),
            description: None,
            is_current: false,
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;

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
    fn test_new_card() {
        let models = create_test_models();
        let card = ModelsCard::new(models, Some("claude-sonnet-4".to_string()));

        assert_eq!(card.title(), "Models");
        assert!(!card.is_complete());
        assert!(card.is_searchable());
        assert_eq!(card.search_placeholder(), Some("Filter models..."));
    }

    #[test]
    fn test_desired_height() {
        let models = create_test_models();
        let card = ModelsCard::new(models, None);

        // With 4 models + 2 padding = 6, clamped to min 5
        let height = card.desired_height(20, 80);
        assert!(height >= 5);
        assert!(height <= 12);
    }

    #[test]
    fn test_key_hints() {
        let models = create_test_models();
        let card = ModelsCard::new(models, None);

        let hints = card.key_hints();
        assert!(!hints.is_empty());
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
    }

    #[test]
    fn test_escape_closes() {
        let models = create_test_models();
        let mut card = ModelsCard::new(models, None);

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = card.handle_key(key);

        assert!(matches!(result, CardResult::Close));
    }

    #[test]
    fn test_enter_selects() {
        let models = create_test_models();
        let mut card = ModelsCard::new(models, Some("claude-sonnet-4".to_string()));

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = card.handle_key(key);

        // Should select the current model (claude-sonnet-4)
        if let CardResult::Action(CardAction::SelectModel(id)) = result {
            assert_eq!(id, "claude-sonnet-4");
        } else {
            panic!("Expected SelectModel action");
        }
    }

    #[test]
    fn test_navigation() {
        let models = create_test_models();
        let mut card = ModelsCard::new(models, Some("claude-sonnet-4".to_string()));

        // Move down
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        card.handle_key(down);

        // Select (should be gpt-4o now)
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = card.handle_key(enter);

        if let CardResult::Action(CardAction::SelectModel(id)) = result {
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

        let card_model: ModelInfo = provider_model.into();

        assert_eq!(card_model.id, "claude-opus-4");
        assert_eq!(card_model.name, "Claude Opus 4");
        assert_eq!(card_model.provider, "anthropic");
        assert_eq!(card_model.context_length, Some(200_000));
        assert!(!card_model.is_current);
    }
}
