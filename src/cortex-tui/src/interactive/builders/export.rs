//! Builder for export format selection.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};

struct ExportFormat {
    id: &'static str,
    label: &'static str,
    description: &'static str,
}

const FORMATS: &[ExportFormat] = &[
    ExportFormat {
        id: "markdown",
        label: "Markdown",
        description: "Export as .md file",
    },
    ExportFormat {
        id: "json",
        label: "JSON",
        description: "Export as .json file",
    },
    ExportFormat {
        id: "txt",
        label: "Text",
        description: "Export as plain .txt file",
    },
];

/// Build an interactive state for export format selection.
pub fn build_export_selector() -> InteractiveState {
    let items: Vec<InteractiveItem> = FORMATS
        .iter()
        .map(|f| InteractiveItem::new(f.id, f.label).with_description(f.description))
        .collect();

    InteractiveState::new(
        "Export Format",
        items,
        InteractiveAction::Custom("export".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_export_selector() {
        let state = build_export_selector();
        assert_eq!(state.items.len(), 3);
        assert_eq!(state.title, "Export Format");
    }
}
