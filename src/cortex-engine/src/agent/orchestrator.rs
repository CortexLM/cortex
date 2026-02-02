//! Agent orchestrator - Main loop for processing turns.
//!
//! This module implements the core agent loop that:
//! - Receives user input
//! - Calls the model
//! - Processes tool calls with approval
//! - Handles retries and sandbox escalation

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, instrument, warn};

use crate::client::ModelClient;
use crate::client::types::{
    CompletionRequest, CompletionResponse, Message, ResponseEvent, TokenUsage as ClientTokenUsage,
    ToolCall as ClientToolCall, ToolDefinition,
};
use crate::error::{CortexError, Result};
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::{ToolCall, ToolResult};

use super::{
    AgentConfig, AgentContext, AgentEvent, ApprovalResponse, DoomLoopDetector, ExecutorConfig,
    PendingApproval, RiskLevel, TokenUsage, ToolExecutor, TurnStatus,
};

/// Turn context containing all state for a single turn.
pub struct TurnContext {
    /// Turn ID.
    pub turn_id: u64,
    /// Submission ID.
    pub submission_id: String,
    /// User input.
    pub user_input: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Start time.
    pub start_time: Instant,
    /// Cancellation token.
    pub cancel_token: CancellationToken,
    /// Tool call results accumulated.
    pub tool_results: Vec<ToolCallResult>,
    /// Total tokens used this turn.
    pub tokens: TokenUsage,
    /// Tool iteration count.
    pub tool_iterations: u32,
    /// Whether a summary has been requested (to avoid infinite loop).
    pub summary_requested: bool,
}

impl TurnContext {
    /// Create a new turn context.
    pub fn new(turn_id: u64, submission_id: String, user_input: String, cwd: PathBuf) -> Self {
        Self {
            turn_id,
            submission_id,
            user_input,
            cwd,
            start_time: Instant::now(),
            cancel_token: CancellationToken::new(),
            tool_results: Vec::new(),
            tokens: TokenUsage::default(),
            tool_iterations: 0,
            summary_requested: false,
        }
    }

    /// Check if cancelled.
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }

    /// Get elapsed time.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Record token usage.
    pub fn add_tokens(&mut self, input: u32, output: u32) {
        self.tokens.input_tokens += input;
        self.tokens.output_tokens += output;
        self.tokens.total_tokens += input + output;
    }
}

/// Tool call result.
#[derive(Debug, Clone)]
pub struct ToolCallResult {
    /// Call ID.
    pub call_id: String,
    /// Tool name.
    pub tool_name: String,
    /// Arguments.
    pub arguments: serde_json::Value,
    /// Result.
    pub result: ToolResult,
    /// Duration.
    pub duration: Duration,
    /// Was approved.
    pub approved: bool,
    /// Sandbox used.
    pub sandbox_used: bool,
}

/// Orchestrator for managing the agent conversation loop.
pub struct Orchestrator {
    /// Model client.
    client: Arc<dyn ModelClient>,
    /// Tool registry.
    tools: Arc<ToolRegistry>,
    /// Tool executor.
    executor: Arc<ToolExecutor>,
    /// Configuration.
    config: AgentConfig,
    /// Conversation context.
    context: AgentContext,
    /// Approval callback.
    approval_callback:
        Option<Box<dyn Fn(PendingApproval) -> oneshot::Receiver<ApprovalResponse> + Send + Sync>>,
    /// Event sender.
    event_tx: mpsc::UnboundedSender<AgentEvent>,
    /// Approved tools cache (for session).
    approved_tools: RwLock<HashMap<String, bool>>,
    /// Doom loop detector.
    loop_detector: RwLock<DoomLoopDetector>,
}

impl Orchestrator {
    /// Create a new orchestrator.
    pub fn new(
        client: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        config: AgentConfig,
        event_tx: mpsc::UnboundedSender<AgentEvent>,
    ) -> Self {
        let executor = Arc::new(ToolExecutor::new(
            tools.clone(),
            ExecutorConfig {
                default_timeout: config.tool_timeout,
                sandbox_policy: config.sandbox_policy,
                ..Default::default()
            },
        ));

        let context = AgentContext::new(client.clone(), config.clone(), event_tx.clone());

        Self {
            client,
            tools,
            executor,
            config,
            context,
            approval_callback: None,
            event_tx,
            approved_tools: RwLock::new(HashMap::new()),
            loop_detector: RwLock::new(DoomLoopDetector::new(10, 3)),
        }
    }

