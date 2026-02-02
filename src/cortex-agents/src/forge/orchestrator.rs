//! DAG-based orchestration for Forge agent validation.
//!
//! This module provides the orchestrator that executes validation agents
//! according to their dependencies, running independent agents in parallel
//! where possible.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::forge::{ForgeOrchestrator, ForgeConfig, ValidationResult};
//!
//! // Create orchestrator with configuration
//! let config = ForgeConfig::default();
//! let orchestrator = ForgeOrchestrator::new(config);
//!
//! // Run all agents and collect results
//! let response = orchestrator.run(|agent_id, agent_config| async move {
//!     // Execute the agent and return validation result
//!     Ok(ValidationResult::pass(agent_id))
//! }).await?;
//!
//! if response.is_success() {
//!     println!("All validations passed!");
//! }
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::Instant;

use chrono::Utc;
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};

use super::config::{AgentConfig, ForgeConfig};
use super::protocol::{ForgeResponse, ValidationResult, ValidationStatus};

/// Errors that can occur during orchestration.
#[derive(Debug, Error)]
pub enum OrchestratorError {
    /// Configuration error.
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Circular dependency detected.
    #[error("Circular dependency detected: {path}")]
    CircularDependency { path: String },

    /// Agent execution failed.
    #[error("Agent '{agent_id}' failed: {message}")]
    AgentFailed { agent_id: String, message: String },

    /// Agent not found.
    #[error("Agent not found: {0}")]
    AgentNotFound(String),

    /// Timeout exceeded.
    #[error("Orchestration timeout exceeded after {seconds}s")]
    Timeout { seconds: u64 },

    /// Dependencies not satisfied.
    #[error("Agent '{agent_id}' has unsatisfied dependencies: {missing:?}")]
    DependenciesNotSatisfied {
        agent_id: String,
        missing: Vec<String>,
    },

    /// Internal execution error.
    #[error("Internal error: {0}")]
    Internal(String),
}

/// Result type for orchestration operations.
pub type OrchestratorResult<T> = std::result::Result<T, OrchestratorError>;

/// State of an agent during orchestration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentState {
    /// Waiting for dependencies.
    Pending,
    /// Ready to execute.
    Ready,
    /// Currently executing.
    Running,
    /// Completed successfully (passed or warned).
    Completed,
    /// Failed execution.
    Failed,
    /// Skipped due to failed dependencies.
    Skipped,
}

impl AgentState {
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentState::Completed | AgentState::Failed | AgentState::Skipped
        )
    }

    fn is_success(&self) -> bool {
        matches!(self, AgentState::Completed)
    }
}

/// Internal tracking for an agent during orchestration.
struct AgentTracker {
    config: AgentConfig,
    state: AgentState,
    result: Option<ValidationResult>,
}

/// Options for controlling orchestration behavior.
#[derive(Debug, Clone)]
pub struct OrchestratorOptions {
    /// Maximum number of agents to run in parallel.
    pub max_parallel: usize,
    /// Whether to fail fast on first error.
    pub fail_fast: bool,
    /// Global timeout in seconds.
    pub timeout_seconds: u64,
}

impl Default for OrchestratorOptions {
    fn default() -> Self {
        Self {
            max_parallel: 4,
            fail_fast: false,
            timeout_seconds: 600,
        }
    }
}

impl OrchestratorOptions {
    /// Set max parallel executions.
    pub fn with_max_parallel(mut self, max: usize) -> Self {
        self.max_parallel = max;
        self
    }

    /// Enable fail-fast mode.
    pub fn with_fail_fast(mut self, fail_fast: bool) -> Self {
        self.fail_fast = fail_fast;
        self
    }

    /// Set timeout.
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }
}

/// The Forge orchestrator that manages agent execution.
pub struct ForgeOrchestrator {
    config: ForgeConfig,
    options: OrchestratorOptions,
}

impl ForgeOrchestrator {
    /// Create a new orchestrator with the given configuration.
    pub fn new(config: ForgeConfig) -> Self {
        let options = OrchestratorOptions {
            max_parallel: config.global.max_parallel,
            fail_fast: config.global.fail_fast,
            timeout_seconds: config.global.timeout_seconds,
        };

        Self { config, options }
    }

    /// Create an orchestrator with custom options.
    pub fn with_options(config: ForgeConfig, options: OrchestratorOptions) -> Self {
        Self { config, options }
    }

    /// Get the configuration.
    pub fn config(&self) -> &ForgeConfig {
        &self.config
    }

