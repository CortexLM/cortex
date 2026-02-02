//! Model presets and provider information.
//!
//! This module provides:
//! - Model preset definitions for various AI providers
//! - Model aliases for common shortcuts
//! - Resolution utilities for model name lookups

mod aliases;
mod constants;
mod presets;
mod resolution;
mod types;

// Re-export types
pub use types::{ModelAlias, ModelPreset, ModelResolution};

// Re-export constants
pub use constants::{DEFAULT_MODEL, DEFAULT_MODELS, DEFAULT_PROVIDER};

// Re-export preset data and helpers
pub use presets::{MODEL_PRESETS, get_model_preset, get_models_for_provider};

// Re-export alias data and helpers
pub use aliases::{MODEL_ALIASES, list_model_aliases, resolve_model_alias};

// Re-export resolution functions
pub use resolution::{resolve_model_with_info, warn_if_ambiguous_model};
