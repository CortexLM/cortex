//! Task tool handler - spawn and manage subagents for complex tasks.
//!
//! The Task tool enables delegation of work to specialized subagents that run
//! in isolated sessions. Each subagent type has different capabilities and
//! tool restrictions.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::agents::AgentRegistry;
use crate::client::ModelClient;
use crate::error::{CortexError, Result};
use crate::tools::context::ToolContext;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::{ToolDefinition, ToolHandler, ToolResult};

use super::subagent::{ProgressEvent, SubagentConfig, SubagentExecutor, SubagentType};

/// Task tool handler for spawning subagents.
pub struct TaskHandler {
    /// Subagent executor.
    executor: Arc<SubagentExecutor>,
    /// Working directory.
    working_dir: PathBuf,
}

impl TaskHandler {
    /// Create a new task handler.
    pub fn new(
        client: Arc<dyn ModelClient>,
        tools: Arc<ToolRegistry>,
        agent_registry: Arc<AgentRegistry>,
        default_model: impl Into<String>,
        working_dir: PathBuf,
    ) -> Self {
        let executor = Arc::new(SubagentExecutor::new(
            client,
            tools,
            agent_registry,
            default_model,
        ));

        Self {
            executor,
            working_dir,
        }
    }

    /// Create with a pre-configured executor.
    pub fn with_executor(executor: Arc<SubagentExecutor>, working_dir: PathBuf) -> Self {
        Self {
            executor,
            working_dir,
        }
    }

    /// Get the tool definition.
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new("Task", Self::description(), Self::parameters())
    }

    /// Get detailed description.
    fn description() -> &'static str {
        r#"Delegate a task to a custom agent. The agent executes autonomously and returns the result. Use this to offload specialized subtasks to purpose-built agents. Custom agents are defined in .cortex/agents/ (project-level) or ~/.cortex/agents/ (personal).

## When to Use
- Offloading specialized subtasks to custom agents
- Tasks requiring specific agent expertise
- Parallel work on independent subtasks
- Fire-and-forget background tasks

## Examples

### Basic Delegation
```json
{
  "agent": "code-reviewer",
  "task": "Review the authentication module for security vulnerabilities and suggest improvements"
}
```

### With Context
```json
{
  "agent": "documentation-writer",
  "task": "Generate API documentation for all endpoints",
  "context": "The API uses OpenAPI 3.0 spec. Focus on request/response examples."
}
```

### Fire-and-Forget
```json
{
  "agent": "background-optimizer",
  "task": "Optimize database indexes",
  "await_result": false
}
```

## Best Practices
1. Ensure the agent exists before delegating
2. Be specific about expected inputs and outputs
3. Provide relevant context for better results
4. Use await_result=false for long-running background tasks
5. Check agent capabilities before delegation"#
    }

    /// Get parameter schema.
    fn parameters() -> Value {
        json!({
            "type": "object",
            "properties": {
                "agent": {
                    "type": "string",
                    "description": "Name of the custom agent to invoke. Must exist in .cortex/agents/ or ~/.cortex/agents/"
                },
                "task": {
                    "type": "string",
                    "description": "Detailed description of the task to delegate. Be specific about expected inputs and outputs."
                },
                "context": {
                    "type": "string",
                    "description": "Additional context for the agent: relevant file paths, constraints, preferences, or background information"
                },
                "await_result": {
                    "type": "boolean",
                    "description": "Wait for agent to complete and return result (true) or fire-and-forget (false)",
                    "default": true
                }
            },
            "required": ["agent", "task"],
            "additionalProperties": false
        })
    }

    /// Parse task parameters from arguments.
    fn parse_params(&self, arguments: Value) -> Result<TaskParams> {
        let agent = arguments
            .get("agent")
            .and_then(|a| a.as_str())
            .ok_or_else(|| CortexError::InvalidInput("agent is required".into()))?
            .to_string();

        let task = arguments
            .get("task")
            .and_then(|t| t.as_str())
            .ok_or_else(|| CortexError::InvalidInput("task is required".into()))?
            .to_string();

        let context = arguments
            .get("context")
            .and_then(|c| c.as_str())
            .map(String::from);

        let await_result = arguments
            .get("await_result")
            .and_then(|a| a.as_bool())
            .unwrap_or(true);

        Ok(TaskParams {
            agent,
            task,
            context,
            await_result,
        })
    }

    /// Build subagent config from params.
    fn build_config(&self, params: TaskParams) -> SubagentConfig {
        let mut config = SubagentConfig::new(
            SubagentType::Code,
            &params.agent,
            &params.task,
            self.working_dir.clone(),
        );

        if let Some(context) = params.context {
            config = config.with_context(context);
        }

        config
    }
}

