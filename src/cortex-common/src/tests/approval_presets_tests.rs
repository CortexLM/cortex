//! Comprehensive tests for approval presets.

use crate::approval_presets::*;
use cortex_protocol::{AskForApproval, SandboxPolicy};

#[test]
fn test_presets_not_empty() {
    assert!(!PRESETS.is_empty());
}

#[test]
fn test_get_preset_suggest() {
    let preset = get_preset("suggest");
    assert!(preset.is_some());

    let preset = preset.unwrap();
    assert_eq!(preset.name, "suggest");
    assert_eq!(preset.approval_policy, AskForApproval::UnlessTrusted);
    assert!(matches!(preset.sandbox_policy, SandboxPolicy::ReadOnly));
}

#[test]
fn test_get_preset_auto_edit() {
    let preset = get_preset("auto-edit");
    assert!(preset.is_some());

    let preset = preset.unwrap();
    assert_eq!(preset.name, "auto-edit");
    assert_eq!(preset.approval_policy, AskForApproval::OnRequest);
    assert!(matches!(
        preset.sandbox_policy,
        SandboxPolicy::WorkspaceWrite { .. }
    ));
}

#[test]
fn test_get_preset_full_auto() {
    let preset = get_preset("full-auto");
    assert!(preset.is_some());

    let preset = preset.unwrap();
    assert_eq!(preset.name, "full-auto");
    assert_eq!(preset.approval_policy, AskForApproval::OnFailure);

    if let SandboxPolicy::WorkspaceWrite { network_access, .. } = preset.sandbox_policy {
        assert!(network_access, "full-auto should have network access");
    } else {
        panic!("full-auto should use WorkspaceWrite policy");
    }
}

#[test]
fn test_get_preset_nonexistent() {
    let preset = get_preset("nonexistent");
    assert!(preset.is_none());
}

#[test]
fn test_default_preset() {
    let preset = default_preset();

    // Default is auto-edit
    assert_eq!(preset.name, "auto-edit");
}

#[test]
fn test_all_presets_have_valid_data() {
    for preset in PRESETS {
        // All presets should have non-empty name
        assert!(!preset.name.is_empty(), "Preset has empty name");

        // All presets should have non-empty description
        assert!(
            !preset.description.is_empty(),
            "Preset {} has empty description",
            preset.name
        );
    }
}

#[test]
fn test_preset_uniqueness() {
    use std::collections::HashSet;

    let mut names = HashSet::new();
    for preset in PRESETS {
        assert!(
            names.insert(preset.name),
            "Duplicate preset name: {}",
            preset.name
        );
    }
}

#[test]
fn test_preset_suggest_description() {
    let preset = get_preset("suggest").unwrap();
    assert!(preset.description.contains("suggest") || preset.description.contains("approve"));
}

#[test]
fn test_preset_auto_edit_description() {
    let preset = get_preset("auto-edit").unwrap();
    assert!(preset.description.contains("edit") || preset.description.contains("file"));
}

#[test]
fn test_preset_full_auto_description() {
    let preset = get_preset("full-auto").unwrap();
    assert!(preset.description.contains("auto") || preset.description.contains("sandbox"));
}

#[test]
fn test_approval_preset_debug() {
    let preset = get_preset("suggest").unwrap();
    let debug = format!("{:?}", preset);

    assert!(debug.contains("suggest"));
    assert!(debug.contains("ApprovalPreset"));
}

#[test]
fn test_approval_preset_clone() {
    let preset = get_preset("auto-edit").unwrap();
    let cloned = preset.clone();

    assert_eq!(preset.name, cloned.name);
    assert_eq!(preset.description, cloned.description);
}

#[test]
fn test_suggest_is_most_restrictive() {
    let suggest = get_preset("suggest").unwrap();

    // Should ask for approval most often
    assert_eq!(suggest.approval_policy, AskForApproval::UnlessTrusted);

    // Should have read-only sandbox
    assert!(matches!(suggest.sandbox_policy, SandboxPolicy::ReadOnly));
}

#[test]
fn test_full_auto_is_least_restrictive() {
    let full_auto = get_preset("full-auto").unwrap();

    // Should ask for approval on failure only
    assert_eq!(full_auto.approval_policy, AskForApproval::OnFailure);

    // Should have network access
    if let SandboxPolicy::WorkspaceWrite { network_access, .. } = full_auto.sandbox_policy {
        assert!(network_access);
    }
}

#[test]
fn test_auto_edit_is_middle_ground() {
    let auto_edit = get_preset("auto-edit").unwrap();

    // Should ask on request
    assert_eq!(auto_edit.approval_policy, AskForApproval::OnRequest);

    // Should have workspace write but no network
    if let SandboxPolicy::WorkspaceWrite { network_access, .. } = auto_edit.sandbox_policy {
        assert!(!network_access);
    }
}

#[test]
fn test_presets_count() {
    // Verify we have exactly 3 presets
    assert_eq!(PRESETS.len(), 3);
}

#[test]
fn test_get_preset_case_sensitive() {
    // Preset names should be case-sensitive
    assert!(get_preset("SUGGEST").is_none());
    assert!(get_preset("Suggest").is_none());
    assert!(get_preset("suggest").is_some());
}
