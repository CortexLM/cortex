//! Comprehensive tests for approvals module.

use crate::approvals::*;
use crate::protocol::SandboxRiskLevel;
use std::path::PathBuf;

#[test]
fn test_patch_summary_default() {
    let summary = PatchSummary::default();

    assert_eq!(summary.files_added, 0);
    assert_eq!(summary.files_modified, 0);
    assert_eq!(summary.files_deleted, 0);
    assert_eq!(summary.lines_added, 0);
    assert_eq!(summary.lines_removed, 0);
    assert!(summary.is_empty());
}

#[test]
fn test_patch_summary_new() {
    let summary = PatchSummary::new();

    assert!(summary.is_empty());
    assert_eq!(summary.total_files(), 0);
    assert_eq!(summary.total_lines_changed(), 0);
}

#[test]
fn test_patch_summary_with_changes() {
    let summary = PatchSummary {
        files_added: 2,
        files_modified: 3,
        files_deleted: 1,
        lines_added: 100,
        lines_removed: 50,
    };

    assert!(!summary.is_empty());
    assert_eq!(summary.total_files(), 6);
    assert_eq!(summary.total_lines_changed(), 150);
}

#[test]
fn test_patch_summary_is_empty_edge_cases() {
    let summary1 = PatchSummary {
        files_added: 1,
        ..Default::default()
    };
    assert!(!summary1.is_empty());

    let summary2 = PatchSummary {
        files_modified: 1,
        ..Default::default()
    };
    assert!(!summary2.is_empty());

    let summary3 = PatchSummary {
        files_deleted: 1,
        ..Default::default()
    };
    assert!(!summary3.is_empty());

    // Lines don't affect is_empty
    let summary4 = PatchSummary {
        lines_added: 100,
        ..Default::default()
    };
    assert!(summary4.is_empty());
}

#[test]
fn test_apply_patch_approval_request_event() {
    let event = ApplyPatchApprovalRequestEvent {
        call_id: "patch_call_1".to_string(),
        turn_id: "turn_123".to_string(),
        patch: "@@ -1,3 +1,4 @@\n+new line\n existing".to_string(),
        files: vec![
            PathBuf::from("/project/src/main.rs"),
            PathBuf::from("/project/src/lib.rs"),
        ],
        summary: PatchSummary {
            files_added: 0,
            files_modified: 2,
            files_deleted: 0,
            lines_added: 10,
            lines_removed: 5,
        },
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("patch_call_1"));
    assert!(json.contains("main.rs"));

    let parsed: ApplyPatchApprovalRequestEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.call_id, "patch_call_1");
    assert_eq!(parsed.files.len(), 2);
}

#[test]
fn test_elicitation_request_event() {
    let event = ElicitationRequestEvent {
        server_name: "oauth-server".to_string(),
        request_id: "req_abc123".to_string(),
        message: "Please authenticate with GitHub".to_string(),
        schema: Some(serde_json::json!({
            "type": "object",
            "properties": {
                "token": {"type": "string"}
            }
        })),
    };

    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("oauth-server"));
    assert!(json.contains("authenticate"));

    let parsed: ElicitationRequestEvent = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.server_name, "oauth-server");
    assert!(parsed.schema.is_some());
}

#[test]
fn test_elicitation_request_without_schema() {
    let event = ElicitationRequestEvent {
        server_name: "simple-server".to_string(),
        request_id: "req_simple".to_string(),
        message: "Confirm action".to_string(),
        schema: None,
    };

    let json = serde_json::to_string(&event).expect("serialize");
    // Schema should be omitted when None
    assert!(!json.contains("schema"));
}

#[test]
fn test_command_risk_assessment_low() {
    let assessment = CommandRiskAssessment::low("Read-only file access");

    assert_eq!(assessment.level, SandboxRiskLevel::Low);
    assert_eq!(assessment.explanation, "Read-only file access");
    assert!(assessment.concerns.is_empty());
    assert!(assessment.reversible);
}

#[test]
fn test_command_risk_assessment_medium() {
    let assessment = CommandRiskAssessment::medium(
        "Modifies files in workspace",
        vec!["Overwrites existing files".to_string()],
    );

    assert_eq!(assessment.level, SandboxRiskLevel::Medium);
    assert_eq!(assessment.concerns.len(), 1);
    assert!(assessment.reversible);
}

#[test]
fn test_command_risk_assessment_high() {
    let assessment = CommandRiskAssessment::high(
        "Deletes system files",
        vec![
            "Permanent deletion".to_string(),
            "System-wide impact".to_string(),
        ],
    );

    assert_eq!(assessment.level, SandboxRiskLevel::High);
    assert_eq!(assessment.concerns.len(), 2);
    assert!(!assessment.reversible);
}

#[test]
fn test_command_risk_assessment_into_sandbox_assessment() {
    use crate::protocol::SandboxCommandAssessment;

    let risk = CommandRiskAssessment::medium("Test explanation", vec!["concern1".to_string()]);

    let sandbox: SandboxCommandAssessment = risk.into();

    assert_eq!(sandbox.risk_level, SandboxRiskLevel::Medium);
    assert_eq!(sandbox.explanation, "Test explanation");
}

#[test]
fn test_command_risk_assessment_serialization() {
    let assessment = CommandRiskAssessment {
        level: SandboxRiskLevel::High,
        explanation: "Dangerous operation".to_string(),
        concerns: vec!["Data loss".to_string(), "Irreversible".to_string()],
        reversible: false,
    };

    let json = serde_json::to_string(&assessment).expect("serialize");
    assert!(json.contains("high"));
    assert!(json.contains("Dangerous"));
    assert!(json.contains("Data loss"));

    let parsed: CommandRiskAssessment = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(parsed.level, SandboxRiskLevel::High);
    assert!(!parsed.reversible);
}

#[test]
fn test_review_decision_reexport() {
    // Verify re-exports work
    let decision = ReviewDecision::Approved;
    assert_eq!(decision, ReviewDecision::Approved);
}

#[test]
fn test_elicitation_action_reexport() {
    let approve = ElicitationAction::Approve;
    let deny = ElicitationAction::Deny;

    assert_eq!(approve, ElicitationAction::Approve);
    assert_eq!(deny, ElicitationAction::Deny);
}

#[test]
fn test_sandbox_risk_level_all_variants() {
    let levels = vec![
        SandboxRiskLevel::Low,
        SandboxRiskLevel::Medium,
        SandboxRiskLevel::High,
    ];

    for level in levels {
        let json = serde_json::to_string(&level).expect("serialize");
        let parsed: SandboxRiskLevel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(level, parsed);
    }
}

#[test]
fn test_patch_summary_serialization() {
    let summary = PatchSummary {
        files_added: 1,
        files_modified: 2,
        files_deleted: 0,
        lines_added: 50,
        lines_removed: 10,
    };

    let json = serde_json::to_string(&summary).expect("serialize");
    let parsed: PatchSummary = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.files_added, 1);
    assert_eq!(parsed.files_modified, 2);
    assert_eq!(parsed.files_deleted, 0);
    assert_eq!(parsed.lines_added, 50);
    assert_eq!(parsed.lines_removed, 10);
}

#[test]
fn test_apply_patch_event_with_empty_summary() {
    let event = ApplyPatchApprovalRequestEvent {
        call_id: "empty_patch".to_string(),
        turn_id: "turn_empty".to_string(),
        patch: "".to_string(),
        files: vec![],
        summary: PatchSummary::default(),
    };

    assert!(event.summary.is_empty());
    assert!(event.files.is_empty());
}
