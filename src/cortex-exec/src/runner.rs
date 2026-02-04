//! Headless execution runner.
//!
//! This module provides a complete implementation for running Cortex in headless/non-interactive
//! mode. It supports:
//! - Full LLM conversation with tool calling
//! - Timeout enforcement
//! - Streaming output
//! - Multiple conversation turns
//! - Error recovery and graceful degradation

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use tokio_stream::StreamExt;

use cortex_engine::{
    Config, ConversationManager, CortexError,
    client::{
        CompletionRequest, CompletionResponse, FinishReason, Message, ModelClient, ResponseEvent,
        ToolCall, ToolDefinition as ClientToolDefinition, create_client,
    },
    tools::{ToolContext, ToolRouter},
};
use cortex_protocol::ConversationId;

use crate::output::{OutputFormat, OutputWriter};

/// Default timeout for the entire execution (10 minutes).
///
/// This is the maximum duration for a multi-turn exec session.
/// See `cortex_common::http_client` module documentation for the complete
/// timeout hierarchy across Cortex services.
const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Default timeout for a single LLM request (2 minutes).
///
/// Allows sufficient time for model inference while preventing indefinite hangs.
/// See `cortex_common::http_client` module documentation for the complete
/// timeout hierarchy across Cortex services.
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;

/// Maximum retries for transient errors.
const MAX_RETRIES: usize = 3;

/// Options for headless execution.
#[derive(Debug, Clone)]
pub struct ExecOptions {
    /// Prompt to execute.
    pub prompt: String,
    /// Working directory.
    pub cwd: PathBuf,
    /// Model to use.
    pub model: Option<String>,
    /// Output format.
    pub output_format: OutputFormat,
    /// Whether to auto-approve all actions.
    pub full_auto: bool,
    /// Maximum number of turns.
    pub max_turns: Option<usize>,
    /// Timeout in seconds for the entire execution.
    pub timeout_secs: Option<u64>,
    /// Timeout in seconds for a single LLM request.
    pub request_timeout_secs: Option<u64>,
    /// Sandbox mode.
    pub sandbox: bool,
    /// System prompt override.
    pub system_prompt: Option<String>,
    /// Enable streaming output.
    pub streaming: bool,
    /// Tools to enable (None = all tools).
    pub enabled_tools: Option<Vec<String>>,
    /// Tools to disable.
    pub disabled_tools: Vec<String>,
}

impl Default for ExecOptions {
    fn default() -> Self {
        Self {
            prompt: String::new(),
            cwd: std::env::current_dir().unwrap_or_default(),
            model: None,
            output_format: OutputFormat::Text,
            full_auto: false,
            max_turns: Some(10),
            timeout_secs: Some(DEFAULT_TIMEOUT_SECS),
            request_timeout_secs: Some(DEFAULT_REQUEST_TIMEOUT_SECS),
            sandbox: true,
            system_prompt: None,
            streaming: true,
            enabled_tools: None,
            disabled_tools: Vec::new(),
        }
    }
}

/// Result of headless execution.
#[derive(Debug)]
pub struct ExecResult {
    /// Conversation ID.
    pub conversation_id: ConversationId,
    /// Final response text.
    pub response: String,
    /// Number of turns executed.
    pub turns: usize,
    /// Files modified during execution.
    pub files_modified: Vec<String>,
    /// Commands executed.
    pub commands_executed: Vec<String>,
    /// Tool calls made during execution.
    pub tool_calls: Vec<ToolCallRecord>,
    /// Whether execution was successful.
    pub success: bool,
    /// Error message if failed.
    pub error: Option<String>,
    /// Total input tokens used.
    pub input_tokens: i64,
    /// Total output tokens used.
    pub output_tokens: i64,
    /// Whether execution timed out.
    pub timed_out: bool,
}

/// Record of a tool call made during execution.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    /// Tool name.
    pub name: String,
    /// Tool arguments (JSON string).
    pub arguments: String,
    /// Tool result.
    pub result: String,
    /// Whether the tool call succeeded.
    pub success: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Headless execution runner.
pub struct ExecRunner {
    config: Config,
    options: ExecOptions,
    output: OutputWriter,
    tool_router: ToolRouter,
    client: Option<Box<dyn ModelClient>>,
}

impl ExecRunner {
    /// Create a new execution runner.
    pub fn new(config: Config, options: ExecOptions) -> Self {
        let output = OutputWriter::new(options.output_format);
        let tool_router = ToolRouter::new();

        Self {
            config,
            options,
            output,
            tool_router,
            client: None,
        }
    }