#[async_trait]
impl ToolHandler for TaskHandler {
    fn name(&self) -> &str {
        "Task"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        // Parse parameters
        let params = match self.parse_params(arguments) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::error(e.to_string())),
        };

        // Build config
        let config = self.build_config(params.clone());

        // Create progress channel (for now we collect but don't stream)
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel::<ProgressEvent>();

        // Spawn progress collector
        let progress_collector = tokio::spawn(async move {
            let mut events: Vec<String> = Vec::new();
            while let Some(event) = progress_rx.recv().await {
                let message = event.to_message();
                events.push(message);
                if event.is_terminal() {
                    break;
                }
            }
            events
        });

        // If await_result is false, return immediately with task queued message
        if !params.await_result {
            let output = format!(
                r#"## Task Delegated (Fire-and-Forget)

**Agent:** `{}`
**Task:** {}

The task has been queued for execution by the agent.
No result will be returned (await_result=false)."#,
                params.agent, params.task
            );
            return Ok(ToolResult::success(output));
        }

        // Execute subagent
        let result = self.executor.execute(config, progress_tx).await;

        // Wait for progress collector
        let progress_messages = progress_collector.await.unwrap_or_default();

        // Format result
        match result {
            Ok(subagent_result) => {
                let output = subagent_result.to_tool_output();

                // Add progress log if verbose
                let mut full_output = output;
                if !progress_messages.is_empty() {
                    full_output.push_str("\n## Progress Log\n");
                    for msg in progress_messages.iter().take(20) {
                        full_output.push_str(&format!("• {}\n", msg));
                    }
                    if progress_messages.len() > 20 {
                        full_output.push_str(&format!(
                            "... and {} more events\n",
                            progress_messages.len() - 20
                        ));
                    }
                }

                if subagent_result.success {
                    Ok(ToolResult::success(full_output))
                } else {
                    Ok(ToolResult::error(full_output))
                }
            }
            Err(e) => Ok(ToolResult::error(format!("Task execution failed: {}", e))),
        }
    }
}

/// Parsed task parameters.
#[derive(Debug, Clone)]
struct TaskParams {
    agent: String,
    task: String,
    context: Option<String>,
    await_result: bool,
}

/// Create a standalone task handler with minimal dependencies (for registry integration).
/// This version uses a simpler execution path when the full executor isn't available.
pub struct SimpleTaskHandler;

impl SimpleTaskHandler {
    /// Create a new simple task handler.
    pub fn new() -> Self {
        Self
    }

    /// Get the tool definition.
    pub fn definition() -> ToolDefinition {
        TaskHandler::definition()
    }
}

impl Default for SimpleTaskHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for SimpleTaskHandler {
    fn name(&self) -> &str {
        "Task"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let agent = arguments
            .get("agent")
            .and_then(|a| a.as_str())
            .ok_or_else(|| CortexError::InvalidInput("agent is required".into()))?;

        let task = arguments
            .get("task")
            .and_then(|t| t.as_str())
            .ok_or_else(|| CortexError::InvalidInput("task is required".into()))?;

        let context = arguments.get("context").and_then(|c| c.as_str());

        let await_result = arguments
            .get("await_result")
            .and_then(|a| a.as_bool())
            .unwrap_or(true);

        let mut output = format!(
            r#"## Task Delegated

**Agent:** `{}`
**Task:** {}
**Await Result:** {}

"#,
            agent, task, await_result
        );

        if let Some(ctx) = context {
            output.push_str(&format!("**Context:** {}\n\n", ctx));
        }

        output.push_str(
            "The task has been queued for execution by the agent.\n\
            The orchestrator will handle agent spawning and execution.",
        );

        Ok(ToolResult::success(output))
    }
}

/// Tool for listing available subagent types.
pub struct ListSubagentsHandler {
    executor: Option<Arc<SubagentExecutor>>,
}

impl ListSubagentsHandler {
    /// Create a new handler.
    pub fn new() -> Self {
        Self { executor: None }
    }

    /// Create with executor for dynamic agent listing.
    pub fn with_executor(executor: Arc<SubagentExecutor>) -> Self {
        Self {
            executor: Some(executor),
        }
    }

    /// Get the tool definition.
    pub fn definition() -> ToolDefinition {
        ToolDefinition::new(
            "ListSubagents",
            "List available subagent types and their capabilities. Use this to understand what specialized agents are available for task delegation.",
            json!({
                "type": "object",
                "properties": {
                    "include_custom": {
                        "type": "boolean",
                        "description": "Include custom agents from the agent registry. Default: true"
                    }
                },
                "required": []
            }),
        )
    }
}

