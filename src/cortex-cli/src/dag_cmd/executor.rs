//! Task executor for running DAG tasks.

use anyhow::{Context, Result, bail};
use cortex_agents::task::{Task, TaskStatus};
use std::time::{Duration, Instant};

use crate::styled_output::print_info;

use super::types::TaskExecutionResult;

/// Task executor that runs the actual task commands.
pub struct TaskExecutor {
    timeout: Duration,
    verbose: bool,
}

impl TaskExecutor {
    pub fn new(timeout_secs: u64, verbose: bool) -> Self {
        Self {
            timeout: Duration::from_secs(timeout_secs),
            verbose,
        }
    }

    /// Execute a single task.
    pub async fn execute(&self, task: &Task) -> TaskExecutionResult {
        let start = Instant::now();
        let task_id = task.id.expect("Task must have an ID");

        // Get command from metadata if available
        let command = task
            .metadata
            .get("command")
            .and_then(|v| v.as_str())
            .map(String::from);

        if self.verbose {
            if let Some(ref cmd) = command {
                print_info(&format!("Executing task '{}': {}", task.name, cmd));
            } else {
                print_info(&format!(
                    "Executing task '{}': {}",
                    task.name, task.description
                ));
            }
        }

        // If there's a command, execute it
        let (status, output, error) = if let Some(cmd) = command {
            match self.run_command(&cmd).await {
                Ok(output) => (TaskStatus::Completed, Some(output), None),
                Err(e) => (TaskStatus::Failed, None, Some(e.to_string())),
            }
        } else {
            // Simulated task execution (no command)
            // In a real system, this would delegate to an agent or external executor
            (
                TaskStatus::Completed,
                Some(format!("Task '{}' completed (no command)", task.name)),
                None,
            )
        };

        TaskExecutionResult {
            task_id,
            task_name: task.name.clone(),
            status,
            duration: start.elapsed(),
            output,
            error,
        }
    }

    /// Run a shell command with timeout.
    async fn run_command(&self, cmd: &str) -> Result<String> {
        let timeout_duration = self.timeout;

        let result = tokio::time::timeout(timeout_duration, async {
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .output()
                .await
                .context("Failed to execute command")?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                Ok(stdout)
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();
                bail!("Command failed: {}", stderr);
            }
        })
        .await;

        match result {
            Ok(inner_result) => inner_result,
            Err(_) => bail!("Task timed out after {:?}", timeout_duration),
        }
    }
}
