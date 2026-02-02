//! Agent loop - main model interaction loop.

use std::sync::atomic::Ordering;

use tokio_stream::StreamExt;

use cortex_protocol::{
    AgentMessageDeltaEvent, AgentMessageEvent, ErrorEvent, EventMsg, ExecApprovalRequestEvent,
    ExecCommandBeginEvent, ExecCommandEndEvent, ExecCommandOutputDeltaEvent, ExecCommandSource,
    ExecOutputStream, ParsedCommand, SessionConfiguredEvent, TaskCompleteEvent, TokenCountEvent,
    TokenUsage, TokenUsageInfo,
};

use crate::client::{
    CompletionRequest, Message, ResponseEvent, ToolCall, ToolDefinition as ClientToolDefinition,
};
use crate::error::{CortexError, Result};
use crate::tools::ToolContext;
use crate::tools::context::ToolOutputChunk;

use super::Session;
use super::types::PendingToolCall;

impl Session {
    /// Run the main session loop, processing submissions.
    pub async fn run(&mut self) -> Result<()> {
        use std::path::PathBuf;

        // Emit session configured event
        self.emit(EventMsg::SessionConfigured(Box::new(
            SessionConfiguredEvent {
                session_id: self.conversation_id,
                parent_session_id: None,
                model: self.config.model.clone(),
                model_provider_id: self.config.model_provider_id.clone(),
                approval_policy: self.config.approval_policy,
                sandbox_policy: self.config.sandbox_policy.clone(),
                cwd: self.config.cwd.clone(),
                reasoning_effort: None,
                history_log_id: 0,
                history_entry_count: 0,
                initial_messages: None,
                rollout_path: PathBuf::new(),
            },
        )))
        .await;

        while self.running {
            // Check cancellation flag periodically using a timeout
            let submission = tokio::select! {
                result = self.submission_rx.recv() => {
                    match result {
                        Ok(s) => s,
                        Err(_) => break,
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    // Check if we should exit due to cancellation
                    if self.cancelled.load(Ordering::SeqCst) {
                        tracing::info!("Session detected cancellation, exiting run loop");
                        self.running = false;
                        break;
                    }
                    continue;
                }
            };

            if let Err(e) = self.handle_submission(submission).await {
                self.emit(EventMsg::Error(ErrorEvent {
                    message: e.to_string(),
                    cortex_error_info: None,
                }))
                .await;
            }
        }

        Ok(())
    }

    /// Capture a snapshot of the current workspace.
    #[allow(dead_code)]
    pub(super) async fn capture_snapshot(
        &self,
        description: &str,
    ) -> Result<crate::tasks::snapshot::Snapshot> {
        let mut snapshot = self.snapshot_manager.create(description).await;
        snapshot.turn_id = Some(self.turn_id.to_string());

        // Walk workspace and capture files
        // For performance, we only capture files that are likely to be changed
        // or we could use a more sophisticated approach.
        // For now, let's capture everything in CWD except ignored patterns.
        let mut it = walkdir::WalkDir::new(&self.config.cwd).into_iter();
        loop {
            // Check for cancellation periodically
            if self.cancelled.load(Ordering::SeqCst) {
                tracing::info!("Snapshot capture cancelled by user");
                return Err(CortexError::Cancelled);
            }

            let entry = match it.next() {
                None => break,
                Some(Err(_)) => continue,
                Some(Ok(e)) => e,
            };

            let path = entry.path();
            if path.is_dir() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == ".git"
                    || name == "target"
                    || name == "node_modules"
                    || name == ".factory"
                {
                    it.skip_current_dir();
                }
                continue;
            }

            // Skip large files or binary files if needed
            // For now, just capture
            if let Err(e) = snapshot.capture_file(path) {
                tracing::warn!("Failed to capture file {}: {}", path.display(), e);
            }
        }

        Ok(snapshot)
    }