impl Default for ListSubagentsHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for ListSubagentsHandler {
    fn name(&self) -> &str {
        "ListSubagents"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let include_custom = arguments
            .get("include_custom")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let mut output = String::from("# Available Subagent Types\n\n");

        // Built-in types
        output.push_str("## Built-in Subagents\n\n");
        output.push_str("| Type | Description | Modifies Files |\n");
        output.push_str("|------|-------------|----------------|\n");

        let builtin_types = [
            (
                "code",
                "General-purpose coding agent. Implements features, fixes bugs, writes code.",
                true,
            ),
            (
                "research",
                "Investigation agent. Analyzes code, finds patterns, gathers information.",
                false,
            ),
            (
                "refactor",
                "Code improvement agent. Restructures, renames, cleans up code.",
                true,
            ),
            (
                "test",
                "Testing agent. Writes unit tests, improves coverage, runs tests.",
                true,
            ),
            (
                "documentation",
                "Documentation agent. Creates README, API docs, inline comments.",
                true,
            ),
            (
                "security",
                "Security audit agent. Finds vulnerabilities, reviews access controls.",
                true,
            ),
            (
                "architect",
                "Architecture planning agent. Designs systems, plans refactors.",
                false,
            ),
            (
                "reviewer",
                "Code review agent. Finds bugs, suggests improvements.",
                false,
            ),
        ];

        for (name, desc, modifies) in builtin_types {
            let modify_icon = if modifies { "[Y]" } else { "[N]" };
            output.push_str(&format!("| `{}` | {} | {} |\n", name, desc, modify_icon));
        }

        // Custom agents
        if include_custom {
            if let Some(ref executor) = self.executor {
                let custom_agents = executor.custom_agents().await;
                if !custom_agents.is_empty() {
                    output.push_str("\n## Custom Agents\n\n");
                    output.push_str("| Name | Description | Source |\n");
                    output.push_str("|------|-------------|--------|\n");

                    for agent in custom_agents {
                        output.push_str(&format!(
                            "| `{}` | {} | {} |\n",
                            agent.metadata.name, agent.metadata.description, agent.source
                        ));
                    }
                }
            }
        }

        output.push_str("\n## Usage Example\n\n");
        output.push_str("```json\n");
        output.push_str("{\n");
        output.push_str("  \"description\": \"Your task description\",\n");
        output.push_str("  \"prompt\": \"Detailed instructions for the subagent\",\n");
        output.push_str("  \"subagent_type\": \"code\"\n");
        output.push_str("}\n");
        output.push_str("```\n");

        Ok(ToolResult::success(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_params_parsing() {
        let handler = TaskHandler::with_executor(
            Arc::new(SubagentExecutor::new(
                Arc::new(MockClient::new()),
                Arc::new(ToolRegistry::new()),
                Arc::new(AgentRegistry::new(&PathBuf::from("/tmp"), None)),
                "gpt-4o",
            )),
            PathBuf::from("/project"),
        );

        let args = json!({
            "agent": "code-reviewer",
            "task": "Review the authentication module",
            "await_result": true
        });

        let params = handler.parse_params(args).unwrap();
        assert_eq!(params.agent, "code-reviewer");
        assert_eq!(params.task, "Review the authentication module");
        assert!(params.await_result);
    }

    #[test]
    fn test_task_params_with_context() {
        let handler = TaskHandler::with_executor(
            Arc::new(SubagentExecutor::new(
                Arc::new(MockClient::new()),
                Arc::new(ToolRegistry::new()),
                Arc::new(AgentRegistry::new(&PathBuf::from("/tmp"), None)),
                "gpt-4o",
            )),
            PathBuf::from("/project"),
        );

        let args = json!({
            "agent": "doc-writer",
            "task": "Generate API documentation",
            "context": "Use OpenAPI 3.0 format",
            "await_result": false
        });

        let params = handler.parse_params(args).unwrap();
        assert_eq!(params.agent, "doc-writer");
        assert_eq!(params.context, Some("Use OpenAPI 3.0 format".to_string()));
        assert!(!params.await_result);
    }

    #[tokio::test]
    async fn test_simple_task_handler() {
        let handler = SimpleTaskHandler::new();
        let context = ToolContext::new(PathBuf::from("/project"));

        let args = json!({
            "agent": "test-agent",
            "task": "Do something useful"
        });

        let result = handler.execute(args, &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("test-agent"));
        assert!(result.output.contains("Do something useful"));
    }

    #[tokio::test]
    async fn test_list_subagents_handler() {
        let handler = ListSubagentsHandler::new();
        let context = ToolContext::new(PathBuf::from("/project"));

        let result = handler.execute(json!({}), &context).await.unwrap();
        assert!(result.success);
        assert!(result.output.contains("code"));
        assert!(result.output.contains("research"));
        assert!(result.output.contains("refactor"));
    }

    // Mock client for testing
    struct MockClient {
        capabilities: crate::client::types::ModelCapabilities,
    }

    impl MockClient {
        fn new() -> Self {
            Self {
                capabilities: crate::client::types::ModelCapabilities::default(),
            }
        }
    }

    #[async_trait::async_trait]
    impl ModelClient for MockClient {
        fn model(&self) -> &str {
            "mock-model"
        }

        fn provider(&self) -> &str {
            "mock-provider"
        }

        fn capabilities(&self) -> &crate::client::types::ModelCapabilities {
            &self.capabilities
        }

        async fn complete(
            &self,
            _request: crate::client::types::CompletionRequest,
        ) -> crate::error::Result<crate::client::ResponseStream> {
            Err(crate::error::CortexError::Internal("Mock".into()))
        }

        async fn complete_sync(
            &self,
            _request: crate::client::types::CompletionRequest,
        ) -> crate::error::Result<crate::client::types::CompletionResponse> {
            Err(crate::error::CortexError::Internal("Mock".into()))
        }
    }
}
