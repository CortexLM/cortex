//! Tool executor for the agent.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tokio::time::timeout;
use tracing::{debug, error};

use crate::error::Result;
use crate::tools::registry::ToolRegistry;
use crate::tools::spec::{ToolCall, ToolResult};

use super::{RiskLevel, SandboxPolicy};

/// Tool executor configuration.
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum concurrent tool executions.
    pub max_concurrent: usize,
    /// Default timeout for tool execution.
    pub default_timeout: Duration,
    /// Sandbox policy.
    pub sandbox_policy: SandboxPolicy,
    /// Enable caching of tool results.
    pub cache_enabled: bool,
    /// Cache TTL.
    pub cache_ttl: Duration,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            default_timeout: Duration::from_secs(120),
            sandbox_policy: SandboxPolicy::Prompt,
            cache_enabled: true,
            cache_ttl: Duration::from_secs(300),
        }
    }
}

/// Tool executor.
#[derive(Debug)]
pub struct ToolExecutor {
    /// Tool registry.
    registry: Arc<ToolRegistry>,
    /// Configuration.
    config: ExecutorConfig,
    /// Result cache.
    cache: RwLock<HashMap<String, CachedResult>>,
    /// Execution statistics.
    stats: RwLock<ExecutorStats>,
}

impl ToolExecutor {
    /// Create a new tool executor.
    pub fn new(registry: Arc<ToolRegistry>, config: ExecutorConfig) -> Self {
        Self {
            registry,
            config,
            cache: RwLock::new(HashMap::new()),
            stats: RwLock::new(ExecutorStats::default()),
        }
    }

    /// Execute a tool call.
    pub async fn execute(&self, call: &ToolCall) -> Result<ToolResult> {
        let start = Instant::now();
        let tool_name = &call.name;

        debug!(tool = tool_name, "Executing tool");

        // Check cache
        if self.config.cache_enabled {
            let cache_key = self.cache_key(call);
            if let Some(result) = self.get_cached(&cache_key).await {
                debug!(tool = tool_name, "Cache hit");
                self.record_execution(tool_name, start.elapsed(), true, true)
                    .await;
                return Ok(result);
            }
        }

        // Execute with timeout
        let result = match timeout(
            self.config.default_timeout,
            self.registry.execute(tool_name, call.arguments.clone()),
        )
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => {
                error!(tool = tool_name, error = %e, "Tool execution failed");
                self.record_execution(tool_name, start.elapsed(), false, false)
                    .await;
                return Err(e);
            }
            Err(_) => {
                error!(tool = tool_name, "Tool execution timed out");
                self.record_execution(tool_name, start.elapsed(), false, false)
                    .await;
                return Ok(ToolResult::error("Execution timed out"));
            }
        };

        let duration = start.elapsed();
        self.record_execution(tool_name, duration, result.success, false)
            .await;

        // Cache successful results
        if self.config.cache_enabled && result.success {
            let cache_key = self.cache_key(call);
            self.cache_result(&cache_key, &result).await;
        }