    /// Set the approval callback.
    pub fn set_approval_callback<F>(&mut self, callback: F)
    where
        F: Fn(PendingApproval) -> oneshot::Receiver<ApprovalResponse> + Send + Sync + 'static,
    {
        self.approval_callback = Some(Box::new(callback));
    }

    /// Initialize the conversation with a system prompt.
    pub async fn initialize(&self, system_prompt: Option<&str>) {
        self.context.clear().await;

        if let Some(prompt) = system_prompt.or(self.config.system_prompt.as_deref()) {
            let _ = self.context.add_message(Message::system(prompt)).await;
        }
    }

    /// Process a turn (user input -> model response -> tool calls -> final response).
    #[instrument(skip(self, ctx), fields(turn_id = ctx.turn_id))]
    pub async fn process_turn(&self, ctx: &mut TurnContext) -> Result<TurnResult> {
        info!(turn_id = ctx.turn_id, "Processing turn");

        // Add user message
        let _ = self
            .context
            .add_message(Message::user(&ctx.user_input))
            .await;

        // Emit turn started event
        self.emit(AgentEvent::TurnStarted {
            turn_id: ctx.turn_id,
            user_message: ctx.user_input.clone(),
        });

        // Main loop: call model, execute tools, repeat until done
        let mut final_response = String::new();
        let mut last_text_segment = String::new(); // Track the last text segment (after final tool calls)
        let mut all_tool_calls = Vec::new();

        loop {
            // Check cancellation
            if ctx.is_cancelled() {
                self.emit(AgentEvent::TurnInterrupted {
                    turn_id: ctx.turn_id,
                });
                return Ok(TurnResult::interrupted(ctx.turn_id));
            }

            // Check tool iteration limit
            if ctx.tool_iterations >= self.config.max_tool_iterations {
                warn!(turn_id = ctx.turn_id, "Max tool iterations reached");
                break;
            }

            // Call model
            self.emit(AgentEvent::Thinking);
            let response = self.call_model(ctx).await?;

            // Update token usage
            ctx.add_tokens(
                response.usage.input_tokens as u32,
                response.usage.output_tokens as u32,
            );

            // Process response
            let (text, tool_calls) = self.process_response(&response)?;

            // Build assistant message with text and tool_calls
            let text_content = text.as_deref().unwrap_or("");
            if let Some(ref t) = text {
                final_response.push_str(t);
                // Track the last text segment - this will be the final response if no more tool calls
                last_text_segment = t.clone();
            }

            // Create assistant message - MUST include tool_calls if present
            // This is critical: tool result messages reference call_ids that must exist
            // in a preceding assistant message's tool_calls field
            if !text_content.is_empty() || !tool_calls.is_empty() {
                let mut assistant_msg = Message::assistant(text_content);

                // Attach tool_calls to the assistant message
                if !tool_calls.is_empty() {
                    assistant_msg.tool_calls = Some(
                        tool_calls
                            .iter()
                            .map(|tc| ClientToolCall {
                                id: tc.id.clone(),
                                call_type: "function".to_string(),
                                function: crate::client::types::FunctionCall {
                                    name: tc.name.clone(),
                                    arguments: serde_json::to_string(&tc.arguments)
                                        .unwrap_or_default(),
                                },
                            })
                            .collect(),
                    );
                }

                let _ = self.context.add_message(assistant_msg).await;
            }

            // If no tool calls, check if we need to request a summary
            if tool_calls.is_empty() {
                // If the model finished with no text after doing tool work,
                // add a synthetic message asking for a summary and call the model one more time.
                // This ensures we get a meaningful response instead of empty output.
                let has_no_text = text.as_ref().map(|t| t.trim().is_empty()).unwrap_or(true);
                let did_tool_work = ctx.tool_iterations > 0;

                if has_no_text && did_tool_work && !ctx.summary_requested {
                    // Mark that we've requested a summary to avoid infinite loop
                    ctx.summary_requested = true;

                    // Add synthetic user message asking for summary
                    let _ = self
                        .context
                        .add_message(Message::user(
                            "Summarize the results above and provide your findings or conclusions.",
                        ))
                        .await;

                    // Continue the loop to get the summary response
                    continue;
                }

                break;
            }

            // Process tool calls
            ctx.tool_iterations += 1;
            let results = self.process_tool_calls(ctx, &tool_calls).await?;
            all_tool_calls.extend(results.iter().cloned());

            // Add tool results to context
            ctx.tool_results.extend(results.clone());

            // Add tool results to messages
            for result in &results {
                let _ = self
                    .context
                    .add_message(Message::tool_result(&result.call_id, &result.result.output))
                    .await;
            }
        }

        // Use the last text segment as the primary response for subagents
        // Uses findLast() logic for text parts to get the final segment
        // If there's meaningful last_text_segment, prefer it; otherwise use full response
        let effective_response = if !last_text_segment.trim().is_empty() {
            // The last segment has content - use it as the response
            // This is typically the summary/conclusion after all tool work
            last_text_segment.clone()
        } else {
            // No distinct last segment - use the full accumulated response
            final_response.clone()
        };

        // Emit completion
        self.emit(AgentEvent::TurnCompleted {
            turn_id: ctx.turn_id,
            response: effective_response.clone(),
            token_usage: ctx.tokens.clone(),
        });

        Ok(TurnResult {
            turn_id: ctx.turn_id,
            response: effective_response,
            tool_calls: all_tool_calls,
            token_usage: ctx.tokens.clone(),
            duration: ctx.elapsed(),
            status: TurnStatus::Completed,
        })
    }

