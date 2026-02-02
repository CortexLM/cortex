//! Approval system for spec mode plans.
//!
//! This module provides the approval workflow for specification plans,
//! allowing users to review and approve/reject plans before they are executed.

use super::plan::SpecPlan;
use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Decision made by the user on a spec plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Plan approved as-is
    Approved,
    /// Plan approved with modifications
    ApprovedWithChanges(Vec<String>),
    /// Plan rejected with reason
    Rejected(String),
    /// Decision deferred (user wants to think about it)
    #[default]
    Deferred,
}

impl ApprovalDecision {
    /// Check if the decision allows proceeding with implementation.
    pub fn can_proceed(&self) -> bool {
        matches!(
            self,
            ApprovalDecision::Approved | ApprovalDecision::ApprovedWithChanges(_)
        )
    }

    /// Get a display message for the decision.
    pub fn message(&self) -> String {
        match self {
            ApprovalDecision::Approved => "Plan approved".to_string(),
            ApprovalDecision::ApprovedWithChanges(changes) => {
                format!("Plan approved with {} modifications", changes.len())
            }
            ApprovalDecision::Rejected(reason) => format!("Plan rejected: {}", reason),
            ApprovalDecision::Deferred => "Decision deferred".to_string(),
        }
    }
}

/// A request for user approval of a spec plan.
pub struct ApprovalRequest {
    /// The plan awaiting approval
    pub plan: SpecPlan,
    /// Channel to send the response back
    pub response_tx: oneshot::Sender<ApprovalDecision>,
}

impl std::fmt::Debug for ApprovalRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalRequest")
            .field("plan", &self.plan)
            .field("response_tx", &"<oneshot::Sender>")
            .finish()
    }
}

/// Manager for handling spec plan approvals.
///
/// This struct manages the approval flow between the agent (which generates plans)
/// and the TUI (which displays them to users for approval).
pub struct ApprovalManager {
    /// Currently pending approval request
    pending: Option<ApprovalRequest>,
    /// Whether auto-approval is enabled (for testing or automation)
    auto_approve: bool,
    /// Timeout for approval requests in seconds (0 = no timeout)
    timeout_secs: u64,
}

impl ApprovalManager {
    /// Create a new approval manager.
    pub fn new() -> Self {
        Self {
            pending: None,
            auto_approve: false,
            timeout_secs: 0,
        }
    }

    /// Create an approval manager with auto-approval enabled.
    pub fn with_auto_approve(mut self) -> Self {
        self.auto_approve = true;
        self
    }

    /// Set the timeout for approval requests.
    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    /// Request approval for a plan.
    ///
    /// This method is called by the agent when it has generated a plan in Spec mode.
    /// It will block until the user makes a decision or the request times out.
    ///
    /// Returns the user's decision.
    pub async fn request_approval(&mut self, plan: SpecPlan) -> ApprovalDecision {
        // If auto-approve is enabled, skip the approval process
        if self.auto_approve {
            return ApprovalDecision::Approved;
        }

        let (tx, rx) = oneshot::channel();
        self.pending = Some(ApprovalRequest {
            plan,
            response_tx: tx,
        });

        // Wait for the response
        if self.timeout_secs > 0 {
            match tokio::time::timeout(std::time::Duration::from_secs(self.timeout_secs), rx).await
            {
                Ok(Ok(decision)) => decision,
                Ok(Err(_)) => ApprovalDecision::Rejected("Channel closed".to_string()),
                Err(_) => ApprovalDecision::Rejected("Approval timeout".to_string()),
            }
        } else {
            rx.await.unwrap_or(ApprovalDecision::Rejected(
                "Channel closed unexpectedly".to_string(),
            ))
        }
    }

    /// Get the currently pending plan for display.
    ///
    /// This is called by the TUI to show the plan to the user.
    pub fn get_pending(&self) -> Option<&SpecPlan> {
        self.pending.as_ref().map(|r| &r.plan)
    }

    /// Check if there's a pending approval request.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Respond to the pending approval request.
    ///
    /// This is called by the TUI when the user makes a decision.
    pub fn respond(&mut self, decision: ApprovalDecision) {
        if let Some(request) = self.pending.take() {
            // Ignore send errors (receiver may have been dropped)
            let _ = request.response_tx.send(decision);
        }
    }

    /// Cancel the pending approval request.
    ///
    /// This sends a Rejected decision with a cancellation message.
    pub fn cancel(&mut self) {
        self.respond(ApprovalDecision::Rejected("Cancelled by user".to_string()));
    }

    /// Clear any pending request without responding.
    ///
    /// Use with caution - this will leave the requester waiting indefinitely
    /// unless auto-approve or timeout is configured.
    pub fn clear_pending(&mut self) {
        self.pending = None;
    }
}

impl Default for ApprovalManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approval_decision_can_proceed() {
        assert!(ApprovalDecision::Approved.can_proceed());
        assert!(ApprovalDecision::ApprovedWithChanges(vec!["change".to_string()]).can_proceed());
        assert!(!ApprovalDecision::Rejected("reason".to_string()).can_proceed());
        assert!(!ApprovalDecision::Deferred.can_proceed());
    }

    #[test]
    fn test_approval_decision_message() {
        assert_eq!(ApprovalDecision::Approved.message(), "Plan approved");
        assert!(ApprovalDecision::Rejected("test".to_string())
            .message()
            .contains("test"));
    }

    #[tokio::test]
    async fn test_auto_approve() {
        let mut manager = ApprovalManager::new().with_auto_approve();
        let plan = SpecPlan::new("Test Plan");

        let decision = manager.request_approval(plan).await;
        assert_eq!(decision, ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_approval_respond() {
        let mut manager = ApprovalManager::new();

        // Create a channel manually to test respond
        let (tx, rx) = oneshot::channel();
        manager.pending = Some(ApprovalRequest {
            plan: SpecPlan::new("Test"),
            response_tx: tx,
        });

        assert!(manager.has_pending());
        assert!(manager.get_pending().is_some());

        // Respond in a separate task
        manager.respond(ApprovalDecision::Approved);

        // The channel should have received the response
        let result = rx.await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), ApprovalDecision::Approved);
    }

    #[tokio::test]
    async fn test_approval_cancel() {
        let mut manager = ApprovalManager::new();

        let (tx, rx) = oneshot::channel();
        manager.pending = Some(ApprovalRequest {
            plan: SpecPlan::new("Test"),
            response_tx: tx,
        });

        manager.cancel();

        let result = rx.await;
        assert!(result.is_ok());
        match result.unwrap() {
            ApprovalDecision::Rejected(reason) => {
                assert!(reason.contains("Cancelled"));
            }
            _ => panic!("Expected Rejected decision"),
        }
    }

    #[tokio::test]
    async fn test_approval_timeout() {
        let mut manager = ApprovalManager::new().with_timeout(1); // 1 second timeout
        let plan = SpecPlan::new("Test Plan");

        // Don't respond - should timeout
        let decision = manager.request_approval(plan).await;

        match decision {
            ApprovalDecision::Rejected(reason) => {
                assert!(reason.contains("timeout") || reason.contains("Timeout"));
            }
            _ => panic!("Expected timeout rejection"),
        }
    }
}
