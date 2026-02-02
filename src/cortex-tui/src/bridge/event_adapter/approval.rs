//! Approval state builders.
//!
//! This module provides functions to create ApprovalState instances from
//! approval request events for display in the TUI.

use crate::app::{ApprovalMode, ApprovalState};
use cortex_protocol::{ApplyPatchApprovalRequestEvent, ExecApprovalRequestEvent};

use super::utils::format_command;

/// Create an ApprovalState from an ExecApprovalRequest event.
///
/// This creates a fully-populated ApprovalState that can be used to display
/// an approval dialog in the TUI.
///
/// # Arguments
///
/// * `request` - The exec approval request event
///
/// # Returns
///
/// An ApprovalState configured for the command approval.
///
/// # Example
///
/// ```rust,ignore
/// if let EventMsg::ExecApprovalRequest(ref e) = event.msg {
///     let approval_state = create_approval_state(e);
///     app_state.request_approval(approval_state);
/// }
/// ```
pub fn create_approval_state(request: &ExecApprovalRequestEvent) -> ApprovalState {
    let tool_name = format_command(&request.command);
    let tool_args = serde_json::json!({
        "command": request.command,
        "cwd": request.cwd.display().to_string(),
        "call_id": request.call_id,
        "turn_id": request.turn_id,
    });
    let diff_preview = request
        .sandbox_assessment
        .as_ref()
        .map(|a| format!("Risk Level: {:?}\n\n{}", a.risk_level, a.explanation));

    ApprovalState {
        tool_call_id: request.call_id.clone(),
        tool_name,
        tool_args: tool_args.to_string(),
        tool_args_json: Some(tool_args),
        diff_preview,
        approval_mode: ApprovalMode::Ask,
    }
}

/// Create an ApprovalState from an ApplyPatchApprovalRequest event.
///
/// # Arguments
///
/// * `request` - The patch approval request event
///
/// # Returns
///
/// An ApprovalState configured for the patch approval.
pub fn create_patch_approval_state(request: &ApplyPatchApprovalRequestEvent) -> ApprovalState {
    let tool_name = format!(
        "Apply Patch ({} files, +{} -{} lines)",
        request.summary.total_files(),
        request.summary.lines_added,
        request.summary.lines_removed
    );
    let tool_args = serde_json::json!({
        "call_id": request.call_id,
        "turn_id": request.turn_id,
        "files": request.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    });

    ApprovalState {
        tool_call_id: request.call_id.clone(),
        tool_name,
        tool_args: tool_args.to_string(),
        tool_args_json: Some(tool_args),
        diff_preview: Some(request.patch.clone()),
        approval_mode: ApprovalMode::Ask,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_create_approval_state() {
        let request = ExecApprovalRequestEvent {
            call_id: "call-1".to_string(),
            turn_id: "turn-1".to_string(),
            command: vec!["rm".to_string(), "-rf".to_string()],
            cwd: PathBuf::from("/tmp"),
            sandbox_assessment: None,
        };
        let state = create_approval_state(&request);
        assert_eq!(state.tool_name, "rm -rf");
        assert_eq!(state.approval_mode, ApprovalMode::Ask);
    }
}
