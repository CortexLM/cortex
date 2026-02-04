//! Subagent spawning and event handling.

use std::time::{Duration, Instant};

/// Connection timeout for subagent streaming requests.
/// Higher than main streaming to allow for subagent initialization.
const SUBAGENT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(120);

/// Per-event timeout during subagent responses.
/// Higher than main streaming to account for longer tool executions.
const SUBAGENT_EVENT_TIMEOUT: Duration = Duration::from_secs(60);

use crate::app::SubagentTaskDisplay;
use crate::events::{SubagentEvent, ToolEvent};
use crate::session::StoredToolCall;

use cortex_engine::client::{Message, ResponseEvent, ToolDefinition as ClientToolDefinition};
use cortex_engine::tools::handlers::subagent::ProgressEvent;
use tokio_stream::StreamExt;

use super::core::{EventLoop, simplify_error_message};

impl EventLoop {
    /// Spawns a subagent task (for Task tool).
    /// Handles agent spawning, progress tracking, and result collection.
    pub(super) fn spawn_subagent(&mut self, tool_call_id: String, args: serde_json::Value) {
        tracing::info!("Spawning subagent for tool call: {}", tool_call_id);

        // Support both API format and internal format
        let agent = args.get("agent").and_then(|v| v.as_str());
        let task = args.get("task").and_then(|v| v.as_str());
        let context = args.get("context").and_then(|v| v.as_str());

        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .or(agent)
            .unwrap_or("Subagent task")
            .to_string();

        let prompt = args
            .get("prompt")
            .and_then(|v| v.as_str())
            .or(task)
            .map(|p| {
                if let Some(ctx) = context {
                    format!("{}\n\nContext: {}", p, ctx)
                } else {
                    p.to_string()
                }
            })
            .unwrap_or_default();

        let subagent_type = args
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .or(agent)
            .unwrap_or("code")
            .to_string();

        if prompt.is_empty() {
            self.app_state.add_pending_tool_result(
                tool_call_id,
                "Task".to_string(),
                "Task tool requires a 'task' or 'prompt' parameter with instructions for the subagent.".to_string(),
                false,
            );
            return;
        }

        // Get dependencies for the spawned task
        let Some(registry) = self.tool_registry.clone() else {
            self.app_state.add_pending_tool_result(
                tool_call_id,
                "Task".to_string(),
                "Tool registry not available for subagent.".to_string(),
                false,
            );
            return;
        };

        let Some(provider_manager) = self.provider_manager.clone() else {
            self.app_state.add_pending_tool_result(
                tool_call_id,
                "Task".to_string(),
                "Provider not configured for subagent.".to_string(),
                false,
            );
            return;
        };

        let tool_tx = self.tool_event_tx.clone();
        let id = tool_call_id.clone();

        // Add to UI display
        self.app_state.add_subagent_task(SubagentTaskDisplay::new(
            format!("subagent_{}", id),
            id.clone(),
            description.clone(),
            subagent_type.clone(),
        ));

        // Mark that we're in delegation mode for UI status indicator
        self.app_state.streaming.start_delegation();

        // Spawn background task with full agentic loop
        let task = tokio::spawn(async move {
            let started_at = Instant::now();

            // Send started event
            if let Err(e) = tool_tx
                .send(ToolEvent::Started {
                    id: id.clone(),
                    name: "Task".to_string(),
                    started_at,
                })
                .await
            {
                tracing::error!(
                    "Failed to send ToolEvent::Started for subagent {}: {:?}",
                    id,
                    e
                );
                return;
            }

            // Build subagent system prompt
            let system_prompt = format!(
                "You are a specialized {} subagent working on: {}\n\n\
                 You have access to tools like Read, Edit, Grep, Glob, LS, Execute, Batch, TodoWrite, etc.\n\
                 Note: You cannot use the Task tool (no nested delegation).\n\n\
                 IMPORTANT - Todo List:\n\
                 - For any multi-step task, IMMEDIATELY use TodoWrite to create a todo list\n\
                 - Update the todo list as you progress (mark items in_progress or completed)\n\
                 - This provides real-time visibility to the user\n\
                 - Keep only ONE item as in_progress at a time\n\n\
                 Use Batch to execute multiple tools in parallel for efficiency.\n\
                 If a tool fails, try an alternative approach instead of giving up.\n\
                 Complete the task and provide a clear summary when done.",
                subagent_type, description
            );

            // Build initial messages for subagent
            let mut messages = vec![Message::system(system_prompt), Message::user(&prompt)];

            // Get tool definitions - filter based on subagent permissions
            let tools: Vec<ClientToolDefinition> = registry
                .get_definitions()
                .into_iter()
                .filter(|t| {
                    let name_lower = t.name.to_lowercase();
                    name_lower != "task"
                })
                .map(|t| ClientToolDefinition::function(t.name, t.description, t.parameters))
                .collect();

            // Get model info
            let model = {
                let pm = provider_manager.read().await;
                pm.current_model().to_string()
            };

            let mut final_content = String::new();
            let mut tool_calls_executed: Vec<String> = Vec::new();
            let max_iterations = 500;

            // Agentic loop - continues until no more tool calls
            for iteration in 0..max_iterations {
                tracing::info!("Subagent iteration {}", iteration + 1);

                // Get fresh client for each iteration
                let client = {
                    let mut pm = provider_manager.write().await;
                    if let Err(e) = pm.ensure_client() {
                        let error_msg = format!("Failed to initialize provider: {}", e);
                        tracing::error!("Subagent {}: {}", id, error_msg);
                        if let Err(send_err) = tool_tx
                            .send(ToolEvent::Failed {
                                id: id.clone(),
                                name: "Task".to_string(),
                                error: error_msg,
                                duration: started_at.elapsed(),
                            })
                            .await
                        {
                            tracing::error!("Failed to send ToolEvent::Failed: {:?}", send_err);
                        }
                        return;
                    }
                    pm.take_client()
                };

                let Some(client) = client else {
                    tracing::error!("Subagent {}: No client available", id);
                    if let Err(e) = tool_tx
                        .send(ToolEvent::Failed {
                            id: id.clone(),
                            name: "Task".to_string(),
                            error: "No client available".to_string(),
                            duration: started_at.elapsed(),
                        })
                        .await
                    {
                        tracing::error!("Failed to send ToolEvent::Failed: {:?}", e);
                    }
                    return;
                };

                // Make LLM request
                let request = cortex_engine::client::CompletionRequest {
                    messages: messages.clone(),
                    model: model.clone(),
                    max_tokens: Some(8192),
                    temperature: Some(0.7),
                    seed: None,
                    tools: tools.clone(),
                    stream: true,
                };

                let stream_result =
                    tokio::time::timeout(SUBAGENT_CONNECTION_TIMEOUT, client.complete(request))
                        .await;

                let mut stream = match stream_result {
                    Ok(Ok(s)) => s,
                    Ok(Err(e)) => {
                        let error_msg = simplify_error_message(&e.to_string());
                        tracing::error!("Subagent {} LLM request failed: {}", id, error_msg);
                        if let Err(send_err) = tool_tx
                            .send(ToolEvent::Failed {
                                id: id.clone(),
                                name: "Task".to_string(),
                                error: error_msg,
                                duration: started_at.elapsed(),
                            })
                            .await
                        {
                            tracing::error!("Failed to send ToolEvent::Failed: {:?}", send_err);
                        }
                        return;
                    }
                    Err(_) => {
                        tracing::error!("Subagent {} connection timeout (120s)", id);
                        if let Err(e) = tool_tx
                            .send(ToolEvent::Failed {
                                id: id.clone(),
                                name: "Task".to_string(),
                                error: "Connection timeout".to_string(),
                                duration: started_at.elapsed(),
                            })
                            .await
                        {
                            tracing::error!("Failed to send ToolEvent::Failed: {:?}", e);
                        }
                        return;
                    }
                };

                // Collect response from this iteration
                let mut iteration_content = String::new();
                let mut iteration_tool_calls: Vec<(String, String, serde_json::Value)> = Vec::new();

                loop {
                    let event = tokio::time::timeout(SUBAGENT_EVENT_TIMEOUT, stream.next()).await;

                    match event {
                        Ok(Some(Ok(ResponseEvent::Delta(delta)))) => {
                            iteration_content.push_str(&delta);
                        }
                        Ok(Some(Ok(ResponseEvent::Done(_)))) => {
                            break;
                        }
                        Ok(Some(Ok(ResponseEvent::ToolCall(tc)))) => {
                            let args: serde_json::Value = serde_json::from_str(&tc.arguments)
                                .unwrap_or(serde_json::json!({}));
                            iteration_tool_calls.push((tc.id, tc.name, args));
                        }
                        Ok(Some(Ok(ResponseEvent::Error(e)))) => {
                            tracing::error!("Subagent {} received error from LLM: {}", id, e);
                            if let Err(send_err) = tool_tx
                                .send(ToolEvent::Failed {
                                    id: id.clone(),
                                    name: "Task".to_string(),
                                    error: e,
                                    duration: started_at.elapsed(),
                                })
                                .await
                            {
                                tracing::error!("Failed to send ToolEvent::Failed: {:?}", send_err);
                            }
                            return;
                        }
                        Ok(Some(Err(e))) => {
                            tracing::error!("Subagent {} stream error: {}", id, e);
                            if let Err(send_err) = tool_tx
                                .send(ToolEvent::Failed {
                                    id: id.clone(),
                                    name: "Task".to_string(),
                                    error: e.to_string(),
                                    duration: started_at.elapsed(),
                                })
                                .await
                            {
                                tracing::error!("Failed to send ToolEvent::Failed: {:?}", send_err);
                            }
                            return;
                        }
                        Ok(None) => {
                            tracing::warn!(
                                "Subagent {} stream ended without Done event at iteration {}",
                                id,
                                iteration + 1
                            );
                            break;
                        }
                        Err(_) => {
                            tracing::error!(
                                "Subagent {} response timeout at iteration {}",
                                id,
                                iteration + 1
                            );
                            if let Err(e) = tool_tx
                                .send(ToolEvent::Failed {
                                    id: id.clone(),
                                    name: "Task".to_string(),
                                    error: "Response timeout".to_string(),
                                    duration: started_at.elapsed(),
                                })
                                .await
                            {
                                tracing::error!("Failed to send ToolEvent::Failed: {:?}", e);
                            }
                            return;
                        }
                        _ => {}
                    }
                }

                // If no tool calls, we're done
                if iteration_tool_calls.is_empty() {
                    if !tool_calls_executed.is_empty() && iteration_content.trim().is_empty() {
                        // Request explicit summary if LLM didn't provide one
                        final_content = format!(
                            "Task completed with {} tool call(s).",
                            tool_calls_executed.len()
                        );
                    } else {
                        final_content = iteration_content;
                    }
                    break;
                }

                // Execute tool calls
                let mut tool_results: Vec<(String, String)> = Vec::new();
                let tool_calls_for_msg: Vec<cortex_engine::client::ToolCall> = iteration_tool_calls
                    .iter()
                    .map(
                        |(tc_id, tc_name, tc_args)| cortex_engine::client::ToolCall {
                            id: tc_id.clone(),
                            call_type: "function".to_string(),
                            function: cortex_engine::client::FunctionCall {
                                name: tc_name.clone(),
                                arguments: tc_args.to_string(),
                            },
                        },
                    )
                    .collect();

                const MAX_TOOL_OUTPUT_SIZE: usize = 32_000;

                for (tc_id, tc_name, tc_args) in &iteration_tool_calls {
                    tracing::info!("Subagent executing tool: {} ({})", tc_name, tc_id);

                    // Handle TodoWrite for progress tracking
                    if tc_name == "TodoWrite"
                        && let Some(todos_arr) = tc_args.get("todos").and_then(|v| v.as_array())
                    {
                        let todos: Vec<(String, String)> = todos_arr
                            .iter()
                            .filter_map(|t| {
                                let content = t.get("content").and_then(|v| v.as_str())?;
                                let status = t
                                    .get("status")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("pending");
                                Some((content.to_string(), status.to_string()))
                            })
                            .collect();

                        if !todos.is_empty()
                            && let Err(e) = tool_tx
                                .send(ToolEvent::TodoUpdated {
                                    session_id: format!("subagent_{}", id),
                                    todos,
                                })
                                .await
                        {
                            tracing::warn!("Failed to send TodoUpdated event: {:?}", e);
                        }
                    }

                    let result = registry.execute(tc_name, tc_args.clone()).await;
                    match result {
                        Ok(tool_result) => {
                            let status = if tool_result.success {
                                "success"
                            } else {
                                "failed"
                            };
                            tool_calls_executed.push(format!("{}: {}", tc_name, status));

                            let output = if tool_result.output.len() > MAX_TOOL_OUTPUT_SIZE {
                                let truncated = &tool_result.output[..MAX_TOOL_OUTPUT_SIZE];
                                format!(
                                    "{}...\n\n[Output truncated: {} bytes total, showing first {} bytes]",
                                    truncated,
                                    tool_result.output.len(),
                                    MAX_TOOL_OUTPUT_SIZE
                                )
                            } else {
                                tool_result.output
                            };
                            tool_results.push((tc_id.clone(), output));
                        }
                        Err(e) => {
                            let error_msg = format!("Error executing {}: {}", tc_name, e);
                            tool_calls_executed.push(format!("{}: error", tc_name));
                            tool_results.push((tc_id.clone(), error_msg));
                        }
                    }
                }

                // Add assistant message with tool calls to conversation
                let assistant_msg = Message {
                    role: cortex_engine::client::MessageRole::Assistant,
                    content: cortex_engine::client::MessageContent::Text(iteration_content.clone()),
                    tool_call_id: None,
                    tool_calls: Some(tool_calls_for_msg),
                };
                messages.push(assistant_msg);

                // Add tool results to conversation
                for (tc_id, output) in tool_results {
                    messages.push(Message::tool_result(&tc_id, &output));
                }

                // Store content for final output
                if !iteration_content.is_empty() {
                    final_content = iteration_content;
                }
            }

            // Build output with metadata
            let tools_summary = if tool_calls_executed.is_empty() {
                "No tools executed".to_string()
            } else {
                tool_calls_executed.join("\n")
            };

            // Handle case where LLM produced no text output
            let effective_content = if final_content.trim().is_empty() {
                if tool_calls_executed.is_empty() {
                    format!(
                        "The {} subagent completed but produced no output or tool calls. \
                         This may indicate an issue with the task or model response.",
                        subagent_type
                    )
                } else {
                    let success_count = tool_calls_executed
                        .iter()
                        .filter(|s| s.contains("success"))
                        .count();
                    let error_count = tool_calls_executed
                        .iter()
                        .filter(|s| s.contains("error") || s.contains("failed"))
                        .count();
                    format!(
                        "The {} subagent completed {} tool call(s) ({} successful, {} failed) \
                         but did not provide a textual summary. Task: {}",
                        subagent_type,
                        tool_calls_executed.len(),
                        success_count,
                        error_count,
                        description
                    )
                }
            } else {
                final_content
            };

            let output = format!(
                "{}\n\n\
                 Tools executed:\n{}\n\n\
                 <task_metadata>\n\
                 session_id: subagent_{}\n\
                 agent_type: {}\n\
                 description: {}\n\
                 </task_metadata>",
                effective_content, tools_summary, id, subagent_type, description
            );

            let duration = started_at.elapsed();

            if let Err(e) = tool_tx
                .send(ToolEvent::Completed {
                    id: id.clone(),
                    name: "Task".to_string(),
                    output: output.clone(),
                    success: true,
                    duration,
                })
                .await
            {
                tracing::error!(
                    "CRITICAL: Failed to send ToolEvent::Completed for subagent {}: {:?}. Output was: {}",
                    id,
                    e,
                    &output[..output.len().min(500)]
                );
            }
        });

        self.running_tool_tasks.insert(tool_call_id, task);
    }

