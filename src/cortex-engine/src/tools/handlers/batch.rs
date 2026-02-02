//! Batch tool - Execute multiple tools in parallel.
//!
//! This tool allows executing up to 10 tools simultaneously for improved performance.
//! It provides isolated execution where errors in one tool don't affect others.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::error::{CortexError, Result};
use crate::tools::context::ToolContext;
use crate::tools::spec::{ToolDefinition, ToolHandler, ToolResult};

/// Maximum number of tools that can be executed in a batch.
pub const MAX_BATCH_SIZE: usize = 10;

/// Default timeout for batch execution in seconds.
pub const DEFAULT_BATCH_TIMEOUT_SECS: u64 = 300;

/// Tools that cannot be called within a batch (prevent recursion).
/// Note: Task is now allowed to enable parallel task execution.
pub const DISALLOWED_TOOLS: &[&str] = &["Batch", "batch", "Agent", "agent"];

/// A single tool call within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchToolCall {
    /// Name of the tool to execute.
    pub tool: String,
    /// Arguments/parameters for the tool as JSON.
    #[serde(alias = "parameters")]
    pub arguments: Value,
}

/// Arguments for the Batch tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchToolArgs {
    /// Array of tool calls to execute in parallel (max 10).
    pub calls: Vec<BatchToolCall>,
    /// Optional timeout in seconds for the entire batch (default: 300s).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// Result of a single tool call within the batch.
#[derive(Debug, Clone, Serialize)]
pub struct BatchCallResult {
    /// Name of the tool that was called.
    pub tool: String,
    /// Index of this call in the original batch.
    pub index: usize,
    /// Whether the execution was successful.
    pub success: bool,
    /// The result output if successful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message if failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Aggregated result of all batch executions.
#[derive(Debug, Clone, Serialize)]
pub struct BatchResult {
    /// Individual results for each tool call.
    pub results: Vec<BatchCallResult>,
    /// Number of successful executions.
    pub success_count: usize,
    /// Number of failed executions.
    pub error_count: usize,
    /// Total execution time in milliseconds.
    pub total_duration_ms: u64,
}

/// Trait for executing individual tools within a batch.
/// This allows the batch handler to delegate tool execution to the router.
#[async_trait]
pub trait BatchToolExecutor: Send + Sync {
    /// Execute a single tool with the given name, arguments, and context.
    async fn execute_tool(
        &self,
        name: &str,
        arguments: Value,
        context: &ToolContext,
    ) -> Result<ToolResult>;

    /// Check if a tool exists.
    fn has_tool(&self, name: &str) -> bool;
}

/// Handler for the Batch tool.
///
/// The BatchToolHandler executes multiple tools in parallel, providing:
/// - Up to 10 concurrent tool executions
/// - Isolated error handling (one failure doesn't affect others)
/// - Configurable timeout
/// - Protection against recursive batch calls
pub struct BatchToolHandler {
    /// The executor used to run individual tools.
    executor: Arc<dyn BatchToolExecutor>,
}

impl BatchToolHandler {
    /// Create a new BatchToolHandler with the given executor.
    pub fn new(executor: Arc<dyn BatchToolExecutor>) -> Self {
        Self { executor }
    }

    /// Validate the batch request.
    fn validate_calls(&self, calls: &[BatchToolCall]) -> Result<()> {
        // Check batch size
        if calls.is_empty() {
            return Err(CortexError::InvalidInput(
                "Batch must contain at least one tool call".to_string(),
            ));
        }

        if calls.len() > MAX_BATCH_SIZE {
            return Err(CortexError::InvalidInput(format!(
                "Batch contains {} calls, but maximum is {}",
                calls.len(),
                MAX_BATCH_SIZE
            )));
        }

        // Check for disallowed tools (prevent recursion)
        for (idx, call) in calls.iter().enumerate() {
            if DISALLOWED_TOOLS
                .iter()
                .any(|&t| t.eq_ignore_ascii_case(&call.tool))
            {
                return Err(CortexError::InvalidInput(format!(
                    "Tool '{}' at index {} cannot be called within a batch. Recursive or heavy tools are not allowed: {:?}",
                    call.tool, idx, DISALLOWED_TOOLS
                )));
            }

            // Check if tool exists
            if !self.executor.has_tool(&call.tool) {
                return Err(CortexError::UnknownTool {
                    name: call.tool.clone(),
                });
            }
        }

        Ok(())
    }