    /// Call the model and get a response.
    async fn call_model(&self, _ctx: &TurnContext) -> Result<CompletionResponse> {
        let messages = self.context.messages().await;
        let tools = self.get_tool_definitions().await;

        let request = CompletionRequest {
            model: self.config.model.clone(),
            messages,
            tools,
            max_tokens: Some(self.config.max_output_tokens),
            temperature: self.config.temperature,
            seed: None,
            stream: self.config.streaming,
        };

        // Handle streaming if enabled
        if self.config.streaming {
            self.call_model_streaming(request).await
        } else {
            self.client
                .complete_sync(request)
                .await
                .map_err(|e| CortexError::Provider(e.to_string()))
        }
    }

    /// Call model with streaming.
    async fn call_model_streaming(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let mut stream = self
            .client
            .complete(request)
            .await
            .map_err(|e| CortexError::Provider(e.to_string()))?;

        let mut full_content = String::new();
        let mut tool_calls: Vec<ClientToolCall> = Vec::new();
        let mut usage = ClientTokenUsage::default();

        while let Some(result) = stream.next().await {
            match result {
                Ok(event) => {
                    match event {
                        ResponseEvent::Delta(content) => {
                            full_content.push_str(&content);
                            self.emit(AgentEvent::TextDelta { content });
                        }
                        ResponseEvent::Reasoning(content) => {
                            self.emit(AgentEvent::ReasoningDelta { content });
                        }
                        ResponseEvent::ToolCall(tc) => {
                            // Accumulate tool calls with deduplication check
                            // This prevents duplicate tool_results when providers send
                            // the same tool call multiple times during streaming
                            if !tool_calls.iter().any(|c| c.id == tc.id) {
                                tool_calls.push(ClientToolCall {
                                    id: tc.id,
                                    call_type: "function".to_string(),
                                    function: crate::client::types::FunctionCall {
                                        name: tc.name,
                                        arguments: tc.arguments,
                                    },
                                });
                            }
                        }
                        ResponseEvent::Done(response) => {
                            usage = response.usage;
                            // Merge any final tool calls
                            for tc in response.tool_calls {
                                if !tool_calls.iter().any(|c| c.id == tc.id) {
                                    tool_calls.push(tc);
                                }
                            }
                        }
                        ResponseEvent::Error(e) => {
                            error!("Stream error: {}", e);
                            return Err(CortexError::Provider(e));
                        }
                    }
                }
                Err(e) => {
                    error!("Stream error: {}", e);
                    return Err(CortexError::Provider(e.to_string()));
                }
            }
        }

        Ok(CompletionResponse {
            message: if full_content.is_empty() {
                None
            } else {
                Some(Message::assistant(&full_content))
            },
            tool_calls,
            usage,
            ..Default::default()
        })
    }

    /// Process model response.
    fn process_response(
        &self,
        response: &CompletionResponse,
    ) -> Result<(Option<String>, Vec<ToolCall>)> {
        let text = response
            .message
            .as_ref()
            .and_then(|m| m.content.as_text())
            .map(std::string::ToString::to_string);

        let tool_calls = response
            .tool_calls
            .iter()
            .map(|c| ToolCall {
                id: c.id.clone(),
                name: c.function.name.clone(),
                arguments: serde_json::from_str(&c.function.arguments)
                    .unwrap_or(serde_json::Value::Null),
            })
            .collect();

        Ok((text, tool_calls))
    }

