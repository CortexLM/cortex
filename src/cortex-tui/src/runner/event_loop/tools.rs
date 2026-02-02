//! Tool execution handling: spawning, events, and completion.

use std::time::{Duration, Instant};

use crate::events::ToolEvent;
use crate::session::StoredToolCall;
use crate::views::tool_call::format_result_summary;

use super::core::EventLoop;

impl EventLoop {
    /// Spawns a tool execution task in the background.
    pub(super) fn spawn_tool_execution(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    ) {
        tracing::info!("Spawning tool execution: {} ({})", tool_name, tool_call_id);

        // Get tool registry
        let Some(registry) = self.tool_registry.clone() else {
            tracing::warn!(
                "Tool registry not initialized, cannot execute: {}",
                tool_name
            );
            self.app_state.add_pending_tool_result(
                tool_call_id,
                tool_name,
                "Tool registry not initialized. This is a configuration error.".to_string(),
                false,
            );
            return;
        };

        let tool_tx = self.tool_event_tx.clone();
        let id = tool_call_id.clone();
        let name = tool_name.clone();

        // Spawn background task for tool execution
        let task = tokio::spawn(async move {
            let started_at = Instant::now();

            // Send started event
            let _ = tool_tx
                .send(ToolEvent::Started {
                    id: id.clone(),
                    name: name.clone(),
                    started_at,
                })
                .await;

            // Execute the tool
            let result = registry.execute(&name, args).await;
            let duration = started_at.elapsed();

            match result {
                Ok(tool_result) => {
                    let _ = tool_tx
                        .send(ToolEvent::Completed {
                            id,
                            name,
                            output: tool_result.output,
                            success: tool_result.success,
                            duration,
                        })
                        .await;
                }
                Err(e) => {
                    let _ = tool_tx
                        .send(ToolEvent::Failed {
                            id,
                            name,
                            error: e.to_string(),
                            duration,
                        })
                        .await;
                }
            }
        });

        self.running_tool_tasks.insert(tool_call_id, task);
    }

