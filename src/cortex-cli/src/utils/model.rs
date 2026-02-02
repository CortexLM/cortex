//! Model name validation and resolution utilities.
//!
//! Provides centralized model handling functionality used across CLI commands.

use anyhow::{Result, bail};
use cortex_common::{resolve_model_alias, resolve_model_with_info, warn_if_ambiguous_model};

/// Known LLM providers for validation.
pub const KNOWN_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "mistral",
    "xai",
    "deepseek",
    "groq",
    "lmstudio",
    "llamacpp",
    "vllm",
    "openrouter",
];

/// Known model name patterns that do not support streaming.
pub const NON_STREAMING_PATTERNS: &[&str] = &[
    "embedding",
    "text-embedding",
    "ada-002",
    "text-search",
    "text-similarity",
];

/// Validated and resolved model information.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// The resolved model name (after alias expansion).
    pub name: String,
    /// The provider (if in provider/model format).
    pub provider: Option<String>,
    /// Whether the model appears to be an embedding model.
    pub is_embedding: bool,
    /// Whether the provider is recognized.
    pub is_known_provider: bool,
}

/// Validate and resolve a model name.
///
/// Performs the following:
/// - Resolves aliases (e.g., "sonnet" -> "anthropic/claude-sonnet-4-20250514")
/// - Validates format (provider/model or simple name)
/// - Warns about unknown providers
/// - Detects embedding models
///
/// # Arguments
/// * `model` - The model name or alias to validate
///
/// # Returns
/// The resolved model information, or an error if invalid.
pub fn validate_and_resolve_model(model: &str) -> Result<ResolvedModel> {
    // Check for empty model name
    if model.trim().is_empty() {
        bail!(
            "Model name cannot be empty. Please provide a valid model name \
             (e.g., 'gpt-4', 'claude-sonnet-4-20250514')."
        );
    }

    // Resolve any alias
    let resolved = resolve_model_alias(model);

    // Parse provider/model format
    let (provider, model_name) = if resolved.contains('/') {
        let parts: Vec<&str> = resolved.splitn(2, '/').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            bail!(
                "Invalid model format: '{}'. Expected 'provider/model' format.\n\
                 Examples: anthropic/claude-sonnet-4-20250514, openai/gpt-4o\n\
                 Run 'cortex models list' to see available models.",
                model
            );
        }
        (Some(parts[0].to_lowercase()), parts[1].to_string())
    } else {
        // Validate model name characters
        let valid_chars = resolved
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' || c == ':');
        if !valid_chars {
            bail!(
                "Invalid model name: '{}'. Model names should contain only alphanumeric \
                 characters, hyphens, underscores, dots, and colons.\n\
                 Run 'cortex models list' to see available models.",
                model
            );
        }
        (None, resolved.to_string())
    };

    // Check if provider is known
    let is_known_provider = provider
        .as_ref()
        .map(|p| KNOWN_PROVIDERS.contains(&p.as_str()))
        .unwrap_or(true);

    // Log warning for unknown providers (don't block)
    if let Some(ref p) = provider
        && !is_known_provider
    {
        tracing::warn!(
            "Unknown provider '{}'. Known providers: {}",
            p,
            KNOWN_PROVIDERS.join(", ")
        );
    }

    // Detect if this is an embedding model
    let model_lower = model_name.to_lowercase();
    let is_embedding = NON_STREAMING_PATTERNS
        .iter()
        .any(|p| model_lower.contains(p));

    Ok(ResolvedModel {
        name: resolved.to_string(),
        provider,
        is_embedding,
        is_known_provider,
    })
}

/// Resolve model with ambiguity warning.
///
/// This is a convenience wrapper that handles the common case of resolving
/// a model and warning the user if the name was ambiguous.
///
/// # Arguments
/// * `model` - The model name or alias to resolve
///
/// # Returns
/// The resolved model name.
pub fn resolve_model_with_warning(model: &str) -> String {
    let resolution = resolve_model_with_info(model);
    warn_if_ambiguous_model(&resolution, model);
    resolution.model.clone()
}

/// Check if a model supports streaming output.
///
/// # Arguments
/// * `model` - The model name to check
///
/// # Returns
/// `true` if the model likely supports streaming, `false` for embedding models.
pub fn supports_streaming(model: &str) -> bool {
    let model_lower = model.to_lowercase();
    !NON_STREAMING_PATTERNS
        .iter()
        .any(|p| model_lower.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_empty_model() {
        assert!(validate_and_resolve_model("").is_err());
        assert!(validate_and_resolve_model("   ").is_err());
    }

    #[test]
    fn test_validate_provider_model_format() {
        let result = validate_and_resolve_model("anthropic/claude-3");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert_eq!(resolved.provider, Some("anthropic".to_string()));
        assert!(resolved.is_known_provider);
    }

    #[test]
    fn test_validate_simple_model_name() {
        let result = validate_and_resolve_model("gpt-4");
        assert!(result.is_ok());
        let resolved = result.unwrap();
        assert!(resolved.provider.is_none());
    }

    #[test]
    fn test_detect_embedding_model() {
        let result = validate_and_resolve_model("text-embedding-ada-002");
        assert!(result.is_ok());
        assert!(result.unwrap().is_embedding);
    }

    #[test]
    fn test_supports_streaming() {
        assert!(supports_streaming("gpt-4"));
        assert!(supports_streaming("claude-3"));
        assert!(!supports_streaming("text-embedding-ada-002"));
    }
}