    /// Get the options.
    pub fn options(&self) -> &OrchestratorOptions {
        &self.options
    }

    /// Run all enabled agents with the provided executor function.
    ///
    /// The executor function receives the agent ID and configuration,
    /// and should return a `ValidationResult`.
    pub async fn run<F, Fut>(&self, executor: F) -> OrchestratorResult<ForgeResponse>
    where
        F: Fn(String, AgentConfig) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = OrchestratorResult<ValidationResult>> + Send,
    {
        let start_time = Instant::now();

        // Build the execution graph
        let execution_order = self.build_execution_order()?;

        // Initialize trackers
        let trackers: Arc<RwLock<HashMap<String, AgentTracker>>> = Arc::new(RwLock::new(
            self.config
                .enabled_agents()
                .map(|a| {
                    (
                        a.id.clone(),
                        AgentTracker {
                            config: a.clone(),
                            state: if a.depends_on.is_empty() {
                                AgentState::Ready
                            } else {
                                AgentState::Pending
                            },
                            result: None,
                        },
                    )
                })
                .collect(),
        ));

        let results: Arc<Mutex<Vec<ValidationResult>>> = Arc::new(Mutex::new(Vec::new()));
        let errors: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let executor = Arc::new(executor);

        // Process agents in waves based on dependencies
        let mut processed = HashSet::new();

        while processed.len() < execution_order.len() {
            // Check for timeout
            let elapsed = start_time.elapsed().as_secs();
            if elapsed > self.options.timeout_seconds {
                return Err(OrchestratorError::Timeout {
                    seconds: self.options.timeout_seconds,
                });
            }

            // Find ready agents
            let ready_agents: Vec<String> = {
                let trackers_read = trackers.read().await;
                trackers_read
                    .iter()
                    .filter(|(id, tracker)| {
                        !processed.contains(*id) && tracker.state == AgentState::Ready
                    })
                    .map(|(id, _)| id.clone())
                    .take(self.options.max_parallel)
                    .collect()
            };

            if ready_agents.is_empty() {
                // Check if we're stuck (all remaining agents have unresolved dependencies)
                let trackers_read = trackers.read().await;
                let any_running = trackers_read
                    .values()
                    .any(|t| t.state == AgentState::Running);

                if !any_running {
                    // No agents running and none ready - we're stuck or done
                    let pending: Vec<_> = trackers_read
                        .iter()
                        .filter(|(id, t)| {
                            !processed.contains(*id) && t.state == AgentState::Pending
                        })
                        .map(|(id, _)| id.clone())
                        .collect();

                    if !pending.is_empty() {
                        return Err(OrchestratorError::DependenciesNotSatisfied {
                            agent_id: pending[0].clone(),
                            missing: pending,
                        });
                    }
                    break;
                }

                // Wait a bit for running agents to complete
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                continue;
            }

            // Execute ready agents in parallel
            let mut handles = Vec::new();

            for agent_id in ready_agents {
                let agent_config = {
                    let mut trackers_write = trackers.write().await;
                    if let Some(tracker) = trackers_write.get_mut(&agent_id) {
                        tracker.state = AgentState::Running;
                        tracker.config.clone()
                    } else {
                        continue;
                    }
                };

                let trackers_clone = Arc::clone(&trackers);
                let results_clone = Arc::clone(&results);
                let errors_clone = Arc::clone(&errors);
                let executor_clone = Arc::clone(&executor);
                let fail_fast = self.options.fail_fast;
                let require_pass = agent_config.require_dependencies_pass;

                let handle = tokio::spawn(async move {
                    let agent_id_clone = agent_config.id.clone();

                    // Check if dependencies passed (if required)
                    if require_pass {
                        let trackers_read = trackers_clone.read().await;
                        for dep_id in &agent_config.depends_on {
                            if let Some(dep_tracker) = trackers_read.get(dep_id) {
                                if !dep_tracker.state.is_success() {
                                    // Skip this agent - dependency didn't pass
                                    drop(trackers_read);
                                    let mut trackers_write = trackers_clone.write().await;
                                    if let Some(tracker) = trackers_write.get_mut(&agent_id_clone) {
                                        tracker.state = AgentState::Skipped;
                                        tracker.result = Some(ValidationResult {
                                            status: ValidationStatus::Warning,
                                            agent_id: agent_id_clone.clone(),
                                            rules_applied: vec![],
                                            findings: vec![],
                                            timestamp: Utc::now(),
                                        });
                                    }
                                    return (agent_id_clone, true);
                                }
                            }
                        }
                    }

                    // Execute the agent
                    let result = executor_clone(agent_id_clone.clone(), agent_config).await;

                    match result {
                        Ok(validation_result) => {
                            let failed = validation_result.status == ValidationStatus::Fail;
                            let mut results_lock = results_clone.lock().await;
                            results_lock.push(validation_result.clone());
                            drop(results_lock);

                            let mut trackers_write = trackers_clone.write().await;
                            if let Some(tracker) = trackers_write.get_mut(&agent_id_clone) {
                                tracker.state = if failed {
                                    AgentState::Failed
                                } else {
                                    AgentState::Completed
                                };
                                tracker.result = Some(validation_result);
                            }

                            (agent_id_clone, failed && fail_fast)
                        }
                        Err(e) => {
                            let mut errors_lock = errors_clone.lock().await;
                            errors_lock.push(format!("Agent '{}' error: {}", agent_id_clone, e));
                            drop(errors_lock);

                            let mut trackers_write = trackers_clone.write().await;
                            if let Some(tracker) = trackers_write.get_mut(&agent_id_clone) {
                                tracker.state = AgentState::Failed;
                            }

                            (agent_id_clone, fail_fast)
                        }
                    }
                });

                handles.push(handle);
            }

            // Wait for this wave to complete
            let mut should_stop = false;
            for handle in handles {
                match handle.await {
                    Ok((agent_id, stop)) => {
                        processed.insert(agent_id.clone());

                        // Update dependent agents to ready if all dependencies are met
                        let mut trackers_write = trackers.write().await;
                        let completed_ids: HashSet<String> = trackers_write
                            .iter()
                            .filter(|(_, t)| t.state.is_terminal())
                            .map(|(id, _)| id.clone())
                            .collect();

                        for (_id, tracker) in trackers_write.iter_mut() {
                            if tracker.state == AgentState::Pending {
                                let deps_met = tracker
                                    .config
                                    .depends_on
                                    .iter()
                                    .all(|d| completed_ids.contains(d));
                                if deps_met {
                                    tracker.state = AgentState::Ready;
                                }
                            }
                        }

                        if stop {
                            should_stop = true;
                        }
                    }
                    Err(e) => {
                        let mut errors_lock = errors.lock().await;
                        errors_lock.push(format!("Task join error: {e}"));
                        if self.options.fail_fast {
                            should_stop = true;
                        }
                    }
                }
            }

            if should_stop {
                break;
            }
        }

        // Collect final results - use lock() which is more robust than try_unwrap()
        // when tasks might still be running (e.g., after fail-fast early exit)
        let final_results = {
            let guard = results.lock().await;
            guard.clone()
        };

        let final_errors = {
            let guard = errors.lock().await;
            guard.clone()
        };

        let execution_time_ms = start_time.elapsed().as_millis() as u64;

        let response = ForgeResponse::new(final_results, execution_time_ms);
        let response = if final_errors.is_empty() {
            response
        } else {
            response.with_errors(final_errors)
        };

        Ok(response)
    }

