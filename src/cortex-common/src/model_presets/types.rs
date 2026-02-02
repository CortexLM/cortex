//! Type definitions for model presets.

/// Model preset information.
#[derive(Debug, Clone)]
pub struct ModelPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub provider: &'static str,
    pub context_window: i64,
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_reasoning: bool,
}

/// Model alias entry mapping a short name to a full model identifier.
#[derive(Debug, Clone, Copy)]
pub struct ModelAlias {
    /// Short alias (e.g., "sonnet").
    pub alias: &'static str,
    /// Full model identifier (e.g., "anthropic/claude-sonnet-4-20250514").
    pub model: &'static str,
}

/// Result of model resolution with additional metadata.
#[derive(Debug, Clone)]
pub struct ModelResolution {
    /// The resolved model identifier.
    pub model: String,
    /// Whether the input was an exact alias match.
    pub was_alias: bool,
    /// Whether the input was a partial match (for warning purposes).
    pub was_partial_match: bool,
    /// Other models that also matched (for ambiguity warnings).
    pub other_matches: Vec<String>,
}