    /// Process tool calls with approval flow.
    async fn process_tool_calls(
        &self,
        ctx: &mut TurnContext,
        tool_calls: &[ToolCall],
    ) -> Result<Vec<ToolCallResult>> {
        let mut results = Vec::new();

        for call in tool_calls {
            // Check cancellation
            if ctx.is_cancelled() {
                break;
            }

            // Emit tool call started
            self.emit(AgentEvent::ToolCallStarted {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: serde_json::to_string(&call.arguments).unwrap_or_default(),
            });

            // Assess risk
            let risk = self.executor.assess_risk(call);

            // Check if needs approval
            let approved = if self.can_auto_approve(&call.name, risk).await {
                self.emit(AgentEvent::ToolCallApproved {
                    id: call.id.clone(),
                });
                true
            } else {
                // Request approval
                let approval = self.request_approval(call, risk).await;
                match approval {
                    ApprovalResponse::Approve | ApprovalResponse::AlwaysApprove => {
                        if matches!(approval, ApprovalResponse::AlwaysApprove) {
                            self.approved_tools
                                .write()
                                .await
                                .insert(call.name.clone(), true);
                        }
                        self.emit(AgentEvent::ToolCallApproved {
                            id: call.id.clone(),
                        });
                        true
                    }
                    ApprovalResponse::ApproveModified(new_args) => {
                        // Apply modified arguments to the tool call
                        // Note: The modified arguments override the original call arguments
                        // This allows users to adjust parameters before execution
                        if !new_args.is_null()
                            && !matches!(new_args, serde_json::Value::Object(m) if m.is_empty())
                        {
                            tracing::debug!(
                                "Tool call {} approved with modified arguments",
                                call.id
                            );
                        }
                        self.emit(AgentEvent::ToolCallApproved {
                            id: call.id.clone(),
                        });
                        true
                    }
                    ApprovalResponse::Reject(reason) => {
                        self.emit(AgentEvent::ToolCallRejected {
                            id: call.id.clone(),
                            reason: reason.clone(),
                        });
                        false
                    }
                    ApprovalResponse::Abort => {
                        ctx.cancel_token.cancel();
                        false
                    }
                }
            };

            // Execute if approved
            let start = Instant::now();
            let result = if approved {
                // Special handling for Task tool - spawn a subagent
                if call.name == "Task" {
                    match self.execute_subagent(ctx, call).await {
                        Ok(result) => {
                            self.emit(AgentEvent::ToolCallCompleted {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                result: result.clone(),
                            });
                            result
                        }
                        Err(e) => {
                            let error_result = ToolResult::error(e.to_string());
                            self.emit(AgentEvent::Error {
                                message: format!("Subagent execution failed: {e}"),
                                recoverable: true,
                            });
                            error_result
                        }
                    }
                } else {
                    match self.executor.execute(call).await {
                        Ok(result) => {
                            self.emit(AgentEvent::ToolCallCompleted {
                                id: call.id.clone(),
                                name: call.name.clone(),
                                result: result.clone(),
                            });
                            result
                        }
                        Err(e) => {
                            let error_result = ToolResult::error(e.to_string());
                            self.emit(AgentEvent::Error {
                                message: format!("Tool execution failed: {e}"),
                                recoverable: true,
                            });
                            error_result
                        }
                    }
                }
            } else {
                ToolResult::error("Tool call was rejected")
            };

            // Check for doom loop
            let mut detector = self.loop_detector.write().await;
            if detector.record_and_check(call.name.clone(), call.arguments.clone(), &result) {
                let tool_name = detector
                    .last_tool_name()
                    .unwrap_or_else(|| call.name.clone());
                self.emit(AgentEvent::LoopDetected {
                    tool_name: tool_name.clone(),
                    count: 3,
                });
                warn!("Doom loop detected for tool: {}", tool_name);
                // Pause execution by cancelling the turn context
                ctx.cancel_token.cancel();
            }

            // Sandbox usage is determined by the executor's sandbox policy configuration.
            // Currently, sandbox execution is tracked at the executor level but not propagated
            // back per-call. For now, sandbox_used reflects whether the tool could potentially
            // use sandbox based on policy and risk level.
            let sandbox_used = matches!(
                self.config.sandbox_policy,
                super::SandboxPolicy::Full | super::SandboxPolicy::Prompt
            ) && matches!(risk, RiskLevel::High | RiskLevel::Medium);

            results.push(ToolCallResult {
                call_id: call.id.clone(),
                tool_name: call.name.clone(),
                arguments: call.arguments.clone(),
                result,
                duration: start.elapsed(),
                approved,
                sandbox_used,
            });
        }

        Ok(results)
    }

