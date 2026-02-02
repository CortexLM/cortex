//! Comprehensive tests for config_types module.

use crate::config_types::*;

#[test]
fn test_reasoning_effort_default() {
    let effort = ReasoningEffort::default();
    assert_eq!(effort, ReasoningEffort::Medium);
}

#[test]
fn test_reasoning_effort_all_variants() {
    let variants = vec![
        ReasoningEffort::Low,
        ReasoningEffort::Medium,
        ReasoningEffort::High,
    ];

    let expected_json = vec!["\"low\"", "\"medium\"", "\"high\""];

    for (variant, expected) in variants.into_iter().zip(expected_json.into_iter()) {
        let json = serde_json::to_string(&variant).expect("serialize");
        assert_eq!(json, expected);

        let parsed: ReasoningEffort = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, parsed);
    }
}

#[test]
fn test_reasoning_summary_default() {
    let summary = ReasoningSummary::default();
    assert_eq!(summary, ReasoningSummary::None);
}

#[test]
fn test_reasoning_summary_all_variants() {
    let variants = vec![
        ReasoningSummary::None,
        ReasoningSummary::Brief,
        ReasoningSummary::Detailed,
        ReasoningSummary::Auto,
    ];

    let expected_json = vec!["\"none\"", "\"brief\"", "\"detailed\"", "\"auto\""];

    for (variant, expected) in variants.into_iter().zip(expected_json.into_iter()) {
        let json = serde_json::to_string(&variant).expect("serialize");
        assert_eq!(json, expected);

        let parsed: ReasoningSummary = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, parsed);
    }
}

#[test]
fn test_sandbox_mode_default() {
    let mode = SandboxMode::default();
    // Default is now DangerFullAccess for Cortex
    assert_eq!(mode, SandboxMode::DangerFullAccess);
}

#[test]
fn test_sandbox_mode_all_variants() {
    let variants = vec![
        SandboxMode::DangerFullAccess,
        SandboxMode::ReadOnly,
        SandboxMode::WorkspaceWrite,
    ];

    let expected_json = vec![
        "\"danger-full-access\"",
        "\"read-only\"",
        "\"workspace-write\"",
    ];

    for (variant, expected) in variants.into_iter().zip(expected_json.into_iter()) {
        let json = serde_json::to_string(&variant).expect("serialize");
        assert_eq!(json, expected);

        let parsed: SandboxMode = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(variant, parsed);
    }
}

#[test]
fn test_trust_level_serialization() {
    let trusted = TrustLevel::Trusted;
    let untrusted = TrustLevel::Untrusted;

    let trusted_json = serde_json::to_string(&trusted).expect("serialize");
    let untrusted_json = serde_json::to_string(&untrusted).expect("serialize");

    assert_eq!(trusted_json, "\"trusted\"");
    assert_eq!(untrusted_json, "\"untrusted\"");
}

#[test]
fn test_trust_level_display() {
    assert_eq!(format!("{}", TrustLevel::Trusted), "trusted");
    assert_eq!(format!("{}", TrustLevel::Untrusted), "untrusted");
}

#[test]
fn test_forced_login_method_serialization() {
    let api_key = ForcedLoginMethod::ApiKey;
    let chat_gpt = ForcedLoginMethod::ChatGpt;

    let api_key_json = serde_json::to_string(&api_key).expect("serialize");
    let chat_gpt_json = serde_json::to_string(&chat_gpt).expect("serialize");

    assert_eq!(api_key_json, "\"api-key\"");
    assert_eq!(chat_gpt_json, "\"chat-gpt\"");

    let parsed_api_key: ForcedLoginMethod =
        serde_json::from_str(&api_key_json).expect("deserialize");
    let parsed_chat_gpt: ForcedLoginMethod =
        serde_json::from_str(&chat_gpt_json).expect("deserialize");

    assert_eq!(parsed_api_key, ForcedLoginMethod::ApiKey);
    assert_eq!(parsed_chat_gpt, ForcedLoginMethod::ChatGpt);
}

#[test]
fn test_verbosity_serialization() {
    let terse = Verbosity::Terse;
    let normal = Verbosity::Normal;
    let verbose = Verbosity::Verbose;

    assert_eq!(
        serde_json::to_string(&terse).expect("serialize"),
        "\"terse\""
    );
    assert_eq!(
        serde_json::to_string(&normal).expect("serialize"),
        "\"normal\""
    );
    assert_eq!(
        serde_json::to_string(&verbose).expect("serialize"),
        "\"verbose\""
    );
}

#[test]
fn test_reasoning_summary_format_default() {
    let format = ReasoningSummaryFormat::default();
    assert_eq!(format, ReasoningSummaryFormat::Text);
}

#[test]
fn test_reasoning_summary_format_all_variants() {
    let text = ReasoningSummaryFormat::Text;
    let structured = ReasoningSummaryFormat::Structured;

    assert_eq!(serde_json::to_string(&text).expect("serialize"), "\"text\"");
    assert_eq!(
        serde_json::to_string(&structured).expect("serialize"),
        "\"structured\""
    );
}

#[test]
fn test_reasoning_effort_equality() {
    assert_eq!(ReasoningEffort::Low, ReasoningEffort::Low);
    assert_ne!(ReasoningEffort::Low, ReasoningEffort::High);
}

#[test]
fn test_reasoning_summary_equality() {
    assert_eq!(ReasoningSummary::Brief, ReasoningSummary::Brief);
    assert_ne!(ReasoningSummary::Brief, ReasoningSummary::Detailed);
}

#[test]
fn test_sandbox_mode_equality() {
    assert_eq!(SandboxMode::ReadOnly, SandboxMode::ReadOnly);
    assert_ne!(SandboxMode::ReadOnly, SandboxMode::WorkspaceWrite);
}

#[test]
fn test_trust_level_equality() {
    assert_eq!(TrustLevel::Trusted, TrustLevel::Trusted);
    assert_ne!(TrustLevel::Trusted, TrustLevel::Untrusted);
}

#[test]
fn test_config_types_clone() {
    let effort = ReasoningEffort::High;
    let cloned = effort;
    assert_eq!(effort, cloned);

    let summary = ReasoningSummary::Detailed;
    let cloned_summary = summary;
    assert_eq!(summary, cloned_summary);
}

#[test]
fn test_config_types_copy() {
    let effort = ReasoningEffort::Medium;
    let copied = effort; // Copy trait
    assert_eq!(effort, copied);

    let mode = SandboxMode::ReadOnly;
    let copied_mode = mode;
    assert_eq!(mode, copied_mode);
}

#[test]
fn test_config_types_debug() {
    let effort = ReasoningEffort::High;
    let debug = format!("{:?}", effort);
    assert!(debug.contains("High"));

    let summary = ReasoningSummary::Brief;
    let debug = format!("{:?}", summary);
    assert!(debug.contains("Brief"));
}

#[test]
fn test_case_insensitive_deserialization_effort() {
    // Test that case matters for deserialization (lowercase expected)
    let result: Result<ReasoningEffort, _> = serde_json::from_str("\"low\"");
    assert!(result.is_ok());

    let result: Result<ReasoningEffort, _> = serde_json::from_str("\"LOW\"");
    assert!(result.is_err()); // Should fail with uppercase
}

#[test]
fn test_invalid_value_deserialization() {
    let result: Result<ReasoningEffort, _> = serde_json::from_str("\"invalid\"");
    assert!(result.is_err());

    let result: Result<SandboxMode, _> = serde_json::from_str("\"not-a-mode\"");
    assert!(result.is_err());
}
