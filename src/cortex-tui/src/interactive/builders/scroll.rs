//! Builder for scroll navigation.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

struct ScrollOption {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

const SCROLL_OPTIONS: &[ScrollOption] = &[
    ScrollOption {
        id: "top",
        label: "Top",
        description: "Scroll to top of chat",
    },
    ScrollOption {
        id: "bottom",
        label: "Bottom",
        description: "Scroll to bottom of chat",
    },
    ScrollOption {
        id: "up10",
        label: "Up 10",
        description: "Scroll up 10 lines",
    },
    ScrollOption {
        id: "down10",
        label: "Down 10",
        description: "Scroll down 10 lines",
    },
    ScrollOption {
        id: "pageup",
        label: "Page Up",
        description: "Scroll up one page",
    },
    ScrollOption {
        id: "pagedown",
        label: "Page Down",
        description: "Scroll down one page",
    },
];

/// Build an interactive state for scroll navigation.
pub fn build_scroll_selector() -> InteractiveState {
    let items: Vec<InteractiveItem> = SCROLL_OPTIONS
        .iter()
        .map(|s| InteractiveItem::new(s.id, s.label).with_description(s.description))
        .collect();

    InteractiveState::new("Scroll", items, InteractiveAction::Custom("scroll".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_scroll_selector() {
        let state = build_scroll_selector();
        assert!(!state.items.is_empty());
        assert_eq!(state.title, "Scroll");
    }
}
