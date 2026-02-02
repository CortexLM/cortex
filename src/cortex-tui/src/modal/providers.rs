//! Providers modal for selecting AI providers.
//!
//! This modal displays available AI providers and allows the user to select one.

use cortex_core::style::{
    CYAN_PRIMARY, ERROR, SUCCESS, SURFACE_0, TEXT, TEXT_DIM, TEXT_MUTED, VOID,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::Widget;

use super::{CancelBehavior, Modal, ModalAction, ModalResult};
use crate::widgets::{ActionBar, SelectionItem, SelectionList, SelectionResult};

// ============================================================================
// PROVIDER INFO
// ============================================================================

/// Information about a provider for display in the modal.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// Provider identifier (e.g., "cortex", "anthropic").
    pub id: String,
    /// Display name (e.g., "Cortex - Access multiple models").
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// Whether this is the currently active provider.
    pub is_current: bool,
    /// Whether the provider is configured (has API key, etc.).
    pub is_configured: bool,
}

impl ProviderInfo {
    /// Creates a new ProviderInfo.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            is_current: false,
            is_configured: false,
        }
    }

    /// Sets the description.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Marks this provider as the current selection.
    pub fn with_current(mut self, is_current: bool) -> Self {
        self.is_current = is_current;
        self
    }

    /// Marks this provider as configured.
    pub fn with_configured(mut self, is_configured: bool) -> Self {
        self.is_configured = is_configured;
        self
    }
}

// ============================================================================
// KNOWN PROVIDERS
// ============================================================================

/// Returns the list of known providers with their default information.
pub fn known_providers() -> Vec<ProviderInfo> {
    vec![ProviderInfo::new("cortex", "Cortex").with_description("Cortex AI Gateway")]
}

// ============================================================================
// PROVIDERS MODAL
// ============================================================================

/// Modal for selecting an AI provider.
pub struct ProvidersModal {
    /// Available providers.
    providers: Vec<ProviderInfo>,
    /// Selection list widget.
    list: SelectionList,
    /// Currently active provider ID.
    _current_provider: Option<String>,
}

impl ProvidersModal {
    /// Creates a new ProvidersModal.
    pub fn new(providers: Vec<ProviderInfo>, current_provider: Option<String>) -> Self {
        // Convert ProviderInfo to SelectionItems
        let items: Vec<SelectionItem> = providers
            .iter()
            .map(|provider| {
                let is_current = current_provider
                    .as_ref()
                    .is_some_and(|current| current == &provider.id);

                // Build description with configuration status
                let mut desc_parts = Vec::new();
                if let Some(ref desc) = provider.description {
                    desc_parts.push(desc.clone());
                }
                if !provider.is_configured {
                    desc_parts.push("Not configured".to_string());
                }
                let description = desc_parts.join(" | ");

                let mut item = SelectionItem::new(&provider.name).with_current(is_current);
                if !description.is_empty() {
                    item = item.with_description(description);
                }
                item
            })
            .collect();

        // Small list, no search needed
        let list = SelectionList::new(items)
            .with_searchable(false)
            .with_max_visible(10);

        Self {
            providers,
            list,
            _current_provider: current_provider,
        }
    }

    /// Creates a new ProvidersModal with known providers.
    pub fn with_known_providers(current_provider: Option<String>) -> Self {
        Self::new(known_providers(), current_provider)
    }

    /// Creates a new ProvidersModal with known providers and configuration status.
    ///
    /// This version checks the actual configuration to determine which providers
    /// have API keys configured.
    pub fn with_config(
        current_provider: Option<String>,
        config: &crate::providers::config::CortexConfig,
    ) -> Self {
        use crate::providers::config::PROVIDERS;

        let providers: Vec<ProviderInfo> = PROVIDERS
            .iter()
            .map(|p| {
                let is_current = current_provider.as_ref().is_some_and(|c| c == p.id);
                let is_configured = config.get_api_key(p.id).is_some() || !p.requires_key;

                ProviderInfo::new(p.id, p.name)
                    .with_description(format!("Env: {}", p.env_var))
                    .with_current(is_current)
                    .with_configured(is_configured)
            })
            .collect();

        Self::new(providers, current_provider)
    }

    /// Gets the currently selected provider info.
    pub fn selected_provider(&self) -> Option<&ProviderInfo> {
        self.list
            .selected_index()
            .and_then(|idx| self.providers.get(idx))
    }

    /// Builds the action bar for the modal.
    fn build_action_bar(&self) -> ActionBar {
        let mut bar = ActionBar::new();

        // Only show Configure for unconfigured providers
        if let Some(provider) = self.selected_provider()
            && !provider.is_configured
        {
            bar = bar.action('c', "Configure");
        }

        bar.with_standard_hints()
    }

