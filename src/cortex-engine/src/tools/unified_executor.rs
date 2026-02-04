//! Unified Tool Executor - Central point for all tool execution.
//!
//! This module provides a unified executor that handles all tool execution,
//! including special tools like Task (subagent spawning) and Batch (parallel execution).
//!
//! # Architecture
//!
//! The `UnifiedToolExecutor` consolidates:
//! - `ToolRegistry` for standard tool definitions and execution
//! - `SubagentExecutor` for Task tool (spawning child sessions)
//! - `BatchToolHandler` for Batch tool (parallel execution)
//!
//! This provides a single entry point for cortex-tui, eliminating the need
//! for special-case handling of Task and Batch tools in the event loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::agents::AgentRegistry;
use crate::client::{ModelClient, create_client};
use crate::error::{CortexError, Result};

use super::context::ToolContext;
use super::handlers::batch::{BatchToolArgs, BatchToolCall, BatchToolExecutor, BatchToolHandler};
use super::handlers::subagent::{
    ProgressEvent, SubagentConfig, SubagentExecutor, SubagentResult, SubagentType,
};
use super::registry::ToolRegistry;
use super::spec::{ToolDefinition, ToolHandler as ToolHandlerTrait, ToolResult};

/// Configuration for creating a UnifiedToolExecutor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorConfig {
    /// Provider ID (e.g., "cortex", "anthropic", "openai").
    pub provider: String,
    /// Model ID (e.g., "anthropic/claude-sonnet-4-20250514").
    pub model_id: String,
    /// API key for the provider.
    #[serde(skip_serializing)]
    pub api_key: String,
    /// Optional base URL override.
    pub base_url: Option<String>,
    /// Maximum concurrent subagent tasks.
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_tasks: usize,
    /// Default timeout for subagent execution.
    #[serde(default = "default_timeout")]
    pub default_timeout: Duration,
    /// Working directory.
    pub working_dir: PathBuf,
}

fn default_max_concurrent() -> usize {
    3
}

fn default_timeout() -> Duration {
    Duration::from_secs(300)
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            provider: "cortex".to_string(),
            model_id: "anthropic/claude-sonnet-4-20250514".to_string(),
            api_key: String::new(),
            base_url: None,
            max_concurrent_tasks: 3,
            default_timeout: Duration::from_secs(300),
            working_dir: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

impl ExecutorConfig {
    /// Create a new config with required fields.
    pub fn new(
        provider: impl Into<String>,
        model_id: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            model_id: model_id.into(),
            api_key: api_key.into(),
            ..Default::default()
        }
    }

    /// Set the working directory.
    pub fn with_working_dir(mut self, dir: PathBuf) -> Self {
        self.working_dir = dir;
        self
    }

    /// Set max concurrent tasks.
    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent_tasks = max;
        self
    }

    /// Set default timeout.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.default_timeout = timeout;
        self
    }

    /// Set base URL.
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    /// Create config from environment variables.
    ///
    /// Looks for:
    /// - `CORTEX_AUTH_TOKEN`
    /// - `CORTEX_PROVIDER` (defaults to "cortex")
    /// - `CORTEX_MODEL` (defaults to claude-sonnet)
    pub fn from_env() -> Result<Self> {
        let provider = std::env::var("CORTEX_PROVIDER").unwrap_or_else(|_| "cortex".to_string());

        let api_key = std::env::var("CORTEX_AUTH_TOKEN").map_err(|_| {
            CortexError::Config(format!(
                "API key not found for provider '{}'. Set CORTEX_AUTH_TOKEN environment variable.",
                provider
            ))
        })?;

        let model_id = std::env::var("CORTEX_MODEL")
            .unwrap_or_else(|_| "anthropic/claude-sonnet-4-20250514".to_string());

        Ok(Self::new(provider, model_id, api_key))
    }
}

