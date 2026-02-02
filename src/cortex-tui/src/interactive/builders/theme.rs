//! Builder for theme selection.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

struct ThemeDef {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

const THEMES: &[ThemeDef] = &[
    ThemeDef {
        id: "ocean",
        label: "Ocean",
        description: "Deep blue theme - default",
    },
    ThemeDef {
        id: "midnight",
        label: "Midnight",
        description: "Dark purple theme",
    },
    ThemeDef {
        id: "forest",
        label: "Forest",
        description: "Dark green theme",
    },
    ThemeDef {
        id: "monochrome",
        label: "Monochrome",
        description: "Black and white",
    },
];

/// Build an interactive state for theme selection.
pub fn build_theme_selector(current: Option<&str>) -> InteractiveState {
    let current_theme = current.unwrap_or("ocean");

    let items: Vec<InteractiveItem> = THEMES
        .iter()
        .map(|t| {
            let is_current = t.id == current_theme;
            InteractiveItem::new(t.id, t.label)
                .with_description(t.description)
                .with_current(is_current)
                .with_icon(if is_current { '>' } else { ' ' })
        })
        .collect();

    InteractiveState::new("Theme", items, InteractiveAction::Custom("theme".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_theme_selector() {
        let state = build_theme_selector(Some("ocean"));
        assert!(!state.items.is_empty());
        assert_eq!(state.title, "Theme");

        let current = state.items.iter().find(|i| i.is_current);
        assert!(current.is_some());
        assert_eq!(current.unwrap().id, "ocean");
    }
}