    /// Create execution runner with a custom model client.
    pub fn with_client(config: Config, options: ExecOptions, client: Box<dyn ModelClient>) -> Self {
        let output = OutputWriter::new(options.output_format);
        let tool_router = ToolRouter::new();

        Self {
            config,
            options,
            output,
            tool_router,
            client: Some(client),
        }
    }

    /// Initialize the model client.
    fn init_client(&mut self) -> Result<&dyn ModelClient, CortexError> {
        if self.client.is_none() {
            let model = self
                .options
                .model
                .as_ref()
                .unwrap_or(&self.config.model)
                .clone();

            // Create client using cortex-core's factory
            let client = create_client(
                &self.config.model_provider_id,
                &model,
                "", // API key resolved from environment by the client
                None,
            )?;

            self.client = Some(client);
        }

        self.client
            .as_ref()
            .map(|c| c.as_ref())
            .ok_or_else(|| CortexError::Internal("LLM client not initialized".to_string()))
    }

    /// Get filtered tool definitions based on options.
    fn get_tool_definitions(&self) -> Vec<ClientToolDefinition> {
        let all_tools = self.tool_router.get_tool_definitions();
        let disabled: HashSet<&str> = self
            .options
            .disabled_tools
            .iter()
            .map(String::as_str)
            .collect();

        all_tools
            .into_iter()
            .filter(|tool| {
                // Check if tool is disabled
                if disabled.contains(tool.name.as_str()) {
                    return false;
                }

                // Check if only specific tools are enabled
                if let Some(enabled) = &self.options.enabled_tools {
                    return enabled.contains(&tool.name);
                }

                true
            })
            .map(|tool| {
                ClientToolDefinition::function(&tool.name, &tool.description, tool.parameters)
            })
            .collect()
    }

    /// Build the system prompt.
    fn build_system_prompt(&self) -> String {
        if let Some(custom) = &self.options.system_prompt {
            return custom.clone();
        }

        let mut prompt = String::from(
            "You are Cortex, an AI coding assistant running in headless/exec mode. \
            You have access to tools to read/write files, execute commands, and search the web. \
            Complete the user's request efficiently and thoroughly.\n\n",
        );

        // Add sandbox notice
        if self.options.sandbox {
            prompt.push_str(
                "SANDBOX MODE: You are running in sandbox mode. Some operations may be restricted.\n\n",
            );
        }

        // Add auto-approve notice
        if self.options.full_auto {
            prompt.push_str(
                "AUTO-APPROVE MODE: All tool calls will be automatically approved. \
                Be careful with destructive operations.\n\n",
            );
        }

        // Add working directory
        prompt.push_str(&format!(
            "Working Directory: {}\n",
            self.options.cwd.display()
        ));

        // Add user instructions if available
        if let Some(instructions) = &self.config.user_instructions {
            prompt.push_str("\nUser Instructions:\n");
            prompt.push_str(instructions);
            prompt.push('\n');
        }

        prompt
    }

    /// Run the execution with full timeout enforcement.
    pub async fn run(&mut self) -> Result<ExecResult, CortexError> {
        let timeout =
            Duration::from_secs(self.options.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));

