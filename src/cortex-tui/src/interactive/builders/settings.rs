//! Builder for settings selection with categories.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState, NavTab};

/// Setting category for grouping
#[derive(Debug, Clone, Copy, PartialEq)]
enum SettingCategory {
    Display,
    Behavior,
    AI,
    Git,
    Cloud,
    Privacy,
}

impl SettingCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::Display => "Display",
            Self::Behavior => "Behavior",
            Self::AI => "AI",
            Self::Git => "Git",
            Self::Cloud => "Cloud",
            Self::Privacy => "Privacy",
        }
    }
}

/// Settings definition
struct SettingDef {
    id: &'static str,
    label: &'static str,
    description: &'static str,
    category: SettingCategory,
}

const SETTINGS: &[SettingDef] = &[
    // Display
    SettingDef {
        id: "compact",
        label: "Compact Mode",
        description: "Reduce visual spacing",
        category: SettingCategory::Display,
    },
    SettingDef {
        id: "timestamps",
        label: "Timestamps",
        description: "Show message timestamps",
        category: SettingCategory::Display,
    },
    SettingDef {
        id: "line_numbers",
        label: "Line Numbers",
        description: "Show line numbers in code",
        category: SettingCategory::Display,
    },
    SettingDef {
        id: "word_wrap",
        label: "Word Wrap",
        description: "Wrap long lines",
        category: SettingCategory::Display,
    },
    SettingDef {
        id: "syntax_highlight",
        label: "Syntax Highlight",
        description: "Colorize code blocks",
        category: SettingCategory::Display,
    },
    // Behavior
    SettingDef {
        id: "auto_approve",
        label: "Auto Approve",
        description: "Auto-approve tool calls",
        category: SettingCategory::Behavior,
    },
    SettingDef {
        id: "sandbox",
        label: "Sandbox Mode",
        description: "Run tools in sandbox",
        category: SettingCategory::Behavior,
    },
    SettingDef {
        id: "streaming",
        label: "Streaming",
        description: "Stream responses live",
        category: SettingCategory::Behavior,
    },
    SettingDef {
        id: "auto_scroll",
        label: "Auto Scroll",
        description: "Scroll to new messages",
        category: SettingCategory::Behavior,
    },
    SettingDef {
        id: "sound",
        label: "Sound",
        description: "Play notification sounds",
        category: SettingCategory::Behavior,
    },
    // AI
    SettingDef {
        id: "thinking",
        label: "Thinking Mode",
        description: "Show model thinking",
        category: SettingCategory::AI,
    },
    SettingDef {
        id: "debug",
        label: "Debug Mode",
        description: "Show debug info",
        category: SettingCategory::AI,
    },
    SettingDef {
        id: "context_aware",
        label: "Context Aware",
        description: "Include open files context",
        category: SettingCategory::AI,
    },
    // Git
    SettingDef {
        id: "co_author",
        label: "Co-Author",
        description: "Add as commit co-author",
        category: SettingCategory::Git,
    },
    SettingDef {
        id: "auto_commit",
        label: "Auto Commit",
        description: "Suggest commits after changes",
        category: SettingCategory::Git,
    },
    SettingDef {
        id: "sign_commits",
        label: "Sign Commits",
        description: "GPG sign commits",
        category: SettingCategory::Git,
    },
    // Cloud
    SettingDef {
        id: "cloud_sync",
        label: "Cloud Sync",
        description: "Sync sessions to cloud",
        category: SettingCategory::Cloud,
    },
    SettingDef {
        id: "auto_save",
        label: "Auto Save",
        description: "Auto-save sessions",
        category: SettingCategory::Cloud,
    },
    SettingDef {
        id: "session_history",
        label: "Session History",
        description: "Keep session history",
        category: SettingCategory::Cloud,
    },
    // Privacy
    SettingDef {
        id: "telemetry",
        label: "Telemetry",
        description: "Send usage telemetry",
        category: SettingCategory::Privacy,
    },
    SettingDef {
        id: "analytics",
        label: "Analytics",
        description: "Usage analytics",
        category: SettingCategory::Privacy,
    },
];

/// Current settings state for display
#[derive(Default, Clone)]
pub struct SettingsSnapshot {
    // Display
    pub compact_mode: bool,
    pub timestamps: bool,
    pub line_numbers: bool,
    pub word_wrap: bool,
    pub syntax_highlight: bool,
    // Behavior
    pub auto_approve: bool,
    pub sandbox_mode: bool,
    pub streaming_enabled: bool,
    pub auto_scroll: bool,
    pub sound: bool,
    // AI
    pub thinking_enabled: bool,
    pub debug_mode: bool,
    pub context_aware: bool,
    // Git
    pub co_author: bool,
    pub auto_commit: bool,
    pub sign_commits: bool,
    // Cloud
    pub cloud_sync: bool,
    pub auto_save: bool,
    pub session_history: bool,
    // Privacy
    pub telemetry: bool,
    pub analytics: bool,
}