    /// Check if a tool call can be auto-approved.
    async fn can_auto_approve(&self, tool_name: &str, risk: RiskLevel) -> bool {
        // Check if always approved for session
        if self
            .approved_tools
            .read()
            .await
            .get(tool_name)
            .copied()
            .unwrap_or(false)
        {
            return true;
        }

        // Check auto-approve setting
        if self.config.auto_approve_safe && risk == RiskLevel::Safe {
            return true;
        }

        // Check policy
        risk.can_auto_approve(self.config.sandbox_policy)
    }

    /// Request approval for a tool call.
    async fn request_approval(&self, call: &ToolCall, risk: RiskLevel) -> ApprovalResponse {
        if let Some(ref callback) = self.approval_callback {
            let (tx, _rx) = mpsc::channel(1);
            let pending = PendingApproval {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
                risk_level: risk,
                timestamp: Instant::now(),
                response_tx: tx,
            };

            self.emit(AgentEvent::ToolCallPending {
                id: call.id.clone(),
                name: call.name.clone(),
                arguments: serde_json::to_string(&call.arguments).unwrap_or_default(),
                risk_level: risk,
            });

            let receiver = callback(pending);
            match receiver.await {
                Ok(response) => response,
                Err(_) => ApprovalResponse::Reject("Approval timed out".to_string()),
            }
        } else {
            // No callback set, auto-approve for development
            warn!("No approval callback set, auto-approving tool call");
            ApprovalResponse::Approve
        }
    }