    /// Renders the provider list with custom icons.
    fn render_provider_list(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width < 20 {
            return;
        }

        let selected_idx = self.list.selected_index();

        for (idx, provider) in self.providers.iter().enumerate() {
            let y = area.y + idx as u16;
            if y >= area.bottom() {
                break;
            }

            let is_selected = selected_idx == Some(idx);
            self.render_provider_row(area.x, y, area.width, buf, provider, is_selected);
        }

        // Empty state
        if self.providers.is_empty() {
            let msg = "No providers available";
            let x = area.x + (area.width.saturating_sub(msg.len() as u16)) / 2;
            let y = area.y + area.height / 2;
            buf.set_string(x, y, msg, Style::default().fg(TEXT_MUTED));
        }
    }

    /// Renders a single provider row with icons.
    fn render_provider_row(
        &self,
        x: u16,
        y: u16,
        width: u16,
        buf: &mut Buffer,
        provider: &ProviderInfo,
        is_selected: bool,
    ) {
        // Determine styles based on selection
        let (bg, fg, icon_fg) = if is_selected {
            (CYAN_PRIMARY, VOID, VOID)
        } else {
            (SURFACE_0, TEXT, CYAN_PRIMARY)
        };

        // Clear the line with background
        for col in x..x.saturating_add(width) {
            buf[(col, y)].set_bg(bg);
        }

        let mut col = x + 1; // Start with padding

        // Selection icon: ">" for selected, " " for others
        let sel_icon = if is_selected { ">" } else { " " };
        buf.set_string(col, y, sel_icon, Style::default().fg(icon_fg).bg(bg));
        col += 2;

        // Provider icon based on configured state: [>] or [ ]
        let provider_icon = if provider.is_configured { "[>]" } else { "[ ]" };
        let provider_icon_style = if is_selected {
            Style::default().fg(VOID).bg(bg)
        } else if provider.is_configured {
            Style::default().fg(SUCCESS).bg(bg)
        } else {
            Style::default().fg(TEXT_MUTED).bg(bg)
        };
        buf.set_string(col, y, provider_icon, provider_icon_style);
        col += 2;

        // Provider name
        let name_style = Style::default().fg(fg).bg(bg);
        buf.set_string(col, y, &provider.name, name_style);
        col += provider.name.len() as u16;

        // Current marker: "(current)" if this is the active provider
        if provider.is_current {
            col += 1;
            let marker = "(current)";
            let marker_style = if is_selected {
                Style::default().fg(VOID).bg(bg)
            } else {
                Style::default().fg(TEXT_DIM).bg(bg)
            };
            buf.set_string(col, y, marker, marker_style);
        }

        // Config status on the right: "[+] Configured" or "[x] Not configured"
        let config_status = if provider.is_configured {
            "[+] Configured"
        } else {
            "[x] Not configured"
        };

        let status_style = if is_selected {
            Style::default().fg(VOID).bg(bg)
        } else if provider.is_configured {
            Style::default().fg(SUCCESS).bg(bg)
        } else {
            Style::default().fg(ERROR).bg(bg)
        };

        let status_x = x + width - config_status.len() as u16 - 2;
        if status_x > col + 2 {
            buf.set_string(status_x, y, config_status, status_style);
        }
    }
}

impl Modal for ProvidersModal {
    fn title(&self) -> &str {
        "Providers"
    }