    /// Run the agent loop until completion or interruption.
    pub(super) async fn run_agent_loop(&mut self, _turn_id: &str) -> Result<()> {
        let max_iterations = 200;
        let mut iteration = 0;

        loop {
            // Check for cancellation
            if self.cancelled.load(Ordering::SeqCst) {
                tracing::info!("Agent loop cancelled by user");
                break;
            }

            iteration += 1;
            if iteration > max_iterations {
                self.emit(EventMsg::Error(ErrorEvent {
                    message: "Maximum iterations reached".to_string(),
                    cortex_error_info: None,
                }))
                .await;
                break;
            }

            // Build completion request
            let tool_defs = self.tool_router.get_tool_definitions();

            // Calculate precise context tokens
            let client_tools: Vec<ClientToolDefinition> = tool_defs
                .iter()
                .map(|t| {
                    ClientToolDefinition::function(&t.name, &t.description, t.parameters.clone())
                })
                .collect();

            let context_tokens = self
                .token_counter
                .count_messages(&self.config.model, &self.messages)
                .await
                .unwrap_or(0);
            let tools_tokens = self
                .token_counter
                .count_tools(&self.config.model, &client_tools)
                .await
                .unwrap_or(0);
            let total_context_tokens = context_tokens + tools_tokens;

            // Emit token count event before starting request
            self.emit(EventMsg::TokenCount(TokenCountEvent {
                info: Some(TokenUsageInfo {
                    total_token_usage: self.total_usage.clone(),
                    last_token_usage: TokenUsage::default(),
                    model_context_window: self.config.model_context_window,
                    context_tokens: total_context_tokens as i64,
                }),
                rate_limits: None,
            }))
            .await;

            let tools = client_tools;

            let request = CompletionRequest {
                model: self.config.model.clone(),
                messages: self.messages.clone(),
                max_tokens: Some(4096),
                // Use CLI-provided temperature or default to 0.7
                temperature: Some(self.config.temperature.unwrap_or(0.7)),
                seed: None,
                tools,
                stream: true,
            };

            // Get streaming response
            let mut stream = self.client.complete(request).await?;

            let mut full_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();

            // Process stream
            while let Some(event) = stream.next().await {
                // Check for cancellation during streaming
                if self.cancelled.load(Ordering::SeqCst) {
                    tracing::info!("Stream processing cancelled by user");
                    return Ok(());
                }

                match event? {
                    ResponseEvent::Delta(delta) => {
                        full_content.push_str(&delta);
                        self.emit(EventMsg::AgentMessageDelta(AgentMessageDeltaEvent {
                            delta,
                        }))
                        .await;
                    }
                    ResponseEvent::ToolCall(tc) => {
                        tracing::info!("Session received tool call: {} (id: {})", tc.name, tc.id);
                        tool_calls.push(ToolCall {
                            id: tc.id,
                            call_type: "function".to_string(),
                            function: crate::client::FunctionCall {
                                name: tc.name,
                                arguments: tc.arguments,
                            },
                        });
                    }
                    ResponseEvent::Done(response) => {
                        // Update token usage
                        self.total_usage.input_tokens += response.usage.input_tokens;
                        self.total_usage.output_tokens += response.usage.output_tokens;
                        self.total_usage.total_tokens += response.usage.total_tokens;

                        self.emit(EventMsg::TokenCount(TokenCountEvent {
                            info: Some(TokenUsageInfo {
                                total_token_usage: self.total_usage.clone(),
                                last_token_usage: TokenUsage {
                                    input_tokens: response.usage.input_tokens,
                                    output_tokens: response.usage.output_tokens,
                                    total_tokens: response.usage.total_tokens,
                                    ..Default::default()
                                },
                                model_context_window: self.config.model_context_window,
                                context_tokens: response.usage.input_tokens
                                    + response.usage.output_tokens,
                            }),
                            rate_limits: None,
                        }))
                        .await;
                    }
                    ResponseEvent::Error(e) => {
                        self.emit(EventMsg::Error(ErrorEvent {
                            message: e,
                            cortex_error_info: None,
                        }))
                        .await;
                        return Ok(());
                    }
                    _ => {}
                }
            }

            // Emit full message if we have content
            if !full_content.is_empty() {
                self.emit(EventMsg::AgentMessage(AgentMessageEvent {
                    id: None,
                    parent_id: None,
                    message: full_content.clone(),
                    finish_reason: None,
                }))
                .await;
            }

            // Add assistant message to history
            let mut assistant_msg = Message::assistant(&full_content);
            if !tool_calls.is_empty() {
                assistant_msg.tool_calls = Some(tool_calls.clone());
            }
            self.messages.push(assistant_msg);

            // If no tool calls, we're done
            if tool_calls.is_empty() {
                break;
            }

            // Execute tool calls
            tracing::info!("Processing {} tool calls", tool_calls.len());
            for tool_call in tool_calls {
                // Check for cancellation before each tool
                if self.cancelled.load(Ordering::SeqCst) {
                    tracing::info!("Tool execution cancelled by user");
                    return Ok(());
                }

                let tool_name = &tool_call.function.name;
                tracing::info!("Processing tool call: {} (id: {})", tool_name, tool_call.id);
                let args: serde_json::Value = serde_json::from_str(&tool_call.function.arguments)
                    .unwrap_or(serde_json::Value::Null);

                // Check if approval is needed for shell commands
                let needs_approval = if tool_name == "Execute" {
                    if let Some(cmd_array) = args.get("command").and_then(|c| c.as_array()) {
                        let cmd: Vec<String> = cmd_array
                            .iter()
                            .filter_map(|v| v.as_str().map(std::string::ToString::to_string))
                            .collect();

                        let analysis = crate::safety::analyze_command(&cmd, &self.config.cwd);
                        let requires = crate::safety::requires_approval(
                            &analysis,
                            &self.config.approval_policy,
                        );

                        if requires {
                            // Emit approval request
                            self.emit(EventMsg::ExecApprovalRequest(ExecApprovalRequestEvent {
                                call_id: tool_call.id.clone(),
                                turn_id: self.turn_id.to_string(),
                                command: cmd.clone(),
                                cwd: self.config.cwd.clone(),
                                sandbox_assessment: None,
                            }))
                            .await;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                // If approval needed, store pending and wait for user response
                tracing::info!("needs_approval for {}: {}", tool_name, needs_approval);
                if needs_approval {
                    tracing::info!("Tool {} requires approval, storing pending", tool_name);
                    self.pending_approvals.insert(
                        tool_call.id.clone(),
                        PendingToolCall {
                            tool_name: tool_name.clone(),
                            arguments: args,
                            tool_call_id: tool_call.id.clone(),
                        },
                    );
                    // Return early - we'll continue when approval comes
                    return Ok(());
                }
                tracing::info!("Tool {} does NOT require approval, executing", tool_name);

                // Handle PatchApply events
                if tool_name == "ApplyPatch" {
                    if let Some(patch) = args.get("patch").and_then(|p| p.as_str()) {
                        if let Ok(file_changes) =
                            crate::tools::handlers::apply_patch::parse_unified_diff(patch)
                        {
                            let mut protocol_changes = std::collections::HashMap::new();
                            for change in file_changes {
                                if let Some(path) = change.new_path.or(change.old_path) {
                                    let protocol_change = if change.is_new_file {
                                        cortex_protocol::FileChange::Add {
                                            content: String::new(),
                                        }
                                    } else if change.is_deleted {
                                        cortex_protocol::FileChange::Delete {
                                            content: String::new(),
                                        }
                                    } else {
                                        cortex_protocol::FileChange::Update {
                                            unified_diff: String::new(),
                                            move_path: None,
                                        }
                                    };
                                    protocol_changes.insert(path, protocol_change);
                                }
                            }
                            self.emit(EventMsg::PatchApplyBegin(
                                cortex_protocol::PatchApplyBeginEvent {
                                    call_id: tool_call.id.clone(),
                                    turn_id: self.turn_id.to_string(),
                                    auto_approved: true,
                                    changes: protocol_changes,
                                },
                            ))
                            .await;
                        }
                    }
                }

                // Get command for event (for shell tools, parse the command array)
                let command_for_event: Vec<String> = if tool_name == "Execute" {
                    args.get("command")
                        .and_then(|c| c.as_array())
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_else(|| vec![tool_name.clone()])
                } else {
                    vec![tool_name.clone()]
                };

                // Emit ExecCommandBegin event
                let exec_start = std::time::Instant::now();
                self.emit(EventMsg::ExecCommandBegin(ExecCommandBeginEvent {
                    call_id: tool_call.id.clone(),
                    turn_id: self.turn_id.to_string(),
                    command: command_for_event.clone(),
                    cwd: self.config.cwd.clone(),
                    parsed_cmd: vec![ParsedCommand {
                        program: command_for_event.first().cloned().unwrap_or_default(),
                        args: command_for_event.iter().skip(1).cloned().collect(),
                    }],
                    source: ExecCommandSource::Agent,
                    interaction_input: None,
                    tool_name: Some(tool_name.clone()),
                    tool_arguments: Some(args.clone()),
                }))
                .await;

                // Create channel for streaming output
                let (output_tx, mut output_rx) =
                    tokio::sync::mpsc::channel::<(String, ToolOutputChunk)>(100);

                // Execute tool with streaming context
                let context = ToolContext::new(self.config.cwd.clone())
                    .with_sandbox_policy(self.config.sandbox_policy.clone())
                    .with_turn_id(self.turn_id.to_string())
                    .with_conversation_id(self.conversation_id.to_string())
                    .with_call_id(tool_call.id.clone())
                    .with_output_sender(output_tx)
                    .with_lsp(self.lsp.clone());

                // Clone event sender for the streaming task
                let event_tx = self.event_tx.clone();
                let turn_id = self.turn_id;

                // Spawn task to forward output chunks as events
                let streaming_task = tokio::spawn(async move {
                    while let Some((call_id, chunk)) = output_rx.recv().await {
                        let (stream, data) = match chunk {
                            ToolOutputChunk::Stdout(s) => (ExecOutputStream::Stdout, s),
                            ToolOutputChunk::Stderr(s) => (ExecOutputStream::Stderr, s),
                        };

                        // Encode chunk as base64
                        use base64::Engine;
                        let chunk_b64 =
                            base64::engine::general_purpose::STANDARD.encode(data.as_bytes());

                        let event = cortex_protocol::Event {
                            id: turn_id.to_string(),
                            msg: EventMsg::ExecCommandOutputDelta(ExecCommandOutputDeltaEvent {
                                call_id,
                                stream,
                                chunk: chunk_b64,
                            }),
                        };

                        let _ = event_tx.send(event).await;
                    }
                });

                tracing::info!("About to execute tool {} via tool_router", tool_name);
                let result = self
                    .tool_router
                    .execute(tool_name, args.clone(), &context)
                    .await;
                match &result {
                    Ok(r) => tracing::info!(
                        "Tool {} succeeded: {:?}",
                        tool_name,
                        r.output.chars().take(100).collect::<String>()
                    ),
                    Err(e) => tracing::error!("Tool {} FAILED: {}", tool_name, e),
                }

                // Drop the context to close the output channel
                drop(context);

                // Wait for streaming to finish (will complete now that sender is dropped)
                let _ = streaming_task.await;
                tracing::info!("Streaming task completed for tool {}", tool_name);

                let (result_text, exit_code, metadata) = match &result {
                    Ok(r) => {
                        let meta = r.metadata.as_ref().and_then(|m| m.data.clone());
                        (r.output.clone(), if r.success { 0 } else { 1 }, meta)
                    }
                    Err(e) => (format!("Error: {e}"), 1, None),
                };

                // Emit ExecCommandEnd event
                tracing::info!("Emitting ExecCommandEnd for tool {}", tool_call.id);
                let duration_ms = exec_start.elapsed().as_millis() as u64;
                self.emit(EventMsg::ExecCommandEnd(Box::new(ExecCommandEndEvent {
                    call_id: tool_call.id.clone(),
                    turn_id: self.turn_id.to_string(),
                    command: command_for_event.clone(),
                    cwd: self.config.cwd.clone(),
                    parsed_cmd: vec![ParsedCommand {
                        program: command_for_event.first().cloned().unwrap_or_default(),
                        args: command_for_event.iter().skip(1).cloned().collect(),
                    }],
                    source: ExecCommandSource::Agent,
                    interaction_input: None,
                    stdout: result_text.clone(),
                    stderr: String::new(),
                    aggregated_output: result_text.clone(),
                    exit_code,
                    duration_ms,
                    formatted_output: result_text.clone(),
                    metadata,
                })))
                .await;

                // Add tool result to messages
                self.messages
                    .push(Message::tool_result(&tool_call.id, &result_text));

                // Handle PatchApplyEnd
                if tool_name == "ApplyPatch" {
                    self.emit(EventMsg::PatchApplyEnd(
                        cortex_protocol::PatchApplyEndEvent {
                            call_id: tool_call.id.clone(),
                            turn_id: self.turn_id.to_string(),
                            stdout: result_text.clone(),
                            stderr: String::new(),
                            success: exit_code == 0,
                            changes: std::collections::HashMap::new(),
                        },
                    ))
                    .await;
                }
            }
        }

        // Emit task complete
        let last_msg = self
            .messages
            .last()
            .and_then(|m| m.content.as_text())
            .map(std::string::ToString::to_string);

        self.emit(EventMsg::TaskComplete(TaskCompleteEvent {
            last_agent_message: last_msg,
        }))
        .await;

        Ok(())
    }
}