    /// Handles events from background subagent execution tasks.
    pub(super) async fn _handle_subagent_event(&mut self, event: SubagentEvent) {
        use crate::app::SubagentDisplayStatus;

        match event {
            SubagentEvent::Progress(progress) => {
                let session_id = progress.session_id().to_string();

                match &progress {
                    ProgressEvent::Started {
                        agent_type,
                        description,
                        ..
                    } => {
                        tracing::debug!(
                            "Subagent started: {} ({}) - {}",
                            session_id,
                            agent_type,
                            description
                        );
                        self.app_state.update_subagent(&session_id, |task| {
                            task.status = SubagentDisplayStatus::Starting;
                            task.current_activity = format!("Starting {} agent", agent_type);
                        });
                    }

                    ProgressEvent::Thinking { turn_number, .. } => {
                        tracing::trace!("Subagent thinking: {} turn {}", session_id, turn_number);
                        self.app_state.update_subagent(&session_id, |task| {
                            task.status = SubagentDisplayStatus::Thinking;
                            task.current_activity = format!("Thinking (turn {})", turn_number);
                        });
                    }

                    ProgressEvent::ToolCallStarted { tool_name, .. } => {
                        tracing::debug!("Subagent calling tool: {} - {}", session_id, tool_name);
                        self.app_state.update_subagent(&session_id, |task| {
                            task.status = SubagentDisplayStatus::ExecutingTool(tool_name.clone());
                            task.current_activity = format!("Running {}", tool_name);
                        });
                    }

                    ProgressEvent::ToolCallCompleted {
                        tool_name, success, ..
                    } => {
                        tracing::debug!(
                            "Subagent tool completed: {} - {} (success: {})",
                            session_id,
                            tool_name,
                            success
                        );
                        self.app_state.update_subagent(&session_id, |task| {
                            task.tool_calls.push((tool_name.clone(), *success));
                            task.status = SubagentDisplayStatus::Thinking;
                            task.current_activity = "Thinking...".to_string();
                        });
                    }

                    ProgressEvent::TextOutput { content, .. } => {
                        self.app_state.update_subagent(&session_id, |task| {
                            task.output_preview = content.chars().take(200).collect();
                        });
                    }

                    ProgressEvent::TurnCompleted {
                        turn_number,
                        tool_calls_count,
                        ..
                    } => {
                        tracing::debug!(
                            "Subagent turn completed: {} - turn {} ({} tool calls)",
                            session_id,
                            turn_number,
                            tool_calls_count
                        );
                    }

                    ProgressEvent::Warning { message, .. } => {
                        tracing::warn!("Subagent warning: {} - {}", session_id, message);
                        self.app_state.update_subagent(&session_id, |task| {
                            task.current_activity = format!("Warning: {}", message);
                        });
                    }

                    ProgressEvent::TodoUpdated { todos, .. } => {
                        use crate::app::{SubagentTodoItem, SubagentTodoStatus};
                        tracing::debug!(
                            "Subagent todos updated: {} - {} items",
                            session_id,
                            todos.len()
                        );
                        self.app_state.update_subagent(&session_id, |task| {
                            task.todos = todos
                                .iter()
                                .map(|(content, status)| {
                                    let todo_status = match status.as_str() {
                                        "completed" => SubagentTodoStatus::Completed,
                                        "in_progress" => SubagentTodoStatus::InProgress,
                                        _ => SubagentTodoStatus::Pending,
                                    };
                                    SubagentTodoItem::new(content.clone(), todo_status)
                                })
                                .collect();
                        });
                    }

                    _ => {}
                }
            }

            SubagentEvent::Completed {
                session_id,
                output,
                tool_call_id,
            } => {
                tracing::info!("Subagent completed: {}", session_id);
                self._handle_subagent_completed(session_id, output, tool_call_id)
                    .await;
            }

            SubagentEvent::Failed {
                session_id,
                error,
                tool_call_id,
            } => {
                tracing::error!("Subagent failed: {} - {}", session_id, error);
                self._handle_subagent_failed(session_id, error, tool_call_id)
                    .await;
            }
        }
    }