    /// Execute all tool calls in parallel.
    async fn execute_parallel(
        &self,
        calls: Vec<BatchToolCall>,
        context: &ToolContext,
        timeout_duration: Duration,
    ) -> BatchResult {
        let start_time = Instant::now();
        let results = Arc::new(Mutex::new(Vec::with_capacity(calls.len())));

        // Create futures for all tool calls
        let futures: Vec<_> = calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| {
                let executor = Arc::clone(&self.executor);
                let ctx = context.clone();
                let results = Arc::clone(&results);
                let tool_name = call.tool.clone();

                async move {
                    let call_start = Instant::now();

                    // Execute with per-call timeout (use batch timeout for each call)
                    let execution_result = timeout(
                        timeout_duration,
                        executor.execute_tool(&call.tool, call.arguments, &ctx),
                    )
                    .await;

                    let duration_ms = call_start.elapsed().as_millis() as u64;

                    let call_result = match execution_result {
                        Ok(Ok(tool_result)) => BatchCallResult {
                            tool: tool_name,
                            index,
                            success: tool_result.success,
                            result: Some(json!({
                                "output": tool_result.output,
                                "success": tool_result.success,
                            })),
                            error: if tool_result.success {
                                None
                            } else {
                                Some(tool_result.output)
                            },
                            duration_ms,
                        },
                        Ok(Err(e)) => BatchCallResult {
                            tool: tool_name,
                            index,
                            success: false,
                            result: None,
                            error: Some(format!("Execution error: {}", e)),
                            duration_ms,
                        },
                        Err(_) => BatchCallResult {
                            tool: tool_name,
                            index,
                            success: false,
                            result: None,
                            error: Some(format!("Timeout after {}s", timeout_duration.as_secs())),
                            duration_ms,
                        },
                    };

                    results.lock().await.push(call_result);
                }
            })
            .collect();

        // Execute all futures concurrently
        join_all(futures).await;

        // Get results and sort by original index
        let mut final_results = results.lock().await.clone();
        final_results.sort_by_key(|r| r.index);

        let success_count = final_results.iter().filter(|r| r.success).count();
        let error_count = final_results.len() - success_count;
        let total_duration_ms = start_time.elapsed().as_millis() as u64;

        BatchResult {
            results: final_results,
            success_count,
            error_count,
            total_duration_ms,
        }
    }

    /// Format the batch result as a human-readable string.
    fn format_result(&self, batch_result: &BatchResult) -> String {
        let mut output = String::new();

        // Summary header
        if batch_result.error_count == 0 {
            output.push_str(&format!(
                "All {} tools executed successfully in {}ms.\n",
                batch_result.success_count, batch_result.total_duration_ms
            ));
            output.push_str("Batch execution complete - all operations succeeded.\n\n");
        } else {
            output.push_str(&format!(
                "Executed {}/{} tools successfully ({} failed) in {}ms.\n\n",
                batch_result.success_count,
                batch_result.results.len(),
                batch_result.error_count,
                batch_result.total_duration_ms
            ));
        }

        // Individual results
        output.push_str("Results:\n");
        output.push_str(&"-".repeat(60));
        output.push('\n');

        for result in &batch_result.results {
            if result.success {
                output.push_str(&format!(
                    "[{}] {} ({}ms)\n",
                    result.index + 1,
                    result.tool,
                    result.duration_ms
                ));

                // Show preview of output
                if let Some(ref res) = result.result {
                    if let Some(out) = res.get("output").and_then(|o| o.as_str()) {
                        let preview: String = out.chars().take(300).collect();
                        let truncated = out.len() > 300;
                        output.push_str(&format!("    Output: {}", preview));
                        if truncated {
                            output.push_str("...[truncated]");
                        }
                        output.push('\n');
                    }
                }
            } else {
                output.push_str(&format!(
                    "[{}] {} - FAILED ({}ms)\n",
                    result.index + 1,
                    result.tool,
                    result.duration_ms
                ));
                if let Some(ref err) = result.error {
                    output.push_str(&format!("    Error: {}\n", err));
                }
            }
        }

        output.push_str(&"-".repeat(60));
        output.push('\n');
        output.push_str(&format!(
            "Summary: {} succeeded, {} failed, {}ms total\n",
            batch_result.success_count, batch_result.error_count, batch_result.total_duration_ms
        ));

        output
    }
}

