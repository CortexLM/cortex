//! Builders for approval mode and log level selection.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

/// Build an interactive state for approval mode selection.
pub fn build_approval_selector(current: Option<&str>) -> InteractiveState {
    let items = vec![
        InteractiveItem::new("ask", "Ask")
            .with_icon('?')
            .with_shortcut('a')
            .with_description("Ask for each tool call")
            .with_current(current == Some("ask")),
        InteractiveItem::new("session", "Session")
            .with_icon('S')
            .with_shortcut('s')
            .with_description("Remember choice for this session")
            .with_current(current == Some("session")),
        InteractiveItem::new("always", "Always")
            .with_icon('Y')
            .with_shortcut('y')
            .with_description("Always approve automatically")
            .with_current(current == Some("always")),
        InteractiveItem::new("never", "Never")
            .with_icon('N')
            .with_shortcut('n')
            .with_description("Never approve (reject all)")
            .with_current(current == Some("never")),
    ];

    InteractiveState::new("Approval Mode", items, InteractiveAction::SetApprovalMode)
}

/// Build an interactive state for log level selection.
pub fn build_log_level_selector(current: Option<&str>) -> InteractiveState {
    let items = vec![
        InteractiveItem::new("trace", "Trace")
            .with_icon('T')
            .with_shortcut('t')
            .with_description("Most verbose - all details")
            .with_current(current == Some("trace")),
        InteractiveItem::new("debug", "Debug")
            .with_icon('D')
            .with_shortcut('d')
            .with_description("Debug information")
            .with_current(current == Some("debug")),
        InteractiveItem::new("info", "Info")
            .with_icon('I')
            .with_shortcut('i')
            .with_description("General information")
            .with_current(current == Some("info")),
        InteractiveItem::new("warn", "Warn")
            .with_icon('W')
            .with_shortcut('w')
            .with_description("Warnings only")
            .with_current(current == Some("warn")),
        InteractiveItem::new("error", "Error")
            .with_icon('E')
            .with_shortcut('e')
            .with_description("Errors only")
            .with_current(current == Some("error")),
    ];

    InteractiveState::new("Log Level", items, InteractiveAction::SetLogLevel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_approval_selector() {
        let state = build_approval_selector(Some("ask"));
        assert_eq!(state.items.len(), 4);
        assert!(state.items[0].is_current);
    }

    #[test]
    fn test_build_log_level_selector() {
        let state = build_log_level_selector(Some("info"));
        assert_eq!(state.items.len(), 5);
        assert!(state.items[2].is_current); // info is at index 2
    }
}