    /// Build the execution order using topological sort.
    fn build_execution_order(&self) -> OrchestratorResult<Vec<String>> {
        let enabled: HashMap<String, &AgentConfig> = self
            .config
            .enabled_agents()
            .map(|a| (a.id.clone(), a))
            .collect();

        // Calculate in-degrees
        let mut in_degree: HashMap<String, usize> =
            enabled.keys().map(|id| (id.clone(), 0)).collect();

        for agent in enabled.values() {
            for dep in &agent.depends_on {
                if enabled.contains_key(dep) {
                    // Increment in-degree - entry must exist since we initialized all enabled agents
                    if let Some(degree) = in_degree.get_mut(&agent.id) {
                        *degree += 1;
                    }
                }
            }
        }

        // Build reverse adjacency (who depends on whom)
        let mut dependents: HashMap<String, Vec<String>> = HashMap::new();
        for agent in enabled.values() {
            for dep in &agent.depends_on {
                if enabled.contains_key(dep) {
                    dependents
                        .entry(dep.clone())
                        .or_default()
                        .push(agent.id.clone());
                }
            }
        }

        // Kahn's algorithm for topological sort
        let mut queue: VecDeque<String> = in_degree
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(id, _)| id.clone())
            .collect();

        // Sort by priority for consistent ordering
        let mut queue_vec: Vec<_> = queue.drain(..).collect();
        queue_vec.sort_by(|a, b| {
            let pa = enabled.get(a).map(|c| c.priority).unwrap_or(0);
            let pb = enabled.get(b).map(|c| c.priority).unwrap_or(0);
            pb.cmp(&pa) // Higher priority first
        });
        queue.extend(queue_vec);

