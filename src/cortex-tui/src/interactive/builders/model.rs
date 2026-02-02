//! Builder for model selection.

use crate::interactive::state::{InteractiveAction, InteractiveItem, InteractiveState};
use crate::providers::models::ModelInfo;

/// Build an interactive state for model selection.
/// Models should be passed from ProviderManager.available_models().
pub fn build_model_selector(
    models: Vec<ModelInfo>,
    current_model: Option<&str>,
) -> InteractiveState {
    let mut items: Vec<InteractiveItem> = models
        .iter()
        .map(|model| {
            let is_current = current_model.map(|c| c == model.id).unwrap_or(false);

            let description = format_model_description(model);

            InteractiveItem::new(&model.id, &model.name)
                .with_description(description)
                .with_current(is_current)
                .with_metadata(model.id.clone())
        })
        .collect();

    // Sort: current first, then by name
    items.sort_by(|a, b| {
        if a.is_current && !b.is_current {
            std::cmp::Ordering::Less
        } else if !a.is_current && b.is_current {
            std::cmp::Ordering::Greater
        } else {
            a.label.cmp(&b.label)
        }
    });

    let title = "Select Model".to_string();

    InteractiveState::new(title, items, InteractiveAction::SetModel)
        .with_search()
        .with_max_visible(15)
}

/// Format a model description showing context window and other info.
fn format_model_description(model: &ModelInfo) -> String {
    let mut parts = Vec::new();

    // Context window
    let ctx = model.context_window;
    let ctx_str = if ctx >= 1_000_000 {
        format!("{}M ctx", ctx / 1_000_000)
    } else if ctx >= 1_000 {
        format!("{}K ctx", ctx / 1_000)
    } else {
        format!("{} ctx", ctx)
    };
    parts.push(ctx_str);

    // Capabilities
    if model.vision {
        parts.push("vision".to_string());
    }
    if model.tools {
        parts.push("tools".to_string());
    }

    parts.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_model_selector() {
        let state = build_model_selector(Vec::new(), None);
        // May be empty if no models configured, but should not panic
        assert_eq!(state.title, "Select Model");
        assert!(state.searchable);
    }
}
