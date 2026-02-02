//! Session submission handlers - handle_submission, handle_user_input, handle_compact, etc.

use std::sync::atomic::Ordering;

use chrono::Utc;
use tracing::info;

use cortex_protocol::{
    AgentMessageEvent, ErrorEvent, EventMsg, SessionConfiguredEvent, TaskCompleteEvent,
    TaskStartedEvent, TurnDiffEvent, UserMessageEvent,
};

use crate::client::{Message, MessageRole};
use crate::error::Result;
use crate::rollout::RolloutRecorder;
use crate::rollout::recorder::SessionMeta;
use crate::summarization::SummarizationStrategy;

use super::Session;
use super::prompt::build_system_prompt;

impl Session {
    /// Handle an incoming submission.
    pub(super) async fn handle_submission(
        &mut self,
        submission: cortex_protocol::Submission,
    ) -> Result<()> {
        use cortex_protocol::Op;

        match submission.op {
            Op::Shutdown => {
                self.running = false;
                self.emit(EventMsg::ShutdownComplete).await;
            }
            Op::Interrupt => {
                // Set cancellation flag to stop current request
                self.cancelled.store(true, Ordering::SeqCst);
                self.emit(EventMsg::TurnAborted(cortex_protocol::TurnAbortedEvent {
                    reason: cortex_protocol::TurnAbortReason::Interrupted,
                }))
                .await;
            }
            Op::UserInput { items } => {
                self.handle_user_input(&submission.id, items).await?;
            }
            Op::UserTurn { items, .. } => {
                self.handle_user_input(&submission.id, items).await?;
            }
            Op::Compact => {
                self.handle_compact().await?;
            }
            Op::Undo => {
                self.handle_undo().await?;
            }
            Op::Redo => {
                self.handle_redo().await?;
            }
            Op::ForkSession {
                fork_point_message_id,
                message_index,
            } => {
                self.handle_fork_session(fork_point_message_id, message_index)
                    .await?;
            }
            Op::ExecApproval { id, decision } => {
                self.handle_exec_approval(&id, decision).await?;
            }
            Op::OverrideTurnContext {
                cwd,
                approval_policy,
                sandbox_policy,
                model,
                effort,
                summary,
            } => {
                if let Some(cwd) = cwd {
                    info!("Updating session CWD to: {:?}", cwd);
                    self.config.cwd = cwd;
                }
                if let Some(policy) = approval_policy {
                    self.config.approval_policy = policy;
                }
                if let Some(policy) = sandbox_policy {
                    self.config.sandbox_policy = policy;
                }
                if let Some(model) = model {
                    self.config.model = model;
                }
                if let Some(effort) = effort {
                    self.config.reasoning_effort = effort.map(|e| match e {
                        cortex_protocol::ReasoningEffort::Low => {
                            crate::config::ReasoningEffort::Low
                        }
                        cortex_protocol::ReasoningEffort::Medium => {
                            crate::config::ReasoningEffort::Medium
                        }
                        cortex_protocol::ReasoningEffort::High => {
                            crate::config::ReasoningEffort::High
                        }
                    });
                }
                if let Some(summary) = summary {
                    self.config.reasoning_summary = match summary {
                        cortex_protocol::ReasoningSummary::None => {
                            crate::config::ReasoningSummary::None
                        }
                        cortex_protocol::ReasoningSummary::Brief => {
                            crate::config::ReasoningSummary::Brief
                        }
                        cortex_protocol::ReasoningSummary::Detailed => {
                            crate::config::ReasoningSummary::Detailed
                        }
                        cortex_protocol::ReasoningSummary::Auto => {
                            crate::config::ReasoningSummary::Auto
                        }
                    };
                }
            }
            Op::SwitchAgent { name } => {
                info!("Switching agent to: {}", name);
                self.config.current_agent = Some(name);
                // Update system prompt in existing message history
                if let Some(msg) = self.messages.first_mut() {
                    if matches!(msg.role, crate::client::MessageRole::System) {
                        *msg = Message::system(build_system_prompt(&self.config));
                    }
                }
            }
            Op::Share => {
                let url = self
                    .share_service
                    .share(&self.conversation_id.to_string(), &self.messages)
                    .await?;
                self.emit(EventMsg::SessionShared(
                    cortex_protocol::SessionSharedEvent { url },
                ))
                .await;
            }
            Op::Unshare => {
                self.share_service
                    .unshare(&self.conversation_id.to_string())
                    .await?;
                self.emit(EventMsg::SessionUnshared(
                    cortex_protocol::SessionUnsharedEvent { success: true },
                ))
                .await;
            }
            Op::ReloadMcpServers => {
                info!("Reloading MCP servers...");
            }
            Op::EnableMcpServer { name } => {
                info!("Enabling MCP server: {}...", name);
            }
            Op::DisableMcpServer { name } => {
                info!("Disabling MCP server: {}...", name);
            }
            _ => {}
        }
        Ok(())
    }