        let mut result = Vec::new();

        while let Some(id) = queue.pop_front() {
            result.push(id.clone());

            if let Some(deps) = dependents.get(&id) {
                for dep_id in deps {
                    if let Some(degree) = in_degree.get_mut(dep_id) {
                        *degree -= 1;
                        if *degree == 0 {
                            queue.push_back(dep_id.clone());
                        }
                    }
                }
            }
        }

        // Check for cycles
        if result.len() != enabled.len() {
            let remaining: Vec<_> = enabled
                .keys()
                .filter(|id| !result.contains(id))
                .cloned()
                .collect();

            return Err(OrchestratorError::CircularDependency {
                path: remaining.join(" -> "),
            });
        }

        Ok(result)
    }

    /// Get the execution order without running.
    pub fn get_execution_order(&self) -> OrchestratorResult<Vec<String>> {
        self.build_execution_order()
    }

    /// Check if an agent can run based on its dependencies.
    pub fn can_run(&self, agent_id: &str, completed: &HashSet<String>) -> bool {
        if let Some(agent) = self.config.get_agent(agent_id) {
            agent.depends_on.iter().all(|d| completed.contains(d))
        } else {
            false
        }
    }
}

/// Builder for creating an orchestrator with fluent API.
#[derive(Debug, Default)]
pub struct OrchestratorBuilder {
    config: ForgeConfig,
    options: OrchestratorOptions,
}

