//! Comprehensive tests for model presets.

use crate::model_presets::*;

#[test]
fn test_default_model() {
    assert_eq!(DEFAULT_MODEL, "claude-opus-4-5-20251101");
}

#[test]
fn test_default_provider() {
    assert_eq!(DEFAULT_PROVIDER, "cortex");
}

#[test]
fn test_model_presets_not_empty() {
    assert!(!MODEL_PRESETS.is_empty());
}

#[test]
fn test_get_model_preset_existing() {
    let preset = get_model_preset("gpt-4o");
    assert!(preset.is_some());

    let preset = preset.unwrap();
    assert_eq!(preset.id, "gpt-4o");
    assert_eq!(preset.provider, "openai");
    assert!(preset.context_window > 0);
}

#[test]
fn test_get_model_preset_nonexistent() {
    let preset = get_model_preset("nonexistent-model");
    assert!(preset.is_none());
}

#[test]
fn test_get_models_for_provider_openai() {
    let openai_models = get_models_for_provider("openai");

    assert!(!openai_models.is_empty());
    for model in openai_models {
        assert_eq!(model.provider, "openai");
    }
}

#[test]
fn test_get_models_for_provider_anthropic() {
    let anthropic_models = get_models_for_provider("anthropic");

    assert!(!anthropic_models.is_empty());
    for model in anthropic_models {
        assert_eq!(model.provider, "anthropic");
    }
}

#[test]
fn test_get_models_for_provider_nonexistent() {
    let models = get_models_for_provider("nonexistent-provider");
    assert!(models.is_empty());
}

#[test]
fn test_all_presets_have_valid_data() {
    for preset in MODEL_PRESETS {
        // All presets should have non-empty ID
        assert!(!preset.id.is_empty(), "Preset has empty ID");

        // All presets should have non-empty name
        assert!(
            !preset.name.is_empty(),
            "Preset {} has empty name",
            preset.id
        );

        // All presets should have non-empty provider
        assert!(
            !preset.provider.is_empty(),
            "Preset {} has empty provider",
            preset.id
        );

        // Context window should be positive
        assert!(
            preset.context_window > 0,
            "Preset {} has invalid context window",
            preset.id
        );
    }
}

#[test]
fn test_gpt4o_preset() {
    let preset = get_model_preset("gpt-4o").expect("gpt-4o should exist");

    assert_eq!(preset.name, "GPT-4o");
    assert_eq!(preset.provider, "openai");
    assert_eq!(preset.context_window, 128_000);
    assert!(preset.supports_vision);
    assert!(preset.supports_tools);
    assert!(!preset.supports_reasoning);
}

#[test]
fn test_gpt4o_mini_preset() {
    let preset = get_model_preset("gpt-4o-mini").expect("gpt-4o-mini should exist");

    assert_eq!(preset.name, "GPT-4o Mini");
    assert_eq!(preset.provider, "openai");
    assert!(preset.supports_vision);
    assert!(preset.supports_tools);
}

#[test]
fn test_o1_preset() {
    let preset = get_model_preset("o1").expect("o1 should exist");

    assert_eq!(preset.provider, "openai");
    assert!(preset.supports_reasoning);
    assert!(preset.supports_tools);
}

#[test]
fn test_claude_presets() {
    let sonnet = get_model_preset("claude-3-5-sonnet").expect("claude-3-5-sonnet should exist");
    let opus = get_model_preset("claude-3-opus").expect("claude-3-opus should exist");

    assert_eq!(sonnet.provider, "anthropic");
    assert_eq!(opus.provider, "anthropic");

    // Both should support vision and tools
    assert!(sonnet.supports_vision);
    assert!(sonnet.supports_tools);
    assert!(opus.supports_vision);
    assert!(opus.supports_tools);
}

#[test]
fn test_preset_uniqueness() {
    use std::collections::HashSet;

    let mut ids = HashSet::new();
    for preset in MODEL_PRESETS {
        assert!(ids.insert(preset.id), "Duplicate preset ID: {}", preset.id);
    }
}

#[test]
fn test_model_preset_debug() {
    let preset = get_model_preset("gpt-4o").unwrap();
    let debug = format!("{:?}", preset);

    assert!(debug.contains("gpt-4o"));
    assert!(debug.contains("GPT-4o"));
}

#[test]
fn test_model_preset_clone() {
    let preset = get_model_preset("gpt-4o").unwrap();
    let cloned = preset.clone();

    assert_eq!(preset.id, cloned.id);
    assert_eq!(preset.name, cloned.name);
    assert_eq!(preset.provider, cloned.provider);
}