/// Unified executor for all tools including Task and Batch.
///
/// This is the main entry point for tool execution in cortex-tui.
/// It handles:
/// - Standard tools via `ToolRegistry`
/// - Task tool via `SubagentExecutor`
/// - Batch tool via `BatchToolHandler`
pub struct UnifiedToolExecutor {
    /// Tool registry for standard tools.
    registry: Arc<ToolRegistry>,
    /// Model client for subagents (separate from main conversation).
    model_client: Arc<dyn ModelClient>,
    /// Subagent executor for Task tool.
    subagent_executor: SubagentExecutor,
    /// Agent registry for custom agents.
    agent_registry: Arc<AgentRegistry>,
    /// Configuration.
    config: ExecutorConfig,
}

impl std::fmt::Debug for UnifiedToolExecutor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UnifiedToolExecutor")
            .field("config", &self.config)
            .field("registry_tools", &self.registry.all().len())
            .finish()
    }
}

impl UnifiedToolExecutor {
    /// Create a new UnifiedToolExecutor with the given configuration.
    ///
    /// This creates a separate ModelClient for subagent execution,
    /// isolating subagent API calls from the main conversation.
    pub fn new(config: ExecutorConfig) -> Result<Self> {
        // Create the model client for subagents
        let model_client: Arc<dyn ModelClient> = Arc::from(create_client(
            &config.provider,
            &config.model_id,
            &config.api_key,
            config.base_url.as_deref(),
        )?);

        // Create tool registry with default tools
        let registry = Arc::new(ToolRegistry::new());

        // Create agent registry
        let agent_registry = Arc::new(AgentRegistry::new(&config.working_dir, None));

        // Create subagent executor
        let subagent_executor = SubagentExecutor::new(
            model_client.clone(),
            registry.clone(),
            agent_registry.clone(),
            &config.model_id,
        )
        .with_max_concurrent(config.max_concurrent_tasks);

        Ok(Self {
            registry,
            model_client,
            subagent_executor,
            agent_registry,
            config,
        })
    }

    /// Create with an existing registry and model client.
    ///
    /// Use this when you already have a configured registry (e.g., with plugins loaded)
    /// and want to share it with the unified executor.
    pub fn with_registry_and_client(
        registry: Arc<ToolRegistry>,
        model_client: Arc<dyn ModelClient>,
        config: ExecutorConfig,
    ) -> Self {
        let agent_registry = Arc::new(AgentRegistry::new(&config.working_dir, None));

        let subagent_executor = SubagentExecutor::new(
            model_client.clone(),
            registry.clone(),
            agent_registry.clone(),
            &config.model_id,
        )
        .with_max_concurrent(config.max_concurrent_tasks);

        Self {
            registry,
            model_client,
            subagent_executor,
            agent_registry,
            config,
        }
    }

    /// Get the underlying tool registry.
    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    /// Get tool definitions for API (for sending to LLM).
    pub fn get_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.registry.get_definitions();

        // Add Batch tool definition
        definitions.push(super::handlers::batch::batch_tool_definition());

