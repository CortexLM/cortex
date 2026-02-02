//! Model resolution with detailed information and ambiguity handling.

use super::aliases::MODEL_ALIASES;
use super::presets::MODEL_PRESETS;
use super::types::ModelResolution;

/// Resolves a model name with detailed information about the resolution.
///
/// This function:
/// 1. First checks for exact alias matches (e.g., "sonnet" -> "anthropic/claude-sonnet-4-20250514")
/// 2. Then checks for exact preset matches
/// 3. Finally checks for partial matches and provides warnings if ambiguous
///
/// # Returns
/// A `ModelResolution` containing the resolved model and metadata about how it was resolved.
pub fn resolve_model_with_info(model: &str) -> ModelResolution {
    // First check for exact alias match
    if let Some(alias) = MODEL_ALIASES
        .iter()
        .find(|a| a.alias.eq_ignore_ascii_case(model))
    {
        return ModelResolution {
            model: alias.model.to_string(),
            was_alias: true,
            was_partial_match: false,
            other_matches: vec![],
        };
    }

    // Check for exact preset match
    if MODEL_PRESETS.iter().any(|p| p.id == model) {
        return ModelResolution {
            model: model.to_string(),
            was_alias: false,
            was_partial_match: false,
            other_matches: vec![],
        };
    }

    // Check for partial matches (model ID starts with or contains the input)
    let model_lower = model.to_lowercase();
    let partial_matches: Vec<_> = MODEL_PRESETS
        .iter()
        .filter(|p| {
            let id_lower = p.id.to_lowercase();
            id_lower.starts_with(&model_lower) || id_lower.contains(&model_lower)
        })
        .collect();

    if partial_matches.is_empty() {
        // No matches found, return input as-is
        return ModelResolution {
            model: model.to_string(),
            was_alias: false,
            was_partial_match: false,
            other_matches: vec![],
        };
    }

    if partial_matches.len() == 1 {
        // Single partial match
        return ModelResolution {
            model: partial_matches[0].id.to_string(),
            was_alias: false,
            was_partial_match: true,
            other_matches: vec![],
        };
    }

    // Multiple matches - sort by ID length (prefer shorter/more specific) and return first
    // but include all matches in other_matches for warning purposes
    let mut sorted_matches = partial_matches;
    sorted_matches.sort_by_key(|p| p.id.len());

    let selected = sorted_matches[0].id.to_string();
    let other_matches: Vec<String> = sorted_matches[1..]
        .iter()
        .take(5) // Limit to 5 other matches for readability
        .map(|p| p.id.to_string())
        .collect();

    ModelResolution {
        model: selected,
        was_alias: false,
        was_partial_match: true,
        other_matches,
    }
}

/// Prints a warning to stderr if model resolution was ambiguous.
///
/// Call this after `resolve_model_with_info` to inform users about partial matches.
pub fn warn_if_ambiguous_model(resolution: &ModelResolution, input: &str) {
    if resolution.was_partial_match && !resolution.other_matches.is_empty() {
        eprintln!(
            "Warning: Multiple models match '{}'. Using '{}'. Specify exact name to avoid ambiguity.",
            input, resolution.model
        );
        eprintln!("Other matches: {}", resolution.other_matches.join(", "));
    } else if resolution.was_partial_match {
        eprintln!(
            "Note: '{}' resolved to '{}' via partial match.",
            input, resolution.model
        );
    }
}