    /// Spawns a unified tool execution (for Task/Batch tools).
    /// Routes Task to spawn_subagent, Batch to parallel execution.
    pub(super) fn spawn_unified_tool_execution(
        &mut self,
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    ) {
        tracing::info!(
            "Spawning unified tool execution: {} ({})",
            tool_name,
            tool_call_id
        );

        // Route Task tool to spawn_subagent
        if tool_name == "Task" || tool_name == "task" {
            self.spawn_subagent(tool_call_id, args);
            return;
        }

        // Parse args for Batch: { tool_calls: [{ tool, parameters }, ...] }
        let tool_calls = match args.get("tool_calls").and_then(|v| v.as_array()) {
            Some(calls) => calls.clone(),
            None => {
                self.app_state.add_pending_tool_result(
                    tool_call_id,
                    tool_name,
                    "Batch tool requires 'tool_calls' array parameter.".to_string(),
                    false,
                );
                return;
            }
        };

        // Validate: max 10 tools
        if tool_calls.len() > 10 {
            self.app_state.add_pending_tool_result(
                tool_call_id,
                tool_name,
                format!("Batch tool allows max 10 tools, got {}.", tool_calls.len()),
                false,
            );
            return;
        }

        // Get tool registry
        let Some(registry) = self.tool_registry.clone() else {
            self.app_state.add_pending_tool_result(
                tool_call_id,
                tool_name,
                "Tool registry not available for batch execution.".to_string(),
                false,
            );
            return;
        };

        let tool_tx = self.tool_event_tx.clone();
        let id = tool_call_id.clone();

        // Spawn background task for parallel execution
        let task = tokio::spawn(async move {
            let started_at = Instant::now();

            // Send started event
            let _ = tool_tx
                .send(ToolEvent::Started {
                    id: id.clone(),
                    name: "Batch".to_string(),
                    started_at,
                })
                .await;

            // Execute tools in parallel
            let mut handles = Vec::new();

            for call in tool_calls {
                let tool_name_inner = call
                    .get("tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let params = call
                    .get("parameters")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                // Skip batch in batch (prevent recursion)
                if tool_name_inner.to_lowercase() == "batch" {
                    continue;
                }

                let reg = registry.clone();
                let handle = tokio::spawn(async move {
                    let result = reg.execute(&tool_name_inner, params).await;
                    (tool_name_inner, result)
                });
                handles.push(handle);
            }

            // Wait for all to complete
            let mut results = Vec::new();
            for handle in handles {
                results.push(handle.await);
            }

            let mut successful = 0;
            let mut failed = 0;
            let mut details = Vec::new();

            for result in results {
                match result {
                    Ok((name, Ok(tool_result))) => {
                        if tool_result.success {
                            successful += 1;
                        } else {
                            failed += 1;
                        }
                        details.push(format!(
                            "- {}: {}",
                            name,
                            if tool_result.success {
                                "success"
                            } else {
                                "failed"
                            }
                        ));
                    }
                    Ok((name, Err(e))) => {
                        failed += 1;
                        details.push(format!("- {}: error - {}", name, e));
                    }
                    Err(e) => {
                        failed += 1;
                        details.push(format!("- <task error>: {}", e));
                    }
                }
            }

            let total = successful + failed;
            let output = format!(
                "Batch execution completed: {}/{} successful\n\n{}\n\n\
                 <batch_metadata>\n\
                 total_calls: {}\n\
                 successful: {}\n\
                 failed: {}\n\
                 </batch_metadata>",
                successful,
                total,
                details.join("\n"),
                total,
                successful,
                failed
            );

            let duration = started_at.elapsed();

            let _ = tool_tx
                .send(ToolEvent::Completed {
                    id,
                    name: "Batch".to_string(),
                    output,
                    success: failed == 0,
                    duration,
                })
                .await;
        });

        self.running_tool_tasks.insert(tool_call_id, task);
    }

    /// Handles events from background tool execution tasks.
    pub(super) async fn handle_tool_event(&mut self, event: ToolEvent) {
        use crate::app::SubagentDisplayStatus;

        match event {
            ToolEvent::Started {
                id,
                name,
                started_at: _,
            } => {
                tracing::debug!("Tool execution started: {} ({})", name, id);
                self.stream_controller
                    .set_executing_tool(Some(name.clone()));

                // If this is a Task tool, update the subagent to "Thinking" status
                if name == "Task" || name == "task" {
                    let session_id = format!("subagent_{}", id);
                    self.app_state.update_subagent(&session_id, |task| {
                        task.status = SubagentDisplayStatus::Thinking;
                        task.current_activity = "Processing request...".to_string();
                    });
                }
            }

            ToolEvent::Output { id, chunk } => {
                // Append output to the tool call's live output buffer
                for line in chunk.lines() {
                    if !line.is_empty() {
                        self.app_state.append_tool_output(&id, line.to_string());
                    }
                }
            }

            ToolEvent::Completed {
                id,
                name,
                output,
                success,
                duration,
            } => {
                self.handle_tool_completed(id, name, output, success, duration)
                    .await;
            }

            ToolEvent::Failed {
                id,
                name,
                error,
                duration,
            } => {
                self.handle_tool_failed(id, name, error, duration).await;
            }

            ToolEvent::TodoUpdated { session_id, todos } => {
                self.handle_todo_updated(session_id, todos);
            }

            ToolEvent::AgentGenerated {
                name,
                path,
                location,
            } => {
                tracing::info!(
                    "Agent generated: {} (location: {}, path: {})",
                    name,
                    location,
                    path
                );

                self.app_state
                    .toasts
                    .success(format!("Agent @{} created!", name));

                self.inject_agent_created_event(&name);

                let cwd = std::env::current_dir().ok();
                let terminal_height = self.app_state.terminal_size.1;
                let interactive = crate::interactive::builders::build_agents_selector(
                    cwd.as_deref(),
                    Some(terminal_height),
                );
                self.app_state.enter_interactive_mode(interactive);
            }

            ToolEvent::AgentGenerationFailed { error } => {
                tracing::error!("Agent generation failed: {}", error);
                self.app_state.toasts.error(&error);
                self.app_state.exit_interactive_mode();
            }
        }
    }

    /// Handle tool completion event
    async fn handle_tool_completed(
        &mut self,
        id: String,
        name: String,
        output: String,
        success: bool,
        duration: Duration,
    ) {
        // Handle login events specially
        if name == "login" {
            if id == "login_init" && success {
                self.handle_login_init_success(&output).await;
                return;
            } else if id == "login_poll" && success && output == "login:success" {
                self.handle_login_poll_success().await;
                return;
            } else if id == "login" && success {
                self.handle_legacy_login_success(&output).await;
                return;
            }
        }

        // Handle billing events specially
        if name == "billing"
            && id == "billing"
            && success
            && let Some(data_str) = output.strip_prefix("billing:data:")
        {
            self.handle_billing_data(data_str);
            return;
        }

        tracing::debug!(
            "Tool execution completed: {} ({}) in {:?}",
            name,
            id,
            duration
        );

        // Clear tool execution state
        self.tool_execution_started = false;
        self.stream_controller.set_executing_tool(None);
        self.app_state.streaming.stop_tool_execution();

        // Update tool result in UI
        let summary = format_result_summary(&name, &output, success);
        self.app_state
            .update_tool_result(&id, output.clone(), success, summary.clone());

        // If this is a Task tool, remove the subagent from display
        if name == "Task" || name == "task" {
            let session_id = format!("subagent_{}", id);
            self.app_state.remove_subagent(&session_id);
            if !self.app_state.has_active_subagents() {
                self.app_state.streaming.stop_delegation();
            }
        }

        // Store for agentic continuation
        self.app_state
            .add_pending_tool_result(id.clone(), name.clone(), output, success);

        // Remove from running tasks
        self.running_tool_tasks.remove(&id);

        // Force-save assistant message if stream not done
        if !self.stream_done_received && !self.pending_assistant_tool_calls.is_empty() {
            if let Some(ref mut session) = self.cortex_session {
                let tool_calls_for_message = std::mem::take(&mut self.pending_assistant_tool_calls);
                let content = self.stream_controller.full_text();

                let mut stored_msg = crate::session::StoredMessage::assistant(&content);
                for tc in &tool_calls_for_message {
                    let tool_call = StoredToolCall::new(&tc.id, &tc.name, tc.arguments.clone());
                    stored_msg = stored_msg.with_tool_call(tool_call);
                }
                session.add_message_raw(stored_msg);
                tracing::debug!(
                    "Force-saved assistant message with {} tool calls before stream done",
                    tool_calls_for_message.len()
                );
            }
            self.stream_done_received = true;
        }

        // Continue agentic loop if no more tools are running
        tracing::info!(
            running_tools = self.running_tool_tasks.len(),
            running_subagents = self.running_subagents.len(),
            has_pending_results = self.app_state.has_pending_tool_results(),
            stream_done = self.stream_done_received,
            "ToolEvent::Completed - checking continuation state"
        );

        if self.running_tool_tasks.is_empty() && self.running_subagents.is_empty() {
            if self.app_state.has_pending_tool_results() {
                tracing::info!("Calling continue_with_tool_results from ToolEvent::Completed");
                let _ = self.continue_with_tool_results().await;
            } else if self.app_state.has_queued_messages() {
                tracing::info!("Processing message queue after tool completion");
                let _ = self.process_message_queue().await;
            } else {
                tracing::info!("All tools done, full resetting streaming state");
                self.app_state.streaming.full_reset();
            }
        } else {
            tracing::info!("More tools still running after this completion");
        }
    }

    /// Handle tool failure event
    async fn handle_tool_failed(
        &mut self,
        id: String,
        name: String,
        error: String,
        duration: Duration,
    ) {
        use crate::app::SubagentDisplayStatus;

        // Handle login errors specially
        if name == "login" && (id == "login_init" || id == "login_poll") {
            let error_msg = if let Some(msg) = error.strip_prefix("login:error:") {
                msg.to_string()
            } else if error == "login:expired" {
                "Code expired. Please try again.".to_string()
            } else if error == "login:denied" {
                "Access denied".to_string()
            } else if error == "login:timeout" {
                "Login timed out".to_string()
            } else {
                error.clone()
            };
            self.app_state.login_flow = None;
            self.app_state.exit_interactive_mode();
            self.app_state.toasts.error(&error_msg);
            return;
        }

        // Handle billing errors specially
        if name == "billing" && id == "billing" {
            self.handle_billing_error(&error);
            return;
        }

        tracing::error!(
            "Tool execution failed: {} ({}) after {:?} - {}",
            name,
            id,
            duration,
            error
        );

        // Clear tool execution state
        self.tool_execution_started = false;
        self.stream_controller.set_executing_tool(None);
        self.app_state.streaming.stop_tool_execution();

        self.app_state
            .update_tool_result(&id, error.clone(), false, format!("Error: {}", error));

        // If this is a Task tool, update subagent status to Failed
        if name == "Task" || name == "task" {
            let session_id = format!("subagent_{}", id);
            let error_clone = error.clone();
            self.app_state.update_subagent(&session_id, |task| {
                task.status = SubagentDisplayStatus::Failed;
                task.error_message = Some(error_clone);
                task.current_activity = "Failed".to_string();
            });
            let has_running_subagents = self
                .app_state
                .active_subagents
                .iter()
                .any(|t| !t.status.is_terminal());
            if !has_running_subagents {
                self.app_state.streaming.stop_delegation();
            }
        }

        self.app_state
            .add_pending_tool_result(id.clone(), name, error, false);

        self.running_tool_tasks.remove(&id);

        // Same as Completed: force-save assistant message if stream not done
        if !self.stream_done_received && !self.pending_assistant_tool_calls.is_empty() {
            if let Some(ref mut session) = self.cortex_session {
                let tool_calls_for_message = std::mem::take(&mut self.pending_assistant_tool_calls);
                let content = self.stream_controller.full_text();

                let mut stored_msg = crate::session::StoredMessage::assistant(&content);
                for tc in &tool_calls_for_message {
                    let tool_call = StoredToolCall::new(&tc.id, &tc.name, tc.arguments.clone());
                    stored_msg = stored_msg.with_tool_call(tool_call);
                }
                session.add_message_raw(stored_msg);
            }
            self.stream_done_received = true;
        }

        // Continue agentic loop if no more tools are running
        if self.running_tool_tasks.is_empty() && self.running_subagents.is_empty() {
            if self.app_state.has_pending_tool_results() {
                let _ = self.continue_with_tool_results().await;
            } else if self.app_state.has_queued_messages() {
                let _ = self.process_message_queue().await;
            } else {
                self.app_state.streaming.full_reset();
            }
        }
    }

    /// Handle todo updated event
    fn handle_todo_updated(&mut self, session_id: String, todos: Vec<(String, String)>) {
        use crate::app::{SubagentTodoItem, SubagentTodoStatus};

        tracing::debug!(
            "Subagent todo list updated: {} ({} items)",
            session_id,
            todos.len()
        );

        self.app_state.update_subagent(&session_id, |task| {
            task.todos = todos
                .iter()
                .map(|(content, status)| {
                    let status = match status.as_str() {
                        "in_progress" => SubagentTodoStatus::InProgress,
                        "completed" => SubagentTodoStatus::Completed,
                        _ => SubagentTodoStatus::Pending,
                    };
                    SubagentTodoItem {
                        content: content.clone(),
                        status,
                    }
                })
                .collect();

            // Update activity based on in-progress item
            if let Some(in_progress) = task
                .todos
                .iter()
                .find(|t| matches!(t.status, SubagentTodoStatus::InProgress))
            {
                task.current_activity = in_progress.content.clone();
            }
        });
    }

    /// Checks for crashed background tool tasks (panics or cancelled).
    pub(super) async fn check_crashed_tasks(&mut self) {
        // Collect finished task IDs
        let finished_task_ids: Vec<String> = self
            .running_tool_tasks
            .iter()
            .filter(|(_, handle)| handle.is_finished())
            .map(|(id, _)| id.clone())
            .collect();

        // Process finished tasks - check if they actually crashed
        for id in finished_task_ids {
            if let Some(handle) = self.running_tool_tasks.remove(&id) {
                // Await the handle to check if it panicked or was cancelled
                match handle.await {
                    Ok(()) => {
                        // Task completed normally - it sent its own Completed/Failed event
                        tracing::debug!("Task {} finished normally, event pending in channel", id);
                    }
                    Err(join_error) => {
                        // Task actually crashed - send a failure event
                        let error_msg = if join_error.is_panic() {
                            format!("Subagent panicked: {:?}", join_error.into_panic())
                        } else if join_error.is_cancelled() {
                            "Subagent task was cancelled".to_string()
                        } else {
                            format!("Subagent task failed: {}", join_error)
                        };

                        tracing::error!("Detected crashed task: {} - {}", id, error_msg);

                        // Send failure event through the tool event channel
                        let _ = self
                            .tool_event_tx
                            .send(ToolEvent::Failed {
                                id: id.clone(),
                                name: "Task".to_string(),
                                error: error_msg,
                                duration: std::time::Duration::from_secs(0),
                            })
                            .await;
                    }
                }
            }
        }
    }
}