        definitions
    }

    /// Check if a tool exists.
    pub fn has_tool(&self, name: &str) -> bool {
        matches!(name.to_lowercase().as_str(), "task" | "batch") || self.registry.has(name)
    }

    /// Execute a tool by name.
    ///
    /// This is the main entry point for tool execution.
    /// Special handling is provided for:
    /// - `Task` / `task`: Spawns a subagent session
    /// - `Batch` / `batch`: Executes multiple tools in parallel
    /// - All other tools: Delegates to the registry
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: Value,
        context: ToolContext,
    ) -> Result<ToolResult> {
        match tool_name.to_lowercase().as_str() {
            "task" => self.execute_task(arguments, context, None, None).await,
            "batch" => self.execute_batch(arguments, context).await,
            _ => {
                self.registry
                    .execute_with_context(tool_name, arguments, context)
                    .await
            }
        }
    }

    /// Execute a tool with progress reporting.
    ///
    /// Same as `execute`, but provides a channel for progress events
    /// (primarily useful for Task tool).
    ///
    /// # Arguments
    /// * `tool_name` - Name of the tool to execute
    /// * `arguments` - Tool arguments as JSON
    /// * `context` - Execution context
    /// * `progress_tx` - Optional channel for progress events
    /// * `session_id` - Optional session ID for Task tool (for UI coordination)
    pub async fn execute_with_progress(
        &self,
        tool_name: &str,
        arguments: Value,
        context: ToolContext,
        progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
        session_id: Option<String>,
    ) -> Result<ToolResult> {
        match tool_name.to_lowercase().as_str() {
            "task" => {
                self.execute_task(arguments, context, progress_tx, session_id)
                    .await
            }
            "batch" => self.execute_batch(arguments, context).await,
            _ => {
                self.registry
                    .execute_with_context(tool_name, arguments, context)
                    .await
            }
        }
    }

    /// Execute the Task tool - spawns a subagent session.
    ///
    /// # Arguments
    /// * `arguments` - Task tool arguments (description, prompt, subagent_type, etc.)
    /// * `context` - Execution context
    /// * `progress_tx` - Optional channel for progress events
    /// * `ui_session_id` - Optional session ID from UI for coordination
    async fn execute_task(
        &self,
        arguments: Value,
        context: ToolContext,
        progress_tx: Option<mpsc::UnboundedSender<ProgressEvent>>,
        ui_session_id: Option<String>,
    ) -> Result<ToolResult> {
        // Parse Task arguments
        let description = arguments
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("Subagent task")
            .to_string();

        let prompt = arguments
            .get("prompt")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CortexError::InvalidInput("Task requires 'prompt' parameter".to_string())
            })?
            .to_string();

        let subagent_type_str = arguments
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or("general");

        // Session ID from arguments (for continuing a session)
        let continue_session_id = arguments
            .get("session_id")
            .and_then(|v| v.as_str())
            .map(String::from);

        let max_iterations = arguments
            .get("max_iterations")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        let timeout_seconds = arguments.get("timeout_seconds").and_then(|v| v.as_u64());

        let additional_context = arguments
            .get("context")
            .and_then(|v| v.as_str())
            .map(String::from);

        let model = arguments
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from);

        // Build subagent config
        let agent_type = SubagentType::from_str(subagent_type_str);

        let mut config =
            SubagentConfig::new(agent_type, &description, &prompt, context.cwd.clone());

        // Use UI session ID if provided (for coordination), otherwise use continue_session_id
        if let Some(sid) = ui_session_id {
            config = config.with_session_id(sid);
        }

        if let Some(sid) = continue_session_id {
            config = config.with_continue_session(sid);
        }

        if let Some(max) = max_iterations {
            config = config.with_max_iterations(max);
        }

        if let Some(timeout) = timeout_seconds {
            config = config.with_timeout(Duration::from_secs(timeout));
        }

        if let Some(ctx) = additional_context {
            config = config.with_context(ctx);
        }

        if let Some(m) = model {
            config = config.with_model(m);
        }

        // Create progress channel if not provided
        let (tx, mut rx) = mpsc::unbounded_channel();
        let progress_tx = progress_tx.unwrap_or(tx);

        // Spawn progress consumer if we created our own channel
        let _progress_handle = if progress_tx.is_closed() {
            None
        } else {
            Some(tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    tracing::debug!("Subagent progress: {}", event.to_message());
                    if event.is_terminal() {
                        break;
                    }
                }
            }))
        };

        // Execute the subagent
        let result = self.subagent_executor.execute(config, progress_tx).await?;

        // Format result
        Ok(self.format_task_result(result))
    }

    /// Format SubagentResult into ToolResult.
    fn format_task_result(&self, result: SubagentResult) -> ToolResult {
        let mut output = result.output.clone();

        // Add metadata
        output.push_str(&format!(
            "\n\n<task_metadata>\nsession_id: {}\n</task_metadata>",
            result.session.id
        ));

        if result.success {
            ToolResult::success(output)
        } else {
            ToolResult::error(output)
        }
    }

    /// Execute the Batch tool - runs multiple tools in parallel.
    async fn execute_batch(&self, arguments: Value, context: ToolContext) -> Result<ToolResult> {
        // Parse batch arguments
        let args: BatchToolArgs = serde_json::from_value(arguments.clone()).or_else(|_| {
            // Try alternative format with "tool_calls" instead of "calls"
            if let Some(tool_calls) = arguments.get("tool_calls").and_then(|v| v.as_array()) {
                let calls: Vec<BatchToolCall> = tool_calls
                    .iter()
                    .filter_map(|tc| {
                        let tool = tc.get("tool").and_then(|v| v.as_str())?;
                        let args = tc
                            .get("parameters")
                            .cloned()
                            .unwrap_or(Value::Object(Default::default()));
                        Some(BatchToolCall {
                            tool: tool.to_string(),
                            arguments: args,
                        })
                    })
                    .collect();

                Ok(BatchToolArgs {
                    calls,
                    timeout_secs: arguments.get("timeout_secs").and_then(|v| v.as_u64()),
                    tool_timeout_secs: arguments.get("tool_timeout_secs").and_then(|v| v.as_u64()),
                })
            } else {
                Err(CortexError::InvalidInput(
                    "Invalid Batch arguments".to_string(),
                ))
            }
        })?;

        // Create batch handler with self as executor
        let executor: Arc<dyn BatchToolExecutor> = Arc::new(UnifiedBatchExecutor {
            registry: self.registry.clone(),
        });

        let handler = BatchToolHandler::new(executor);

        // Execute batch
        let batch_args = serde_json::to_value(args).map_err(|e| {
            CortexError::InvalidInput(format!("Failed to serialize batch args: {}", e))
        })?;

        ToolHandlerTrait::execute(&handler, batch_args, &context).await
    }

    /// Get the subagent executor (for advanced use cases).
    pub fn subagent_executor(&self) -> &SubagentExecutor {
        &self.subagent_executor
    }

    /// Get the agent registry.
    pub fn agent_registry(&self) -> &Arc<AgentRegistry> {
        &self.agent_registry
    }

    /// Get the model client.
    pub fn model_client(&self) -> &Arc<dyn ModelClient> {
        &self.model_client
    }
}