        // Wrap the entire execution in a timeout
        match tokio::time::timeout(timeout, self.run_inner()).await {
            Ok(result) => result,
            Err(_) => {
                self.output.write_error(&format!(
                    "Execution timed out after {} seconds",
                    timeout.as_secs()
                ));

                Ok(ExecResult {
                    conversation_id: ConversationId::new(),
                    response: String::new(),
                    turns: 0,
                    files_modified: Vec::new(),
                    commands_executed: Vec::new(),
                    tool_calls: Vec::new(),
                    success: false,
                    error: Some(format!(
                        "Execution timed out after {} seconds",
                        timeout.as_secs()
                    )),
                    input_tokens: 0,
                    output_tokens: 0,
                    timed_out: true,
                })
            }
        }
    }

    /// Inner execution logic.
    async fn run_inner(&mut self) -> Result<ExecResult, CortexError> {
        let conversation_id = ConversationId::new();
        let mut turns = 0;
        let mut files_modified = Vec::new();
        let mut commands_executed = Vec::new();
        let mut tool_call_records = Vec::new();
        let mut final_response = String::new();
        let mut total_input_tokens: i64 = 0;
        let mut total_output_tokens: i64 = 0;

        // Validate prompt
        if self.options.prompt.trim().is_empty() {
            return Err(CortexError::InvalidInput("Prompt cannot be empty".into()));
        }

        self.output.write_info(&format!(
            "Starting execution with model: {}",
            self.options.model.as_ref().unwrap_or(&self.config.model)
        ));
        self.output
            .write_info(&format!("Conversation ID: {conversation_id}"));

        // Initialize client
        self.init_client()?;

        // Create conversation manager
        let mut conversation = ConversationManager::new(
            self.options.model.as_ref().unwrap_or(&self.config.model),
            self.options.cwd.clone(),
        );

        // Add system message
        let system_prompt = self.build_system_prompt();
        conversation.add_system_message(&system_prompt);

        // Add initial user message
        conversation.add_user_message(&self.options.prompt);

        // Get tool definitions
        let tools = self.get_tool_definitions();
        let max_turns = self.options.max_turns.unwrap_or(10);

        // Main execution loop
        while turns < max_turns {
            turns += 1;
            self.output.write_info(&format!("Turn {turns}/{max_turns}"));

            // Send request to LLM
            let response = self.send_request(&conversation, &tools).await?;

            // Update token counts
            total_input_tokens += response.usage.input_tokens;
            total_output_tokens += response.usage.output_tokens;

            // Add assistant response to conversation
            conversation.add_response(&response);

            // Extract text response
            if let Some(msg) = &response.message
                && let Some(text) = msg.content.as_text()
                && !text.is_empty()
            {
                final_response = text.to_string();
                self.output.write_response(&final_response);
            }

            // Check if we need to execute tool calls
            if response.finish_reason == FinishReason::ToolCalls && !response.tool_calls.is_empty()
            {
                self.output.write_info(&format!(
                    "Processing {} tool call(s)",
                    response.tool_calls.len()
                ));

                // Execute each tool call
                for tool_call in &response.tool_calls {
                    let record = self
                        .execute_tool_call(tool_call, &mut files_modified, &mut commands_executed)
                        .await;

                    // Add tool result to conversation
                    conversation.add_tool_result(&tool_call.id, &record.result);

                    tool_call_records.push(record);
                }

                // Continue the conversation
                continue;
            }

            // No tool calls and we got a response - we're done
            if response.finish_reason == FinishReason::Stop {
                break;
            }

            // Handle other finish reasons
            match response.finish_reason {
                FinishReason::Length => {
                    self.output
                        .write_info("Response truncated due to length limit");
                    // Could continue with a follow-up, but for now just break
                    break;
                }
                FinishReason::ContentFilter => {
                    self.output
                        .write_error("Response blocked by content filter");
                    return Ok(ExecResult {
                        conversation_id,
                        response: final_response,
                        turns,
                        files_modified,
                        commands_executed,
                        tool_calls: tool_call_records,
                        success: false,
                        error: Some("Response blocked by content filter".to_string()),
                        input_tokens: total_input_tokens,
                        output_tokens: total_output_tokens,
                        timed_out: false,
                    });
                }
                FinishReason::Error => {
                    self.output.write_error("LLM returned an error");
                    return Ok(ExecResult {
                        conversation_id,
                        response: final_response,
                        turns,
                        files_modified,
                        commands_executed,
                        tool_calls: tool_call_records,
                        success: false,
                        error: Some("LLM returned an error".to_string()),
                        input_tokens: total_input_tokens,
                        output_tokens: total_output_tokens,
                        timed_out: false,
                    });
                }
                _ => {
                    // Unknown finish reason, break to be safe
                    break;
                }
            }
        }

        // Check if we hit the turn limit
        if turns >= max_turns {
            self.output
                .write_info(&format!("Reached maximum turn limit ({max_turns})"));
        }

        self.output.write_success("Execution complete");

        Ok(ExecResult {
            conversation_id,
            response: final_response,
            turns,
            files_modified,
            commands_executed,
            tool_calls: tool_call_records,
            success: true,
            error: None,
            input_tokens: total_input_tokens,
            output_tokens: total_output_tokens,
            timed_out: false,
        })
    }

    /// Send a request to the LLM with retry logic.
    async fn send_request(
        &self,
        conversation: &ConversationManager,
        tools: &[ClientToolDefinition],
    ) -> Result<CompletionResponse, CortexError> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| CortexError::Internal("Model client not initialized".into()))?;

        let request = CompletionRequest {
            messages: conversation.messages().to_vec(),
            model: client.model().to_string(),
            max_tokens: Some(4096),
            temperature: Some(0.7),
            seed: None,
            tools: tools.to_vec(),
            stream: self.options.streaming,
        };

        let request_timeout = Duration::from_secs(
            self.options
                .request_timeout_secs
                .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS),
        );

        // Retry loop for transient errors
        let mut last_error = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                self.output.write_info(&format!(
                    "Retrying request (attempt {}/{})",
                    attempt + 1,
                    MAX_RETRIES
                ));
                // Exponential backoff
                tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt as u32))).await;
            }

            let result = tokio::time::timeout(request_timeout, async {
                if self.options.streaming {
                    self.complete_streaming(client.as_ref(), request.clone())
                        .await
                } else {
                    client.complete_sync(request.clone()).await
                }
            })
            .await;

            match result {
                Ok(Ok(response)) => return Ok(response),
                Ok(Err(e)) => {
                    if e.is_retriable() {
                        last_error = Some(e);
                        continue;
                    }
                    return Err(e);
                }
                Err(_) => {
                    self.output.write_error(&format!(
                        "Request timed out after {} seconds",
                        request_timeout.as_secs()
                    ));
                    last_error = Some(CortexError::Timeout);
                    continue;
                }
            }
        }

        Err(last_error.unwrap_or_else(|| CortexError::Internal("Unknown error".into())))
    }

    /// Complete a request with streaming, aggregating the response.
    async fn complete_streaming(
        &self,
        client: &dyn ModelClient,
        request: CompletionRequest,
    ) -> Result<CompletionResponse, CortexError> {
        let mut stream = client.complete(request).await?;

        let mut aggregated_text = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();
        let mut finish_reason = FinishReason::Stop;
        let mut usage = cortex_engine::client::TokenUsage::default();

        // Track partial tool calls being streamed
        let mut partial_tool_calls: std::collections::HashMap<String, (String, String)> =
            std::collections::HashMap::new();

        while let Some(event) = stream.next().await {
            match event? {
                ResponseEvent::Delta(delta) => {
                    if self.options.streaming {
                        self.output.write_delta(&delta);
                    }
                    aggregated_text.push_str(&delta);
                }
                ResponseEvent::ToolCall(tc) => {
                    self.output.write_tool_call(&tc.name, &tc.arguments);

                    // Accumulate tool call data
                    let entry = partial_tool_calls
                        .entry(tc.id.clone())
                        .or_insert_with(|| (tc.name.clone(), String::new()));
                    entry.1.push_str(&tc.arguments);
                }
                ResponseEvent::Reasoning(reasoning) => {
                    // Could log reasoning if needed
                    tracing::debug!("Reasoning: {}", reasoning);
                }
                ResponseEvent::Done(response) => {
                    usage = response.usage;
                    finish_reason = response.finish_reason;

                    // Use tool calls from the final response if available
                    if !response.tool_calls.is_empty() {
                        tool_calls = response.tool_calls;
                    }
                }
                ResponseEvent::Error(err) => {
                    return Err(CortexError::Provider(err));
                }
            }
        }

        // Convert accumulated partial tool calls to full tool calls if not already set
        if tool_calls.is_empty() && !partial_tool_calls.is_empty() {
            for (id, (name, arguments)) in partial_tool_calls {
                tool_calls.push(ToolCall {
                    id,
                    call_type: "function".to_string(),
                    function: cortex_engine::client::FunctionCall { name, arguments },
                });
            }
        }

        // Build the response message
        let message = if !aggregated_text.is_empty() || !tool_calls.is_empty() {
            let mut msg = Message::assistant(&aggregated_text);
            if !tool_calls.is_empty() {
                msg.tool_calls = Some(tool_calls.clone());
            }
            Some(msg)
        } else {
            None
        };

        Ok(CompletionResponse {
            message,
            usage,
            finish_reason,
            tool_calls,
        })
    }

    /// Execute a single tool call.
    async fn execute_tool_call(
        &self,
        tool_call: &ToolCall,
        files_modified: &mut Vec<String>,
        commands_executed: &mut Vec<String>,
    ) -> ToolCallRecord {
        let start = std::time::Instant::now();
        let tool_name = &tool_call.function.name;
        let arguments = &tool_call.function.arguments;

        self.output.write_tool_call(tool_name, arguments);

        // Parse arguments
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => {
                let error_msg = format!("Failed to parse tool arguments: {e}");
                self.output.write_tool_result(tool_name, &error_msg, false);
                return ToolCallRecord {
                    name: tool_name.clone(),
                    arguments: arguments.clone(),
                    result: error_msg,
                    success: false,
                    duration_ms: start.elapsed().as_millis() as u64,
                };
            }
        };

        // Create tool context
        let context = ToolContext::new(self.options.cwd.clone())
            .with_auto_approve(self.options.full_auto)
            .with_conversation_id(ConversationId::new().to_string())
            .with_call_id(&tool_call.id);

        // Execute the tool
        let result = self
            .tool_router
            .execute(tool_name, args.clone(), &context)
            .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(tool_result) => {
                // Track modifications
                if let Some(metadata) = &tool_result.metadata {
                    for file in &metadata.files_modified {
                        if !files_modified.contains(file) {
                            files_modified.push(file.clone());
                        }
                    }
                }

                // Track command executions
                if tool_name == "Execute"
                    && let Some(cmd) = args.get("command")
                {
                    let cmd_str = if let Some(arr) = cmd.as_array() {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    } else {
                        cmd.to_string()
                    };
                    commands_executed.push(cmd_str);
                }

                self.output
                    .write_tool_result(tool_name, &tool_result.output, tool_result.success);

                ToolCallRecord {
                    name: tool_name.clone(),
                    arguments: arguments.clone(),
                    result: tool_result.output,
                    success: tool_result.success,
                    duration_ms,
                }
            }
            Err(e) => {
                let error_msg = format!("Tool execution failed: {e}");
                self.output.write_tool_result(tool_name, &error_msg, false);

                ToolCallRecord {
                    name: tool_name.clone(),
                    arguments: arguments.clone(),
                    result: error_msg,
                    success: false,
                    duration_ms,
                }
            }
        }
    }

    /// Run with a specific prompt.
    pub async fn run_prompt(&mut self, prompt: &str) -> Result<ExecResult, CortexError> {
        self.options.prompt = prompt.to_string();
        self.run().await
    }

    /// Get mutable reference to options for configuration.
    pub fn options_mut(&mut self) -> &mut ExecOptions {
        &mut self.options
    }

    /// Get reference to options.
    pub fn options(&self) -> &ExecOptions {
        &self.options
    }

    /// Get reference to config.
    pub fn config(&self) -> &Config {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_exec_runner_empty_prompt() {
        let config = Config::default();
        let options = ExecOptions {
            prompt: "".to_string(),
            ..Default::default()
        };

        let mut runner = ExecRunner::new(config, options);
        let result = runner.run().await;

        assert!(result.is_err());
        if let Err(CortexError::InvalidInput(msg)) = result {
            assert!(msg.contains("empty"));
        }
    }

    #[tokio::test]
    async fn test_exec_options_defaults() {
        let opts = ExecOptions::default();

        assert!(opts.prompt.is_empty());
        assert!(opts.sandbox);
        assert_eq!(opts.max_turns, Some(10));
        assert_eq!(opts.timeout_secs, Some(DEFAULT_TIMEOUT_SECS));
        assert!(!opts.full_auto);
        assert!(opts.streaming);
    }

    #[test]
    fn test_build_system_prompt() {
        let config = Config::default();
        let options = ExecOptions {
            sandbox: true,
            full_auto: true,
            ..Default::default()
        };

        let runner = ExecRunner::new(config, options);
        let prompt = runner.build_system_prompt();

        assert!(prompt.contains("Cortex"));
        assert!(prompt.contains("SANDBOX MODE"));
        assert!(prompt.contains("AUTO-APPROVE MODE"));
    }

    #[test]
    fn test_tool_filtering() {
        let config = Config::default();
        let options = ExecOptions {
            disabled_tools: vec!["Execute".to_string()],
            ..Default::default()
        };

        let runner = ExecRunner::new(config, options);
        let tools = runner.get_tool_definitions();

        // Execute should be filtered out
        assert!(!tools.iter().any(|t| t.name() == "Execute"));
    }

    #[test]
    fn test_tool_enabled_filter() {
        let config = Config::default();
        let options = ExecOptions {
            enabled_tools: Some(vec!["Read".to_string(), "LS".to_string()]),
            ..Default::default()
        };

        let runner = ExecRunner::new(config, options);
        let tools = runner.get_tool_definitions();

        // Only Read and LS should be present
        assert!(tools.iter().all(|t| t.name() == "Read" || t.name() == "LS"));
    }
}