impl OrchestratorBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the configuration.
    pub fn with_config(mut self, config: ForgeConfig) -> Self {
        self.config = config;
        self
    }

    /// Add an agent to the configuration.
    pub fn add_agent(mut self, agent: AgentConfig) -> Self {
        self.config.add_agent(agent);
        self
    }

    /// Set max parallel executions.
    pub fn max_parallel(mut self, max: usize) -> Self {
        self.options.max_parallel = max;
        self
    }

    /// Enable fail-fast mode.
    pub fn fail_fast(mut self, enabled: bool) -> Self {
        self.options.fail_fast = enabled;
        self
    }

    /// Set timeout.
    pub fn timeout(mut self, seconds: u64) -> Self {
        self.options.timeout_seconds = seconds;
        self
    }

    /// Build the orchestrator.
    pub fn build(self) -> OrchestratorResult<ForgeOrchestrator> {
        self.config
            .validate()
            .map_err(|e| OrchestratorError::ConfigError(e.to_string()))?;

        Ok(ForgeOrchestrator::with_options(self.config, self.options))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_config() -> ForgeConfig {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("lint").with_priority(10));
        config.add_agent(
            AgentConfig::new("security")
                .depends_on("lint")
                .with_priority(5),
        );
        config.add_agent(
            AgentConfig::new("quality")
                .depends_on("lint")
                .with_priority(5),
        );
        config.add_agent(
            AgentConfig::new("final")
                .depends_on("security")
                .depends_on("quality"),
        );
        config
    }

    #[test]
    fn test_build_execution_order() {
        let orchestrator = ForgeOrchestrator::new(simple_config());
        let order = orchestrator.get_execution_order().expect("should succeed");

        // lint should be first
        assert_eq!(order[0], "lint");

        // final should be last
        assert_eq!(order[order.len() - 1], "final");

        // security and quality should be before final
        let final_pos = order.iter().position(|id| id == "final").unwrap();
        let security_pos = order.iter().position(|id| id == "security").unwrap();
        let quality_pos = order.iter().position(|id| id == "quality").unwrap();

        assert!(security_pos < final_pos);
        assert!(quality_pos < final_pos);
    }

    #[test]
    fn test_circular_dependency_detection() {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("a").depends_on("c"));
        config.add_agent(AgentConfig::new("b").depends_on("a"));
        config.add_agent(AgentConfig::new("c").depends_on("b"));

        let orchestrator = ForgeOrchestrator::new(config);
        let result = orchestrator.get_execution_order();

        assert!(matches!(
            result,
            Err(OrchestratorError::CircularDependency { .. })
        ));
    }

    #[test]
    fn test_can_run() {
        let orchestrator = ForgeOrchestrator::new(simple_config());

        let mut completed = HashSet::new();

        // lint can run immediately
        assert!(orchestrator.can_run("lint", &completed));

        // security cannot run until lint is done
        assert!(!orchestrator.can_run("security", &completed));

        completed.insert("lint".to_string());
        assert!(orchestrator.can_run("security", &completed));
        assert!(orchestrator.can_run("quality", &completed));

        // final cannot run until both security and quality are done
        assert!(!orchestrator.can_run("final", &completed));

        completed.insert("security".to_string());
        assert!(!orchestrator.can_run("final", &completed));

        completed.insert("quality".to_string());
        assert!(orchestrator.can_run("final", &completed));
    }

    #[test]
    fn test_orchestrator_builder() {
        let orchestrator = OrchestratorBuilder::new()
            .add_agent(AgentConfig::new("test"))
            .max_parallel(8)
            .fail_fast(true)
            .timeout(300)
            .build()
            .expect("should build");

        assert_eq!(orchestrator.options().max_parallel, 8);
        assert!(orchestrator.options().fail_fast);
        assert_eq!(orchestrator.options().timeout_seconds, 300);
    }

    #[tokio::test]
    async fn test_run_simple() {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("test1"));
        config.add_agent(AgentConfig::new("test2"));

        let orchestrator = ForgeOrchestrator::new(config);

        let response = orchestrator
            .run(|agent_id, _config| async move { Ok(ValidationResult::pass(agent_id)) })
            .await
            .expect("should succeed");

        assert!(response.is_success());
        assert_eq!(response.results.len(), 2);
    }

    #[tokio::test]
    async fn test_run_with_dependencies() {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("first"));
        config.add_agent(AgentConfig::new("second").depends_on("first"));

        let orchestrator = ForgeOrchestrator::new(config);

        let execution_order = Arc::new(Mutex::new(Vec::new()));
        let order_clone = Arc::clone(&execution_order);

        let response = orchestrator
            .run(move |agent_id, _config| {
                let order = Arc::clone(&order_clone);
                async move {
                    let mut order_lock = order.lock().await;
                    order_lock.push(agent_id.clone());
                    Ok(ValidationResult::pass(agent_id))
                }
            })
            .await
            .expect("should succeed");

        assert!(response.is_success());

        let order = execution_order.lock().await;
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], "first");
        assert_eq!(order[1], "second");
    }

    #[tokio::test]
    async fn test_run_fail_fast() {
        let mut config = ForgeConfig::new();
        config.global.fail_fast = true;
        config.add_agent(AgentConfig::new("will-fail"));
        config.add_agent(AgentConfig::new("wont-run").depends_on("will-fail"));

        let orchestrator = ForgeOrchestrator::new(config);

        let response = orchestrator
            .run(|agent_id, _config| async move {
                if agent_id == "will-fail" {
                    Ok(ValidationResult::fail(agent_id, vec![]))
                } else {
                    Ok(ValidationResult::pass(agent_id))
                }
            })
            .await
            .expect("should succeed");

        // The overall response should indicate failure
        assert!(!response.is_success());

        // Only the first agent should have run
        assert_eq!(response.results.len(), 1);
    }

    #[tokio::test]
    async fn test_run_parallel_independent() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let mut config = ForgeConfig::new();
        config.global.max_parallel = 4;
        config.add_agent(AgentConfig::new("a"));
        config.add_agent(AgentConfig::new("b"));
        config.add_agent(AgentConfig::new("c"));
        config.add_agent(AgentConfig::new("d"));

        let orchestrator = ForgeOrchestrator::new(config);

        let concurrent_count = Arc::new(AtomicUsize::new(0));
        let max_concurrent = Arc::new(AtomicUsize::new(0));

        let concurrent_clone = Arc::clone(&concurrent_count);
        let max_clone = Arc::clone(&max_concurrent);

        let response = orchestrator
            .run(move |agent_id, _config| {
                let concurrent = Arc::clone(&concurrent_clone);
                let max = Arc::clone(&max_clone);
                async move {
                    let current = concurrent.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(current, Ordering::SeqCst);

                    // Simulate work
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

                    concurrent.fetch_sub(1, Ordering::SeqCst);
                    Ok(ValidationResult::pass(agent_id))
                }
            })
            .await
            .expect("should succeed");

        assert!(response.is_success());
        assert_eq!(response.results.len(), 4);

        // All 4 should have run in parallel
        let max = max_concurrent.load(Ordering::SeqCst);
        assert!(
            max > 1,
            "Expected parallel execution, got max concurrent: {max}"
        );
    }
}