#[async_trait]
impl ToolHandler for BatchToolHandler {
    fn name(&self) -> &str {
        "Batch"
    }

    async fn execute(&self, arguments: Value, context: &ToolContext) -> Result<ToolResult> {
        // Parse arguments
        let args: BatchToolArgs = serde_json::from_value(arguments)
            .map_err(|e| CortexError::InvalidInput(format!("Invalid Batch arguments: {}", e)))?;

        // Validate calls
        self.validate_calls(&args.calls)?;

        // Determine timeout
        let timeout_secs = args.timeout_secs.unwrap_or(DEFAULT_BATCH_TIMEOUT_SECS);
        let timeout_duration = Duration::from_secs(timeout_secs);

        // Execute all tools in parallel
        let batch_result = self
            .execute_parallel(args.calls, context, timeout_duration)
            .await;

        // Format output
        let output = self.format_result(&batch_result);

        // Determine overall success (at least one succeeded, or all were attempted)
        if batch_result.error_count == batch_result.results.len() {
            // All failed
            Ok(ToolResult::error(output))
        } else {
            Ok(ToolResult::success(output))
        }
    }
}

/// Get the Batch tool definition.
pub fn batch_tool_definition() -> ToolDefinition {
    ToolDefinition::new(
        "Batch",
        "Execute multiple tools in parallel for improved performance. Use this when you need to perform several independent operations simultaneously. Maximum 10 tools per batch. Each tool runs concurrently, significantly reducing total execution time compared to sequential calls. Task tools are allowed for parallel task execution. Cannot call Batch recursively or Agent tools.",
        json!({
            "type": "object",
            "required": ["calls"],
            "properties": {
                "calls": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 10,
                    "description": "Array of tool calls to execute in parallel (max 10). Each call specifies a tool name and its arguments.",
                    "items": {
                        "type": "object",
                        "required": ["tool", "arguments"],
                        "properties": {
                            "tool": {
                                "type": "string",
                                "description": "The name of the tool to execute (e.g., 'Read', 'Grep', 'Glob')"
                            },
                            "arguments": {
                                "type": "object",
                                "description": "Arguments/parameters to pass to the tool"
                            }
                        }
                    }
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "Optional timeout in seconds for the entire batch execution (default: 300)",
                    "minimum": 1,
                    "maximum": 600
                }
            }
        }),
    )
}

/// Execute batch tool - standalone function for use with registry.
pub async fn execute_batch(
    params: BatchParams,
    context: &ToolContext,
    tool_executor: Arc<dyn BatchToolExecutor>,
) -> Result<ToolResult> {
    let handler = BatchToolHandler::new(tool_executor);

    // Convert BatchParams to BatchToolArgs
    let args = BatchToolArgs {
        calls: params
            .tool_calls
            .into_iter()
            .map(|tc| BatchToolCall {
                tool: tc.tool,
                arguments: tc.parameters,
            })
            .collect(),
        timeout_secs: None,
    };

    let arguments = serde_json::to_value(args)
        .map_err(|e| CortexError::InvalidInput(format!("Failed to serialize batch args: {}", e)))?;

    handler.execute(arguments, context).await
}

/// Legacy parameters structure for backwards compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchParams {
    /// Array of tool calls to execute in parallel.
    pub tool_calls: Vec<LegacyBatchToolCall>,
}

