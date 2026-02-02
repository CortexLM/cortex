//! Approval operation methods for SubmissionBuilder.

use cortex_protocol::{Op, ReviewDecision};

use super::SubmissionBuilder;

impl SubmissionBuilder {
    /// Create an execution approval submission with a specific decision.
    ///
    /// This is the general method for creating approval responses.
    /// For common cases, prefer the convenience methods like `approve()`,
    /// `deny()`, etc.
    ///
    /// # Arguments
    ///
    /// * `call_id` - The ID of the tool call being approved/denied
    /// * `decision` - The review decision
    pub fn approval(call_id: impl Into<String>, decision: ReviewDecision) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::ExecApproval {
            id: call_id.into(),
            decision,
        });
        builder
    }

    /// Approve a tool execution.
    ///
    /// The tool will be executed once.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let approval = SubmissionBuilder::approve("call-123").build_expect();
    /// ```
    pub fn approve(call_id: impl Into<String>) -> Self {
        Self::approval(call_id, ReviewDecision::Approved)
    }

    /// Approve a tool execution for this session only.
    ///
    /// The tool (and similar tools) will be auto-approved for the
    /// remainder of this session.
    pub fn approve_session(call_id: impl Into<String>) -> Self {
        Self::approval(call_id, ReviewDecision::ApprovedForSession)
    }

    /// Deny a tool execution.
    ///
    /// The agent will try an alternative approach.
    pub fn deny(call_id: impl Into<String>) -> Self {
        Self::approval(call_id, ReviewDecision::Denied)
    }

    /// Abort the current turn.
    ///
    /// Denies the tool execution and stops the agent until the next
    /// user input.
    pub fn abort(call_id: impl Into<String>) -> Self {
        Self::approval(call_id, ReviewDecision::Abort)
    }

    /// Create a patch approval submission.
    ///
    /// Used for approving code patches/edits.
    pub fn patch_approval(call_id: impl Into<String>, decision: ReviewDecision) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::PatchApproval {
            id: call_id.into(),
            decision,
        });
        builder
    }

    /// Approve a patch.
    pub fn approve_patch(call_id: impl Into<String>) -> Self {
        Self::patch_approval(call_id, ReviewDecision::Approved)
    }

    /// Deny a patch.
    pub fn deny_patch(call_id: impl Into<String>) -> Self {
        Self::patch_approval(call_id, ReviewDecision::Denied)
    }
}