    /// Handle subagent completion
    async fn _handle_subagent_completed(
        &mut self,
        session_id: String,
        output: String,
        tool_call_id: String,
    ) {
        use crate::app::SubagentDisplayStatus;

        // Update status before removing
        self.app_state.update_subagent(&session_id, |task| {
            task.status = SubagentDisplayStatus::Completed;
            task.current_activity = "Completed".to_string();
        });

        // Get task info for result formatting
        let task_info = self
            .app_state
            .active_subagents
            .iter()
            .find(|t| t.session_id == session_id)
            .map(|t| {
                (
                    t.agent_type.clone(),
                    t.description.clone(),
                    t.elapsed(),
                    t.tool_calls.len(),
                )
            });

        // Remove from running tasks
        self.running_subagents.remove(&session_id);

        if let Some((agent_type, description, elapsed, tool_count)) = task_info {
            // Format result for the LLM
            let result_output = format!(
                "## Subagent Completed\n\n\
                **Type:** {}\n\
                **Description:** {}\n\
                **Duration:** {:.1}s\n\
                **Tool Calls:** {}\n\n\
                ## Output\n\n{}",
                agent_type,
                description,
                elapsed.as_secs_f64(),
                tool_count,
                output
            );

            // Add as pending tool result for continuation
            self.app_state.add_pending_tool_result(
                tool_call_id,
                "Task".to_string(),
                result_output,
                true,
            );
        }

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
            }
            self.stream_done_received = true;
        }

        // Stop delegation mode if no more active subagents
        if !self.app_state.has_active_subagents() {
            self.app_state.streaming.stop_delegation();
        }

        // Continue agentic loop if no more subagents or tools running
        if self.running_subagents.is_empty() && self.running_tool_tasks.is_empty() {
            if self.app_state.has_pending_tool_results() {
                let _ = self.continue_with_tool_results().await;
            } else if self.app_state.has_queued_messages() {
                let _ = self.process_message_queue().await;
            } else {
                self.app_state.streaming.full_reset();
            }
        }
    }

    /// Handle subagent failure
    async fn _handle_subagent_failed(
        &mut self,
        session_id: String,
        error: String,
        tool_call_id: String,
    ) {
        use crate::app::SubagentDisplayStatus;

        // Update status before removing
        self.app_state.update_subagent(&session_id, |task| {
            task.status = SubagentDisplayStatus::Failed;
            task.current_activity = format!("Failed: {}", error);
        });

        // Get task info for result formatting
        let task_info = self
            .app_state
            .active_subagents
            .iter()
            .find(|t| t.session_id == session_id)
            .map(|t| (t.agent_type.clone(), t.description.clone(), t.elapsed()));

        // Remove from running tasks
        self.running_subagents.remove(&session_id);

        if let Some((agent_type, description, elapsed)) = task_info {
            // Format error for the LLM
            let error_output = format!(
                "## Subagent Failed\n\n\
                **Type:** {}\n\
                **Description:** {}\n\
                **Duration:** {:.1}s\n\n\
                ## Error\n\n{}",
                agent_type,
                description,
                elapsed.as_secs_f64(),
                error
            );

            // Add as pending tool result
            self.app_state.add_pending_tool_result(
                tool_call_id,
                "Task".to_string(),
                error_output,
                false,
            );
        }

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
            }
            self.stream_done_received = true;
        }

        // Stop delegation mode if no more active subagents
        if !self.app_state.has_active_subagents() {
            self.app_state.streaming.stop_delegation();
        }

        // Continue agentic loop if no more subagents or tools running
        if self.running_subagents.is_empty() && self.running_tool_tasks.is_empty() {
            if self.app_state.has_pending_tool_results() {
                let _ = self.continue_with_tool_results().await;
            } else if self.app_state.has_queued_messages() {
                let _ = self.process_message_queue().await;
            } else {
                self.app_state.streaming.full_reset();
            }
        }
    }
}