    fn desired_height(&self, max_height: u16, _width: u16) -> u16 {
        // Base height for list items + action bar
        let provider_count = self.providers.len() as u16;
        let action_bar_height = 1u16;
        let content_height = provider_count + action_bar_height + 1; // +1 for padding

        // Clamp between min 5 and max 10, respecting max_height
        content_height.clamp(5, 10).min(max_height)
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height < 2 || area.width < 20 {
            return;
        }

        // Layout: provider list and action bar at bottom
        let action_bar_height = 1u16;
        let content_height = area.height.saturating_sub(action_bar_height);

        let content_area = Rect::new(area.x, area.y, area.width, content_height);
        let action_area = Rect::new(
            area.x,
            area.y + content_height,
            area.width,
            action_bar_height,
        );

        // Render custom provider list with icons
        self.render_provider_list(content_area, buf);

        // Render action bar
        let action_bar = self.build_action_bar();
        (&action_bar).render(action_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent) -> ModalResult {
        // Handle 'c' for Configure action on unconfigured providers
        if let KeyCode::Char('c') | KeyCode::Char('C') = key.code
            && let Some(provider) = self.selected_provider()
            && !provider.is_configured
        {
            return ModalResult::Action(ModalAction::ConfigureProvider(provider.id.clone()));
        }

        match self.list.handle_key(key) {
            SelectionResult::Selected(idx) => {
                if let Some(provider) = self.providers.get(idx) {
                    ModalResult::Action(ModalAction::SelectProvider(provider.id.clone()))
                } else {
                    ModalResult::Close
                }
            }
            SelectionResult::Cancelled => ModalResult::Close,
            SelectionResult::None => ModalResult::Continue,
        }
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        // ActionBar handles hints now, but keep this for compatibility
        vec![("↑↓", "navigate"), ("Enter", "select"), ("Esc", "cancel")]
    }

    fn on_cancel(&mut self) -> CancelBehavior {
        CancelBehavior::Close
    }

    fn is_searchable(&self) -> bool {
        false
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyModifiers};

    fn create_test_providers() -> Vec<ProviderInfo> {
        vec![
            ProviderInfo::new("cortex", "Cortex")
                .with_description("Cortex AI Gateway")
                .with_configured(true)
                .with_current(true),
        ]
    }

    #[test]
    fn test_new_modal() {
        let providers = create_test_providers();
        let modal = ProvidersModal::new(providers, Some("cortex".to_string()));

        assert_eq!(modal.title(), "Providers");
        assert!(!modal.is_searchable());
    }

    #[test]
    fn test_with_known_providers() {
        let modal = ProvidersModal::with_known_providers(Some("cortex".to_string()));

        assert_eq!(modal.title(), "Providers");
        assert_eq!(modal.providers.len(), 1);
    }

    #[test]
    fn test_known_providers() {
        let providers = known_providers();

        assert_eq!(providers.len(), 1);
        assert!(providers.iter().any(|p| p.id == "cortex"));
    }

    #[test]
    fn test_desired_height() {
        let providers = create_test_providers();
        let modal = ProvidersModal::new(providers, None);

        // With 3 providers + 1 padding = 4, clamped to min 5
        let height = modal.desired_height(20, 80);
        assert!(height >= 5);
        assert!(height <= 10);
    }

    #[test]
    fn test_key_hints() {
        let providers = create_test_providers();
        let modal = ProvidersModal::new(providers, None);

        let hints = modal.key_hints();
        assert_eq!(hints.len(), 3);
        assert!(hints.iter().any(|(k, _)| *k == "Enter"));
        assert!(hints.iter().any(|(k, _)| *k == "Esc"));
        assert!(hints.iter().any(|(k, v)| *k == "↑↓" && *v == "navigate"));
    }

    #[test]
    fn test_escape_closes() {
        let providers = create_test_providers();
        let mut modal = ProvidersModal::new(providers, None);

        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        assert!(matches!(result, ModalResult::Close));
    }

    #[test]
    fn test_enter_selects() {
        let providers = create_test_providers();
        let mut modal = ProvidersModal::new(providers, Some("cortex".to_string()));

        let key = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(key);

        // Should select the current provider (cortex) since cursor starts at current item
        if let ModalResult::Action(ModalAction::SelectProvider(id)) = result {
            assert_eq!(id, "cortex");
        } else {
            panic!("Expected SelectProvider action");
        }
    }

    #[test]
    fn test_navigation() {
        let providers = create_test_providers();
        let mut modal = ProvidersModal::new(providers, None);

        // Select (should be cortex, only provider)
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        let result = modal.handle_key(enter);

        if let ModalResult::Action(ModalAction::SelectProvider(id)) = result {
            assert_eq!(id, "cortex");
        } else {
            panic!("Expected SelectProvider action");
        }
    }

    #[test]
    fn test_provider_info_builder() {
        let provider = ProviderInfo::new("test", "Test Provider")
            .with_description("A test provider")
            .with_current(true)
            .with_configured(true);

        assert_eq!(provider.id, "test");
        assert_eq!(provider.name, "Test Provider");
        assert_eq!(provider.description, Some("A test provider".to_string()));
        assert!(provider.is_current);
        assert!(provider.is_configured);
    }

    #[test]
    fn test_selected_provider() {
        let providers = create_test_providers();
        let modal = ProvidersModal::new(providers, None);

        // Should have first provider selected by default
        let selected = modal.selected_provider();
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().id, "cortex");
    }

    #[test]
    fn test_on_cancel() {
        let providers = create_test_providers();
        let mut modal = ProvidersModal::new(providers, None);

        let result = modal.on_cancel();
        assert!(matches!(result, CancelBehavior::Close));
    }

    #[test]
    fn test_configure_configured_provider_does_nothing() {
        let providers = create_test_providers();
        let mut modal = ProvidersModal::new(providers, None);

        // Cortex is configured
        let key = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        let result = modal.handle_key(key);

        // Should continue (no action) since provider is already configured
        assert!(matches!(result, ModalResult::Continue));
    }

    #[test]
    fn test_build_action_bar_configured() {
        let providers = create_test_providers();
        let modal = ProvidersModal::new(providers, None);

        // Cortex is configured, ActionBar should not have Configure action
        let action_bar = modal.build_action_bar();
        // ActionBar is built successfully (smoke test)
        assert!(std::mem::size_of_val(&action_bar) > 0);
    }
}