    /// Handle user input submission.
    pub(super) async fn handle_user_input(
        &mut self,
        _submission_id: &str,
        items: Vec<cortex_protocol::UserInput>,
    ) -> Result<()> {
        tracing::info!("Session handling user input: {} items", items.len());

        // Reset cancellation flag at start of each turn
        self.cancelled.store(false, Ordering::SeqCst);

        self.turn_id += 1;
        let turn_id = self.turn_id.to_string();

        // Extract text from user input
        let user_text: String = items
            .iter()
            .filter_map(|item| {
                if let cortex_protocol::UserInput::Text { text } = item {
                    Some(text.clone())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        if user_text.is_empty() {
            tracing::warn!("Session received empty user input");
            return Ok(());
        }

        tracing::debug!("User message: {}", user_text);

        // Track messages for potential redo functionality
        let _msg_start_idx = self.messages.len();

        // Emit user message event
        self.emit(EventMsg::UserMessage(UserMessageEvent {
            id: None,
            parent_id: None,
            message: user_text.clone(),
            images: None,
        }))
        .await;

        // Add user message to history
        self.messages.push(Message::user(&user_text));

        // Emit task started
        self.emit(EventMsg::TaskStarted(TaskStartedEvent {
            model_context_window: self.config.model_context_window,
        }))
        .await;

        // Fast git-based snapshot (uses git write-tree, instant)
        // Only track if the cwd is a git repository (has .git directory)
        let is_git_repo = self.config.cwd.join(".git").exists()
            || std::process::Command::new("git")
                .args(["rev-parse", "--is-inside-work-tree"])
                .current_dir(&self.config.cwd)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);

        let pre_snapshot_hash = if is_git_repo {
            match crate::git_snapshot::GitSnapshot::new(
                &self.config.cortex_home,
                &self.conversation_id.to_string(),
                self.config.cwd.clone(),
            ) {
                Ok(gs) => gs.track().ok(),
                Err(e) => {
                    tracing::warn!("Failed to create git snapshot: {}", e);
                    None
                }
            }
        } else {
            tracing::debug!("Skipping snapshot - cwd is not a git repository");
            None
        };

        // Run the agent loop until complete
        tracing::info!("Starting agent loop for turn {}...", turn_id);
        if let Err(e) = self.run_agent_loop(&turn_id).await {
            tracing::error!("Agent loop failed: {}", e);
            // Emit error event so TUI can display it
            self.emit(EventMsg::Error(ErrorEvent {
                message: e.to_string(),
                cortex_error_info: None,
            }))
            .await;
            // Emit TaskComplete to reset TUI state
            self.emit(EventMsg::TaskComplete(TaskCompleteEvent {
                last_agent_message: None,
            }))
            .await;
            return Err(e);
        }
        tracing::info!("Agent loop completed for turn {}", turn_id);

        // Fast git-based diff (if we have a pre-snapshot)
        if let Some(hash) = pre_snapshot_hash {
            if let Ok(gs) = crate::git_snapshot::GitSnapshot::new(
                &self.config.cortex_home,
                &self.conversation_id.to_string(),
                self.config.cwd.clone(),
            ) {
                // Get diff for display
                if let Ok(diff_text) = gs.diff(&hash) {
                    if !diff_text.is_empty() {
                        self.emit(EventMsg::TurnDiff(TurnDiffEvent {
                            unified_diff: diff_text,
                        }))
                        .await;
                    }
                }
            }
        }

        // Create undo task
        let undo_task = crate::tasks::undo::UndoTask::new(
            format!("undo_{turn_id}"),
            crate::tasks::undo::UndoTarget::Turn(turn_id.clone()),
        );

        self.undo_history.push(undo_task);

        Ok(())
    }

    /// Handle context compaction.
    pub(super) async fn handle_compact(&mut self) -> Result<()> {
        use crate::client::CompletionRequest;
        use crate::client::ResponseEvent;
        use tokio_stream::StreamExt;

        let strategy = SummarizationStrategy::default();
        let (to_summarize, to_keep) = strategy.split_messages(&self.messages);

        if to_summarize.is_empty() {
            return Ok(());
        }

        // Build summarization prompt
        let prompt = strategy.build_summarization_prompt(&to_summarize);

        // Call model to summarize
        let request = CompletionRequest {
            model: "gpt-4o-mini".to_string(), // Use a cheaper model for summarization
            messages: prompt,
            max_tokens: Some(strategy.target_summary_tokens as u32),
            temperature: Some(0.3),
            ..Default::default()
        };

        let mut stream = self.client.complete(request).await?;
        let mut summary = String::new();

        while let Some(event) = stream.next().await {
            if let ResponseEvent::Delta(delta) = event? {
                summary.push_str(&delta);
            }
        }

        // Replace messages
        let mut new_messages = Vec::new();
        if strategy.preserve_system
            && !self.messages.is_empty()
            && self.messages[0].role == MessageRole::System
        {
            new_messages.push(self.messages[0].clone());
        }

        new_messages.push(Message::system(format!(
            "[Conversation Summary]\n\n{}",
            summary
        )));

        new_messages.extend(to_keep);

        self.messages = new_messages;

        // Emit event or log
        tracing::info!("Context compacted using summarization strategy");

        Ok(())
    }

    /// Handle undo operation.
    pub(super) async fn handle_undo(&mut self) -> Result<()> {
        use cortex_protocol::{UndoCompletedEvent, UndoStartedEvent};

        self.emit(EventMsg::UndoStarted(UndoStartedEvent {
            message: Some("Undoing last turn...".to_string()),
        }))
        .await;

        let mut success = false;
        if let Some(undo_task) = self.undo_history.pop() {
            // Revert file changes
            let mut all_success = true;
            for action in &undo_task.actions {
                match action.execute() {
                    Ok(result) => {
                        if !result.is_success() {
                            tracing::error!("Undo action failed: {}", result.description());
                            all_success = false;
                        }
                    }
                    Err(e) => {
                        tracing::error!("Undo action error: {}", e);
                        all_success = false;
                    }
                }
            }

            if all_success {
                // Remove messages from history
                while let Some(msg) = self.messages.last() {
                    if matches!(msg.role, crate::client::MessageRole::User) {
                        break;
                    }
                    self.messages.pop();
                }
                if let Some(msg) = self.messages.last()
                    && matches!(msg.role, crate::client::MessageRole::User)
                {
                    self.messages.pop();
                }

                // Push to redo history
                self.redo_history.push(undo_task);
                success = true;
            }
        }

        self.emit(EventMsg::UndoCompleted(UndoCompletedEvent {
            success,
            message: if success {
                Some("Undo successful".to_string())
            } else {
                Some("Undo failed".to_string())
            },
        }))
        .await;

        Ok(())
    }

    /// Handle redo operation.
    pub(super) async fn handle_redo(&mut self) -> Result<()> {
        use cortex_protocol::{RedoCompletedEvent, RedoStartedEvent};

        self.emit(EventMsg::RedoStarted(RedoStartedEvent {
            message: Some("Redoing last undone turn...".to_string()),
        }))
        .await;

        let mut success = false;
        if let Some(redo_task) = self.redo_history.pop() {
            // 1. Re-apply file changes if we have a forward diff
            if let Some(diff) = &redo_task.forward_diff {
                if let Err(e) = diff.apply(&self.config.cwd).await {
                    tracing::error!("Redo action error: {}", e);
                    // Continue anyway? For now, yes, but log it.
                }
            }

            // 2. Re-apply messages
            let turn_messages = redo_task.messages.clone();
            self.messages.extend(turn_messages.clone());

            // 3. Emit events for the re-applied messages
            for msg in turn_messages {
                match msg.role {
                    crate::client::MessageRole::User => {
                        if let Some(text) = msg.content.as_text() {
                            self.emit(EventMsg::UserMessage(UserMessageEvent {
                                id: None,
                                parent_id: None,
                                message: text.to_string(),
                                images: None,
                            }))
                            .await;
                        }
                    }
                    crate::client::MessageRole::Assistant => {
                        if let Some(text) = msg.content.as_text() {
                            self.emit(EventMsg::AgentMessage(AgentMessageEvent {
                                id: None,
                                parent_id: None,
                                message: text.to_string(),
                                finish_reason: None,
                            }))
                            .await;
                        }
                    }
                    _ => {}
                }
            }

            // 4. Push back to undo history
            self.undo_history.push(redo_task);
            success = true;
        }

        self.emit(EventMsg::RedoCompleted(RedoCompletedEvent {
            success,
            message: if success {
                Some("Redo successful".to_string())
            } else {
                Some("No turns to redo".to_string())
            },
        }))
        .await;

        Ok(())
    }

    /// Handle session forking.
    pub(super) async fn handle_fork_session(
        &mut self,
        fork_point_message_id: Option<String>,
        message_index: Option<usize>,
    ) -> Result<()> {
        use std::path::PathBuf;

        use cortex_protocol::ConversationId;

        let original_id = self.conversation_id;
        let mut fork_index = self.messages.len();

        if let Some(idx) = message_index {
            if idx < self.messages.len() {
                fork_index = idx + 1;
            }
        } else if let Some(id) = &fork_point_message_id {
            // Try to parse as index first
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.messages.len() {
                    fork_index = idx + 1;
                }
            }
        }

        // Truncate messages
        self.messages.truncate(fork_index);

        // New session ID
        self.conversation_id = ConversationId::new();

        // New recorder
        let mut recorder = RolloutRecorder::new(&self.config.cortex_home, self.conversation_id)?;
        recorder.init()?;

        // Record meta
        let meta = SessionMeta {
            id: self.conversation_id,
            parent_id: Some(original_id),
            fork_point: Some(fork_index.to_string()),
            timestamp: Utc::now().to_rfc3339(),
            cwd: self.config.cwd.clone(),
            model: self.config.model.clone(),
            cli_version: env!("CARGO_PKG_VERSION").to_string(),
            instructions: self.config.user_instructions.clone(),
        };
        recorder.record_meta(&meta)?;
        self.recorder = Some(recorder);

        // Emit SessionConfigured
        self.emit(EventMsg::SessionConfigured(Box::new(
            SessionConfiguredEvent {
                session_id: self.conversation_id,
                parent_session_id: Some(original_id),
                model: self.config.model.clone(),
                model_provider_id: self.config.model_provider_id.clone(),
                approval_policy: self.config.approval_policy,
                sandbox_policy: self.config.sandbox_policy.clone(),
                cwd: self.config.cwd.clone(),
                reasoning_effort: None,
                history_log_id: 0,
                history_entry_count: self.messages.len(),
                initial_messages: None,
                rollout_path: PathBuf::new(),
            },
        )))
        .await;

        Ok(())
    }

    /// Handle execution approval decision.
    pub(super) async fn handle_exec_approval(
        &mut self,
        call_id: &str,
        decision: cortex_protocol::ReviewDecision,
    ) -> Result<()> {
        use cortex_protocol::ReviewDecision;

        use crate::tools::ToolContext;

        if let Some(pending) = self.pending_approvals.remove(call_id) {
            match decision {
                ReviewDecision::Approved | ReviewDecision::ApprovedForSession => {
                    // Execute the approved tool
                    let context = ToolContext::new(self.config.cwd.clone())
                        .with_sandbox_policy(self.config.sandbox_policy.clone())
                        .with_turn_id(self.turn_id.to_string())
                        .with_conversation_id(self.conversation_id.to_string())
                        .with_lsp(self.lsp.clone());

                    let result = self
                        .tool_router
                        .execute(&pending.tool_name, pending.arguments, &context)
                        .await;

                    let result_text = match result {
                        Ok(r) => r.output,
                        Err(e) => format!("Error: {e}"),
                    };

                    // Add tool result to messages
                    self.messages
                        .push(Message::tool_result(&pending.tool_call_id, &result_text));

                    // Continue the agent loop
                    self.run_agent_loop(&self.turn_id.to_string()).await?;
                }
                ReviewDecision::Denied | ReviewDecision::Abort => {
                    // Add rejection message as tool result
                    self.messages.push(Message::tool_result(
                        &pending.tool_call_id,
                        "Command was rejected by user.",
                    ));
                }
            }
        }
        Ok(())
    }
}
