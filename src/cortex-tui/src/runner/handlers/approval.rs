//! Approval action handlers.

use cortex_protocol::ReviewDecision;

use anyhow::Result;

use super::ActionHandler;

impl<'a> ActionHandler<'a> {
    /// Handle approve action - approve pending tool execution.
    pub(crate) async fn handle_approve(&mut self) -> Result<bool> {
        if let Some(ref approval) = self.state.pending_approval {
            if let Some(session) = self.session {
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                session
                    .send_approval(call_id, ReviewDecision::Approved)
                    .await?;
            }
            self.state.approve();
            self.stream.start_streaming(); // Resume streaming after approval
        }
        Ok(true)
    }

    /// Handle reject action - reject pending tool execution.
    pub(crate) async fn handle_reject(&mut self) -> Result<bool> {
        if let Some(ref approval) = self.state.pending_approval {
            if let Some(session) = self.session {
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                session
                    .send_approval(call_id, ReviewDecision::Denied)
                    .await?;
            }
            self.state.reject();
        }
        Ok(true)
    }

    /// Handle approve session action - approve and auto-approve this tool for the session.
    /// Note: Tool execution is handled in event_loop.rs for the new provider system.
    pub(crate) async fn handle_approve_session(&mut self) -> Result<bool> {
        if let Some(ref approval) = self.state.pending_approval {
            if let Some(session) = self.session {
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                session
                    .send_approval(call_id, ReviewDecision::ApprovedForSession)
                    .await?;
            }
            self.state.approve();
            self.stream.start_streaming();
        }
        Ok(true)
    }

    /// Handle approve always action - approve and add to always-allowed list.
    /// Note: Tool execution is handled in event_loop.rs for the new provider system.
    pub(crate) async fn handle_approve_always(&mut self) -> Result<bool> {
        if let Some(ref approval) = self.state.pending_approval {
            if let Some(session) = self.session {
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                // For legacy session, treat as approved for session
                session
                    .send_approval(call_id, ReviewDecision::ApprovedForSession)
                    .await?;
            }
            self.state.approve();
            self.stream.start_streaming();
        }
        Ok(true)
    }

    /// Handle approve all action - approve for entire session.
    pub(crate) async fn handle_approve_all(&mut self) -> Result<bool> {
        if let Some(ref approval) = self.state.pending_approval {
            if let Some(session) = self.session {
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                session
                    .send_approval(call_id, ReviewDecision::ApprovedForSession)
                    .await?;
            }
            self.state.approve();
            self.stream.start_streaming(); // Resume streaming
        }
        Ok(true)
    }

    /// Handle reject all action - abort the current turn.
    pub(crate) async fn handle_reject_all(&mut self) -> Result<bool> {
        if let Some(ref approval) = self.state.pending_approval {
            if let Some(session) = self.session {
                let call_id = serde_json::from_str::<serde_json::Value>(&approval.tool_args)
                    .ok()
                    .and_then(|v| {
                        v.get("call_id")
                            .and_then(|id| id.as_str())
                            .map(String::from)
                    })
                    .unwrap_or_default();
                session
                    .send_approval(call_id, ReviewDecision::Abort)
                    .await?;
            }
            self.state.reject();
        }
        Ok(true)
    }

    /// Handle view diff action - toggle diff view in approval modal.
    ///
    /// Currently logs the request for debugging purposes. Diff view toggle
    /// in ApprovalState is planned for future implementation to allow users
    /// to preview file changes before approving tool execution.
    pub(crate) fn handle_view_diff(&mut self) -> Result<bool> {
        // Feature placeholder: diff view toggle in ApprovalState (planned for future implementation)
        tracing::debug!("View diff requested");
        Ok(true)
    }
}