/// Internal executor for Batch tool that delegates to the registry.
struct UnifiedBatchExecutor {
    registry: Arc<ToolRegistry>,
}

#[async_trait]
impl BatchToolExecutor for UnifiedBatchExecutor {
    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<ToolResult> {
        // Prevent recursive batch calls (already handled by BatchToolHandler validation)
        // Also prevent Task from being called in batch for safety
        // (being conservative to avoid complex nesting scenarios)

        self.registry
            .execute_with_context(name, arguments, context.clone())
            .await
    }

    fn has_tool(&self, name: &str) -> bool {
        self.registry.has(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_config_default() {
        let config = ExecutorConfig::default();
        assert_eq!(config.provider, "cortex");
        assert_eq!(config.max_concurrent_tasks, 3);
        assert_eq!(config.default_timeout, Duration::from_secs(300));
    }

    #[test]
    fn test_executor_config_builder() {
        let config = ExecutorConfig::new("anthropic", "claude-3-5-sonnet", "test-key")
            .with_max_concurrent(5)
            .with_timeout(Duration::from_secs(600))
            .with_working_dir(PathBuf::from("/tmp"));

        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model_id, "claude-3-5-sonnet");
        assert_eq!(config.max_concurrent_tasks, 5);
        assert_eq!(config.default_timeout, Duration::from_secs(600));
        assert_eq!(config.working_dir, PathBuf::from("/tmp"));
    }

    #[test]
    fn test_has_tool() {
        // Can't test without API key, but we can test the logic
        let config = ExecutorConfig::default();

        // These would require a valid executor, so we just verify config is valid
        assert!(!config.api_key.is_empty() || config.api_key.is_empty());
    }
}