/// Tab pages for settings (max 3 categories per page)
const TABS: &[(&str, &str, &[SettingCategory])] = &[
    (
        "general",
        "General",
        &[SettingCategory::Display, SettingCategory::Behavior],
    ),
    (
        "ai",
        "AI & Git",
        &[SettingCategory::AI, SettingCategory::Git],
    ),
    (
        "cloud",
        "Cloud",
        &[SettingCategory::Cloud, SettingCategory::Privacy],
    ),
];

/// Build an interactive state for settings selection with tabbed navigation.
///
/// # Arguments
/// * `snapshot` - Current settings values
/// * `terminal_height` - Optional terminal height for dynamic max_visible calculation.
/// * `active_tab` - Which tab to show (0, 1, or 2)
pub fn build_settings_selector(
    snapshot: SettingsSnapshot,
    terminal_height: Option<u16>,
) -> InteractiveState {
    build_settings_selector_with_tab(snapshot, terminal_height, 0)
}

/// Build settings selector with specific tab active.
pub fn build_settings_selector_with_tab(
    snapshot: SettingsSnapshot,
    terminal_height: Option<u16>,
    active_tab: usize,
) -> InteractiveState {
    let active_tab = active_tab.min(TABS.len() - 1);
    let (_, _, categories) = TABS[active_tab];

    let mut items = Vec::new();
    let mut current_category: Option<SettingCategory> = None;

    for setting in SETTINGS {
        // Only include settings from active tab's categories
        if !categories.contains(&setting.category) {
            continue;
        }

        // Add category separator if category changed
        if current_category != Some(setting.category) {
            current_category = Some(setting.category);
            let sep = InteractiveItem::new(
                format!("__cat_{}", setting.category.label()),
                setting.category.label().to_string(),
            )
            .as_separator();
            items.push(sep);
        }

        let is_enabled = match setting.id {
            // Display
            "compact" => snapshot.compact_mode,
            "timestamps" => snapshot.timestamps,
            "line_numbers" => snapshot.line_numbers,
            "word_wrap" => snapshot.word_wrap,
            "syntax_highlight" => snapshot.syntax_highlight,
            // Behavior
            "auto_approve" => snapshot.auto_approve,
            "sandbox" => snapshot.sandbox_mode,
            "streaming" => snapshot.streaming_enabled,
            "auto_scroll" => snapshot.auto_scroll,
            "sound" => snapshot.sound,
            // AI
            "thinking" => snapshot.thinking_enabled,
            "debug" => snapshot.debug_mode,
            "context_aware" => snapshot.context_aware,
            // Git
            "co_author" => snapshot.co_author,
            "auto_commit" => snapshot.auto_commit,
            "sign_commits" => snapshot.sign_commits,
            // Cloud
            "cloud_sync" => snapshot.cloud_sync,
            "auto_save" => snapshot.auto_save,
            "session_history" => snapshot.session_history,
            // Privacy
            "telemetry" => snapshot.telemetry,
            "analytics" => snapshot.analytics,
            _ => false,
        };

        let status = if is_enabled { "Enabled" } else { "Disabled" };
        let icon = if is_enabled { '>' } else { ' ' };

        let item = InteractiveItem::new(setting.id, setting.label)
            .with_description(format!("{} ({})", setting.description, status))
            .with_icon(icon);

        items.push(item);
    }

    // Build tabs
    let tabs: Vec<NavTab> = TABS
        .iter()
        .map(|(id, label, _)| NavTab::new(*id, *label))
        .collect();

    // Calculate max_visible dynamically
    const UI_CHROME_HEIGHT: u16 = 6;
    let total_items = items.len();
    let max_visible = match terminal_height {
        Some(height) => {
            let available = height.saturating_sub(UI_CHROME_HEIGHT) as usize;
            available.clamp(8, total_items)
        }
        None => total_items.min(40),
    };

    let mut state = InteractiveState::new("Settings", items, InteractiveAction::ToggleSetting)
        .with_max_visible(max_visible)
        .with_tabs(tabs);
    state.active_tab = active_tab;
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_settings_selector() {
        let snapshot = SettingsSnapshot::default();
        let state = build_settings_selector(snapshot, None);
        assert!(!state.items.is_empty());
        assert_eq!(state.title, "Settings");
    }

    #[test]
    fn test_settings_categories() {
        let snapshot = SettingsSnapshot::default();
        let state = build_settings_selector(snapshot, None);

        // Check that category separators exist
        let categories: Vec<_> = state
            .items
            .iter()
            .filter(|i| i.id.starts_with("__cat_"))
            .collect();
        assert!(!categories.is_empty());
    }

    #[test]
    fn test_max_visible_dynamic_calculation() {
        let snapshot = SettingsSnapshot::default();

        // Small terminal - should clamp to minimum of 8
        let state_small = build_settings_selector(snapshot.clone(), Some(12));
        assert!(state_small.max_visible >= 8);

        // Large terminal - should show all items
        let state_large = build_settings_selector(snapshot.clone(), Some(100));
        assert_eq!(state_large.max_visible, state_large.items.len());

        // No terminal height - should default to showing all items
        let state_default = build_settings_selector(snapshot, None);
        assert_eq!(state_default.max_visible, state_default.items.len());
    }
}