        Ok(result)
    }

    /// Assess risk level of a tool call.
    pub fn assess_risk(&self, call: &ToolCall) -> RiskLevel {
        // Check for dangerous patterns
        if let Some(cmd) = call.arguments.get("command").and_then(|c| c.as_str()) {
            return assess_command_risk(cmd);
        }

        // Default risk by tool category
        match call.name.as_str() {
            // Read-only operations
            "read_file" | "list_dir" | "search" | "grep" | "glob" | "LspHover"
            | "LspDiagnostics" | "LspSymbols" => RiskLevel::Safe,

            // File modifications
            "write_file" | "create_file" | "edit_file" => RiskLevel::Medium,

            // Destructive operations
            "delete_file" | "remove_dir" | "rm" => RiskLevel::High,

            // Shell execution
            "shell" | "execute" | "run" => RiskLevel::High,

            // Network operations
            "fetch_url" | "web_search" => RiskLevel::Low,

            // Container operations
            "docker_run" | "container_exec" => RiskLevel::High,

            // Git operations
            "git_commit" | "git_push" => RiskLevel::High,
            "git_status" | "git_log" | "git_diff" => RiskLevel::Safe,

            // Default
            _ => RiskLevel::Medium,
        }
    }

    /// Check if tool call can be auto-approved.
    pub fn can_auto_approve(&self, call: &ToolCall) -> bool {
        let risk = self.assess_risk(call);
        risk.can_auto_approve(self.config.sandbox_policy)
    }

    /// Get execution statistics.
    pub async fn stats(&self) -> ExecutorStats {
        self.stats.read().await.clone()
    }

    // Internal helpers

    fn cache_key(&self, call: &ToolCall) -> String {
        format!(
            "{}:{}",
            call.name,
            serde_json::to_string(&call.arguments).unwrap_or_default()
        )
    }

    async fn get_cached(&self, key: &str) -> Option<ToolResult> {
        let cache = self.cache.read().await;
        cache.get(key).and_then(|cached| {
            if cached.is_valid(self.config.cache_ttl) {
                Some(cached.result.clone())
            } else {
                None
            }
        })
    }

    async fn cache_result(&self, key: &str, result: &ToolResult) {
        let mut cache = self.cache.write().await;
        cache.insert(key.to_string(), CachedResult::new(result.clone()));

        // Evict old entries if cache is too large
        if cache.len() > 1000 {
            let cutoff = Instant::now() - self.config.cache_ttl;
            cache.retain(|_, v| v.cached_at > cutoff);
        }
    }

    async fn record_execution(&self, tool: &str, duration: Duration, success: bool, cached: bool) {
        let mut stats = self.stats.write().await;
        stats.total_executions += 1;
        if success {
            stats.successful_executions += 1;
        } else {
            stats.failed_executions += 1;
        }
        if cached {
            stats.cache_hits += 1;
        }
        stats.total_duration_ms += duration.as_millis() as u64;

        let tool_stats = stats.by_tool.entry(tool.to_string()).or_default();
        tool_stats.executions += 1;
        if success {
            tool_stats.successes += 1;
        }
        tool_stats.total_ms += duration.as_millis() as u64;
    }
}

/// Cached tool result.
#[derive(Debug, Clone)]
struct CachedResult {
    result: ToolResult,
    cached_at: Instant,
}

impl CachedResult {
    fn new(result: ToolResult) -> Self {
        Self {
            result,
            cached_at: Instant::now(),
        }
    }

    fn is_valid(&self, ttl: Duration) -> bool {
        self.cached_at.elapsed() < ttl
    }
}

/// Executor statistics.
#[derive(Debug, Clone, Default)]
pub struct ExecutorStats {
    /// Total executions.
    pub total_executions: u64,
    /// Successful executions.
    pub successful_executions: u64,
    /// Failed executions.
    pub failed_executions: u64,
    /// Cache hits.
    pub cache_hits: u64,
    /// Total execution time in ms.
    pub total_duration_ms: u64,
    /// Per-tool statistics.
    pub by_tool: HashMap<String, ToolStats>,
}

impl ExecutorStats {
    /// Get success rate.
    pub fn success_rate(&self) -> f64 {
        if self.total_executions == 0 {
            return 0.0;
        }
        self.successful_executions as f64 / self.total_executions as f64
    }

    /// Get cache hit rate.
    pub fn cache_hit_rate(&self) -> f64 {
        if self.total_executions == 0 {
            return 0.0;
        }
        self.cache_hits as f64 / self.total_executions as f64
    }

    /// Get average execution time.
    pub fn avg_duration_ms(&self) -> f64 {
        if self.total_executions == 0 {
            return 0.0;
        }
        self.total_duration_ms as f64 / self.total_executions as f64
    }
}

