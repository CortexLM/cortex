//! Model alias definitions and resolution functions.

use super::types::ModelAlias;

/// Built-in model aliases for common shortcuts.
pub const MODEL_ALIASES: &[ModelAlias] = &[
    // Claude models
    ModelAlias {
        alias: "claude",
        model: "anthropic/claude-opus-4.5",
    },
    ModelAlias {
        alias: "opus",
        model: "anthropic/claude-opus-4.5",
    },
    ModelAlias {
        alias: "sonnet",
        model: "anthropic/claude-sonnet-4-20250514",
    },
    ModelAlias {
        alias: "haiku",
        model: "anthropic/claude-haiku-4.5",
    },
    // OpenAI models
    ModelAlias {
        alias: "gpt4",
        model: "openai/gpt-4o",
    },
    ModelAlias {
        alias: "gpt",
        model: "openai/gpt-4o",
    },
    ModelAlias {
        alias: "o1",
        model: "openai/o1",
    },
    ModelAlias {
        alias: "o3",
        model: "openai/o3",
    },
    // Google models
    ModelAlias {
        alias: "gemini",
        model: "google/gemini-2.5-pro-preview-06-05",
    },
    // DeepSeek models
    ModelAlias {
        alias: "deepseek",
        model: "deepseek/deepseek-chat",
    },
    ModelAlias {
        alias: "r1",
        model: "deepseek/deepseek-r1",
    },
    // Meta models
    ModelAlias {
        alias: "llama",
        model: "meta-llama/llama-3.3-70b-instruct",
    },
];

/// Resolves a model alias to its full model identifier.
///
/// If the input matches a known alias, returns the corresponding full model name.
/// Otherwise, returns the input unchanged.
///
/// # Examples
///
/// ```
/// use cortex_common::resolve_model_alias;
///
/// assert_eq!(resolve_model_alias("sonnet"), "anthropic/claude-sonnet-4-20250514");
/// assert_eq!(resolve_model_alias("gpt4"), "openai/gpt-4o");
/// assert_eq!(resolve_model_alias("unknown-model"), "unknown-model");
/// ```
pub fn resolve_model_alias(model: &str) -> &str {
    MODEL_ALIASES
        .iter()
        .find(|a| a.alias.eq_ignore_ascii_case(model))
        .map(|a| a.model)
        .unwrap_or(model)
}

/// Returns a list of all available model aliases.
pub fn list_model_aliases() -> &'static [ModelAlias] {
    MODEL_ALIASES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_model_alias_known() {
        assert_eq!(
            resolve_model_alias("sonnet"),
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(resolve_model_alias("opus"), "anthropic/claude-opus-4.5");
        assert_eq!(resolve_model_alias("gpt4"), "openai/gpt-4o");
        assert_eq!(resolve_model_alias("haiku"), "anthropic/claude-haiku-4.5");
        assert_eq!(resolve_model_alias("r1"), "deepseek/deepseek-r1");
    }

    #[test]
    fn test_resolve_model_alias_case_insensitive() {
        assert_eq!(
            resolve_model_alias("SONNET"),
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(
            resolve_model_alias("Sonnet"),
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(resolve_model_alias("GPT4"), "openai/gpt-4o");
    }

    #[test]
    fn test_resolve_model_alias_unknown() {
        assert_eq!(resolve_model_alias("unknown-model"), "unknown-model");
        assert_eq!(
            resolve_model_alias("anthropic/claude-3-opus"),
            "anthropic/claude-3-opus"
        );
    }

    #[test]
    fn test_list_model_aliases() {
        let aliases = list_model_aliases();
        assert!(!aliases.is_empty());
        assert!(aliases.iter().any(|a| a.alias == "sonnet"));
    }
}