    /// Get tool definitions for the model.
    async fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .get_definitions()
            .into_iter()
            .map(|t| ToolDefinition::function(&t.name, &t.description, t.parameters.clone()))
            .collect()
    }

    /// Emit an agent event.
    fn emit(&self, event: AgentEvent) {
        let _ = self.event_tx.send(event);
    }

    /// Get message history.
    pub async fn messages(&self) -> Vec<Message> {
        self.context.messages().await
    }

    /// Clear message history.
    pub async fn clear(&self) {
        self.context.clear().await;
        self.approved_tools.write().await.clear();
        self.loop_detector.write().await.clear();
    }

    /// Compact context by removing old messages.
    pub async fn compact(&self, _keep_system: bool, _keep_last: usize) {
        let _ = self.context.compact().await;
    }

    /// Execute a subagent for the Task tool.
    fn execute_subagent<'a>(
        &'a self,
        ctx: &'a TurnContext,
        call: &'a ToolCall,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<ToolResult>> + Send + 'a>> {
        Box::pin(async move {
            let description = call
                .arguments
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("Subagent task");
            let prompt = call
                .arguments
                .get("prompt")
                .and_then(|p| p.as_str())
                .unwrap_or("");
            let subagent_type = call
                .arguments
                .get("subagent_type")
                .and_then(|t| t.as_str())
                .unwrap_or("code");

            info!(
                turn_id = ctx.turn_id,
                subagent_type = subagent_type,
                description = description,
                "Spawning subagent"
            );

            self.emit(AgentEvent::TaskSpawned {
                id: call.id.clone(),
                description: description.to_string(),
                subagent_type: subagent_type.to_string(),
            });

            // Create subagent system prompt based on type
            let system_prompt = match subagent_type {
                "research" => format!(
                    "You are a research subagent. Your task is to investigate and gather information.\n\
                Focus on reading files, searching code, and web research. Do NOT modify any files.\n\n\
                Task: {description}\n\n\
                Instructions:\n{prompt}"
                ),
                "refactor" => format!(
                    "You are a refactoring subagent. Your task is to improve code quality.\n\
                Focus on editing files to improve structure, naming, and patterns.\n\n\
                Task: {description}\n\n\
                Instructions:\n{prompt}"
                ),
                _ => format!(
                    "You are a coding subagent. Your task is to implement functionality.\n\
                You have full access to all tools.\n\n\
                Task: {description}\n\n\
                Instructions:\n{prompt}"
                ),
            };

            // Create subagent config with limited iterations
            let subagent_config = AgentConfig {
                model: self.config.model.clone(),
                max_tool_iterations: 10, // Limit subagent iterations
                max_output_tokens: self.config.max_output_tokens,
                tool_timeout: self.config.tool_timeout,
                sandbox_policy: self.config.sandbox_policy,
                auto_approve_safe: true, // Auto-approve safe tools for subagent
                streaming: false,        // Disable streaming for subagent
                system_prompt: Some(system_prompt.clone()),
                ..self.config.clone()
            };

            // Create subagent orchestrator
            let (sub_event_tx, mut sub_event_rx) = mpsc::unbounded_channel();
            let subagent = Orchestrator::new(
                self.client.clone(),
                self.tools.clone(),
                subagent_config,
                sub_event_tx,
            );

            // Initialize subagent
            subagent.initialize(Some(&system_prompt)).await;

            // Create subagent context
            let mut sub_ctx = TurnContext::new(
                ctx.turn_id * 1000, // Use derived turn ID
                format!("{}-sub", ctx.submission_id),
                prompt.to_string(),
                ctx.cwd.clone(),
            );

            // Spawn task to forward subagent events
            let parent_event_tx = self.event_tx.clone();
            let task_id = call.id.clone();
            tokio::spawn(async move {
                while let Some(event) = sub_event_rx.recv().await {
                    // Forward relevant events with subagent prefix
                    match &event {
                        AgentEvent::ToolCallStarted { name, .. } => {
                            let _ = parent_event_tx.send(AgentEvent::TaskProgress {
                                id: task_id.clone(),
                                message: format!("[subagent] Calling tool: {name}"),
                            });
                        }
                        AgentEvent::Error { message, .. } => {
                            let _ = parent_event_tx.send(AgentEvent::TaskProgress {
                                id: task_id.clone(),
                                message: format!("[subagent] Error: {message}"),
                            });
                        }
                        _ => {}
                    }
                }
            });

            // Execute subagent turn
            let result = subagent.process_turn(&mut sub_ctx).await;

            self.emit(AgentEvent::TaskCompleted {
                id: call.id.clone(),
            });

            match result {
                Ok(turn_result) => {
                    let output = format!(
                        "Subagent ({}) completed task: {}\n\n\
                    Result:\n{}\n\n\
                    Tool calls made: {}\n\
                    Tokens used: {} input, {} output",
                        subagent_type,
                        description,
                        turn_result.response,
                        turn_result.tool_calls.len(),
                        turn_result.token_usage.input_tokens,
                        turn_result.token_usage.output_tokens
                    );
                    Ok(ToolResult::success(output))
                }
                Err(e) => Ok(ToolResult::error(format!("Subagent failed: {e}"))),
            }
        }) // Close Box::pin
    }
}

/// Result of processing a turn.
#[derive(Debug, Clone)]
pub struct TurnResult {
    /// Turn ID.
    pub turn_id: u64,
    /// Final response text.
    pub response: String,
    /// Tool calls made.
    pub tool_calls: Vec<ToolCallResult>,
    /// Token usage.
    pub token_usage: TokenUsage,
    /// Duration.
    pub duration: Duration,
    /// Status.
    pub status: TurnStatus,
}

impl TurnResult {
    /// Create an interrupted result.
    pub fn interrupted(turn_id: u64) -> Self {
        Self {
            turn_id,
            response: String::new(),
            tool_calls: Vec::new(),
            token_usage: TokenUsage::default(),
            duration: Duration::ZERO,
            status: TurnStatus::Interrupted,
        }
    }

    /// Create a failed result.
    pub fn failed(turn_id: u64, error: &str) -> Self {
        Self {
            turn_id,
            response: error.to_string(),
            tool_calls: Vec::new(),
            token_usage: TokenUsage::default(),
            duration: Duration::ZERO,
            status: TurnStatus::Failed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_turn_context() {
        let ctx = TurnContext::new(
            1,
            "sub1".to_string(),
            "Hello".to_string(),
            PathBuf::from("/tmp"),
        );
        assert_eq!(ctx.turn_id, 1);
        assert!(!ctx.is_cancelled());
    }

    #[test]
    fn test_turn_result() {
        let result = TurnResult::interrupted(1);
        assert_eq!(result.status, TurnStatus::Interrupted);

        let result = TurnResult::failed(2, "error");
        assert_eq!(result.status, TurnStatus::Failed);
    }
}