/// Per-tool statistics.
#[derive(Debug, Clone, Default)]
pub struct ToolStats {
    /// Total executions.
    pub executions: u64,
    /// Successful executions.
    pub successes: u64,
    /// Total time in ms.
    pub total_ms: u64,
}

impl ToolStats {
    /// Get success rate.
    pub fn success_rate(&self) -> f64 {
        if self.executions == 0 {
            return 0.0;
        }
        self.successes as f64 / self.executions as f64
    }

    /// Get average time.
    pub fn avg_ms(&self) -> f64 {
        if self.executions == 0 {
            return 0.0;
        }
        self.total_ms as f64 / self.executions as f64
    }
}

/// Assess risk of a shell command.
fn assess_command_risk(command: &str) -> RiskLevel {
    let command = command.to_lowercase();

    // Critical: system destruction
    if command.trim_start().starts_with("rm -rf /")
        && (command.trim_start() == "rm -rf /" || command.trim_start().starts_with("rm -rf / "))
        || command.contains("dd if=")
        || command.contains(":(){")
        || command.contains("mkfs")
    {
        return RiskLevel::Critical;
    }

    // High: destructive operations
    if command.contains("rm -rf")
        || command.contains("rm -r")
        || command.contains("rmdir")
        || command.contains("git push")
        || command.contains("git reset --hard")
        || command.contains("chmod 777")
        || command.contains("sudo")
        || command.contains("curl") && command.contains("| sh")
        || command.contains("wget") && command.contains("| sh")
    {
        return RiskLevel::High;
    }

    // Medium: file modifications
    if command.contains("mv ")
        || command.contains("cp ")
        || command.contains(">")
        || command.contains(">>")
        || command.contains("git commit")
        || command.contains("npm install")
        || command.contains("pip install")
    {
        return RiskLevel::Medium;
    }

    // Low: network or environment access
    if command.contains("curl")
        || command.contains("wget")
        || command.contains("ssh")
        || command.contains("env")
        || command.contains("export")
    {
        return RiskLevel::Low;
    }

    // Safe: read-only operations
    if command.starts_with("ls")
        || command.starts_with("cat ")
        || command.starts_with("head ")
        || command.starts_with("tail ")
        || command.starts_with("grep ")
        || command.starts_with("find ")
        || command.starts_with("pwd")
        || command.starts_with("echo ")
        || command.starts_with("git status")
        || command.starts_with("git log")
        || command.starts_with("git diff")
    {
        return RiskLevel::Safe;
    }

    RiskLevel::Medium
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_command_risk_assessment() {
        assert_eq!(assess_command_risk("ls -la"), RiskLevel::Safe);
        assert_eq!(assess_command_risk("cat file.txt"), RiskLevel::Safe);
        assert_eq!(assess_command_risk("git status"), RiskLevel::Safe);

        assert_eq!(
            assess_command_risk("curl https://example.com"),
            RiskLevel::Low
        );

        assert_eq!(assess_command_risk("mv file1 file2"), RiskLevel::Medium);
        assert_eq!(
            assess_command_risk("git commit -m 'test'"),
            RiskLevel::Medium
        );

        assert_eq!(assess_command_risk("rm -rf /tmp/test"), RiskLevel::High);
        assert_eq!(assess_command_risk("sudo apt install"), RiskLevel::High);

        assert_eq!(assess_command_risk("rm -rf /"), RiskLevel::Critical);
    }

    #[test]
    fn test_executor_stats() {
        let mut stats = ExecutorStats::default();
        stats.total_executions = 100;
        stats.successful_executions = 90;
        stats.cache_hits = 20;
        stats.total_duration_ms = 5000;

        assert!((stats.success_rate() - 0.9).abs() < 0.01);
        assert!((stats.cache_hit_rate() - 0.2).abs() < 0.01);
        assert!((stats.avg_duration_ms() - 50.0).abs() < 0.01);
    }
}