/// Legacy tool call structure for backwards compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyBatchToolCall {
    /// Name of the tool to execute.
    pub tool: String,
    /// Parameters for the tool.
    pub parameters: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct MockExecutor {
        available_tools: Vec<String>,
    }

    #[async_trait]
    impl BatchToolExecutor for MockExecutor {
        async fn execute_tool(
            &self,
            name: &str,
            _arguments: Value,
            _context: &ToolContext,
        ) -> Result<ToolResult> {
            // Simulate some async work
            tokio::time::sleep(Duration::from_millis(10)).await;

            if name == "FailingTool" {
                Ok(ToolResult::error("Simulated failure"))
            } else {
                Ok(ToolResult::success(format!("Executed {}", name)))
            }
        }

        fn has_tool(&self, name: &str) -> bool {
            self.available_tools.contains(&name.to_string())
        }
    }

    fn create_mock_executor() -> Arc<dyn BatchToolExecutor> {
        Arc::new(MockExecutor {
            available_tools: vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
                "FailingTool".to_string(),
            ],
        })
    }

    #[test]
    fn test_batch_tool_definition() {
        let def = batch_tool_definition();
        assert_eq!(def.name, "Batch");
        assert!(def.description.contains("parallel"));
        assert!(def.description.contains("10"));
    }

    #[test]
    fn test_disallowed_tools() {
        assert!(DISALLOWED_TOOLS.contains(&"Batch"));
        assert!(DISALLOWED_TOOLS.contains(&"batch"));
        assert!(DISALLOWED_TOOLS.contains(&"Agent"));
        // Task is now allowed in batch for parallel task execution
        assert!(!DISALLOWED_TOOLS.contains(&"Task"));
        assert!(!DISALLOWED_TOOLS.contains(&"Read"));
    }

    #[tokio::test]
    async fn test_validate_empty_calls() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);

        let result = handler.validate_calls(&[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one"));
    }

    #[tokio::test]
    async fn test_validate_too_many_calls() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);

        let calls: Vec<BatchToolCall> = (0..15)
            .map(|i| BatchToolCall {
                tool: "Read".to_string(),
                arguments: json!({"path": format!("file{}.txt", i)}),
            })
            .collect();

        let result = handler.validate_calls(&calls);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum is 10"));
    }

    #[tokio::test]
    async fn test_validate_recursive_batch() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);

        let calls = vec![BatchToolCall {
            tool: "Batch".to_string(),
            arguments: json!({}),
        }];

        let result = handler.validate_calls(&calls);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot be called within a batch")
        );
    }

    #[tokio::test]
    async fn test_validate_unknown_tool() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);

        let calls = vec![BatchToolCall {
            tool: "UnknownTool".to_string(),
            arguments: json!({}),
        }];

        let result = handler.validate_calls(&calls);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_successful_batch_execution() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "calls": [
                {"tool": "Read", "arguments": {"path": "file1.txt"}},
                {"tool": "Grep", "arguments": {"pattern": "test"}},
            ]
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(tool_result.success);
        assert!(tool_result.output.contains("2 tools executed successfully"));
    }

    #[tokio::test]
    async fn test_partial_failure_batch() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "calls": [
                {"tool": "Read", "arguments": {"path": "file1.txt"}},
                {"tool": "FailingTool", "arguments": {}},
            ]
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(tool_result.success); // Partial success still counts as success
        assert!(tool_result.output.contains("1/2"));
        assert!(tool_result.output.contains("1 failed"));
    }

    #[tokio::test]
    async fn test_all_failed_batch() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);
        let context = ToolContext::new(PathBuf::from("."));

        let args = json!({
            "calls": [
                {"tool": "FailingTool", "arguments": {}},
                {"tool": "FailingTool", "arguments": {}},
            ]
        });

        let result = handler.execute(args, &context).await;
        assert!(result.is_ok());

        let tool_result = result.unwrap();
        assert!(!tool_result.success); // All failed = overall failure
    }

    #[tokio::test]
    async fn test_parallel_execution_performance() {
        let executor = create_mock_executor();
        let handler = BatchToolHandler::new(executor);
        let context = ToolContext::new(PathBuf::from("."));

        // Each tool takes ~10ms, so 5 tools should complete in ~10-20ms if parallel
        // vs ~50ms if sequential
        let args = json!({
            "calls": [
                {"tool": "Read", "arguments": {}},
                {"tool": "Grep", "arguments": {}},
                {"tool": "Glob", "arguments": {}},
                {"tool": "Read", "arguments": {}},
                {"tool": "Grep", "arguments": {}},
            ]
        });

        let start = Instant::now();
        let result = handler.execute(args, &context).await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        // Should complete much faster than 50ms if truly parallel
        assert!(
            elapsed.as_millis() < 100,
            "Execution took {}ms, expected < 100ms",
            elapsed.as_millis()
        );
    }
}
