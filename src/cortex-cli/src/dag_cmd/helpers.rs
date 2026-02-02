//! Helper functions for DAG command operations.

use anyhow::{Context, Result};
use cortex_agents::task::{TaskDag, TaskId, TaskSpec};
use std::collections::HashMap;
use std::path::PathBuf;

use super::types::{DagExecutionStats, DagOutputFormat, DagSpecInput};

/// Get the DAG store path.
pub fn get_dag_store_path() -> Result<PathBuf> {
    let data_dir = dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))
        .context("Could not determine data directory")?;
    Ok(data_dir.join("cortex").join("dags"))
}

/// Load DAG specification from file.
pub fn load_spec(path: &PathBuf) -> Result<DagSpecInput> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {}", path.display()))?;

    // Detect format from extension
    let spec: DagSpecInput = if path.extension().map(|e| e == "json").unwrap_or(false) {
        serde_json::from_str(&content).context("Failed to parse JSON")?
    } else {
        // Default to YAML
        serde_yaml::from_str(&content).context("Failed to parse YAML")?
    };

    // Check for duplicate task IDs (Issue #3815)
    let mut seen_names = std::collections::HashSet::new();
    for task in &spec.tasks {
        if !seen_names.insert(&task.name) {
            anyhow::bail!(
                "Duplicate task ID '{}' found in DAG specification. Task IDs must be unique.",
                task.name
            );
        }
    }

    Ok(spec)
}

/// Convert input spec to internal TaskSpec format.
pub fn convert_specs(input: &DagSpecInput) -> Vec<TaskSpec> {
    input
        .tasks
        .iter()
        .map(|t| {
            let mut spec = TaskSpec::new(&t.name, &t.description)
                .with_priority(t.priority)
                .with_affected_files(t.affected_files.clone());

            if let Some(duration) = t.estimated_duration {
                spec = spec.with_estimated_duration(duration);
            }

            for dep in &t.depends_on {
                spec = spec.depends_on(dep);
            }

            // Store command in metadata
            if let Some(cmd) = &t.command {
                spec = spec.with_metadata("command", serde_json::json!(cmd));
            }

            for (key, value) in &t.metadata {
                spec = spec.with_metadata(key, value.clone());
            }

            spec
        })
        .collect()
}

/// Print a summary of the DAG.
pub fn print_dag_summary(dag: &TaskDag) {
    println!("Tasks:");
    for task in dag.all_tasks() {
        let deps = task
            .id
            .and_then(|id| dag.get_dependencies(id))
            .map(|d| d.len())
            .unwrap_or(0);
        let dep_str = if deps > 0 {
            format!(" (depends on {} tasks)", deps)
        } else {
            String::new()
        };
        println!("  - {}{}", task.name, dep_str);
    }
}

/// Print execution order.
pub fn print_execution_order(dag: &TaskDag) -> Result<()> {
    let order = dag
        .topological_sort()
        .map_err(|e| anyhow::anyhow!("Failed to sort: {}", e))?;

    println!("Execution order:");
    for (i, task_id) in order.iter().enumerate() {
        if let Some(task) = dag.get_task(*task_id) {
            println!("  {}. {}", i + 1, task.name);
        }
    }

    Ok(())
}

/// Print execution summary.
pub fn print_execution_summary(stats: &DagExecutionStats, format: DagOutputFormat) {
    match format {
        DagOutputFormat::Text => {
            println!("═══════════════════════════════════════");
            println!("Execution Summary");
            println!("═══════════════════════════════════════");
            println!(
                "Total:     {} tasks",
                stats.completed_tasks + stats.failed_tasks + stats.skipped_tasks
            );
            println!("Completed: {} tasks", stats.completed_tasks);
            println!("Failed:    {} tasks", stats.failed_tasks);
            println!("Skipped:   {} tasks", stats.skipped_tasks);
            println!("Duration:  {:.2}s", stats.total_duration.as_secs_f64());
        }
        DagOutputFormat::Json => {
            let output = serde_json::json!({
                "completed": stats.completed_tasks,
                "failed": stats.failed_tasks,
                "skipped": stats.skipped_tasks,
                "duration_secs": stats.total_duration.as_secs_f64()
            });
            println!(
                "{}",
                serde_json::to_string_pretty(&output)
                    .expect("JSON serialization should not fail for DagExecutionStats")
            );
        }
        DagOutputFormat::Compact => {
            println!(
                "{}/{} completed, {} failed, {:.2}s",
                stats.completed_tasks,
                stats.completed_tasks + stats.failed_tasks + stats.skipped_tasks,
                stats.failed_tasks,
                stats.total_duration.as_secs_f64()
            );
        }
    }
}

/// Print ASCII graph representation.
pub fn print_ascii_graph(dag: &TaskDag, spec: &DagSpecInput) {
    println!("DAG: {}", spec.name.as_deref().unwrap_or("unnamed"));
    println!();

    // Group tasks by depth
    let order = dag.topological_sort().unwrap_or_default();
    let mut depth_map: HashMap<TaskId, usize> = HashMap::new();

    for task_id in &order {
        let deps = dag.get_dependencies(*task_id).cloned().unwrap_or_default();
        let max_dep_depth = deps
            .iter()
            .filter_map(|d| depth_map.get(d))
            .max()
            .copied()
            .unwrap_or(0);
        let depth = if deps.is_empty() {
            0
        } else {
            max_dep_depth + 1
        };
        depth_map.insert(*task_id, depth);
    }

    // Group by depth
    let max_depth = depth_map.values().max().copied().unwrap_or(0);
    for depth in 0..=max_depth {
        let tasks_at_depth: Vec<_> = order
            .iter()
            .filter(|id| depth_map.get(id) == Some(&depth))
            .collect();

        let indent = "  ".repeat(depth);
        for task_id in tasks_at_depth {
            if let Some(task) = dag.get_task(*task_id) {
                let deps = dag.get_dependencies(*task_id);
                let arrow = if deps.is_some_and(|d| !d.is_empty()) {
                    "└─► "
                } else {
                    "● "
                };
                println!("{}{}{}", indent, arrow, task.name);
            }
        }
    }
}

/// Print Graphviz DOT format.
pub fn print_dot_graph(dag: &TaskDag, spec: &DagSpecInput) {
    use cortex_agents::task::TaskStatus;

    println!("digraph DAG {{");
    println!("  rankdir=TB;");
    println!(
        "  label=\"{}\";",
        spec.name.as_deref().unwrap_or("Task DAG")
    );
    println!("  node [shape=box];");
    println!();

    for task in dag.all_tasks() {
        let Some(task_id) = task.id else { continue };
        let color = match task.status {
            TaskStatus::Completed => "green",
            TaskStatus::Failed => "red",
            TaskStatus::Skipped => "gray",
            TaskStatus::Running => "yellow",
            _ => "white",
        };
        println!(
            "  \"{}\" [label=\"{}\", style=filled, fillcolor={}];",
            task_id.inner(),
            task.name,
            color
        );
    }

    println!();

    for task in dag.all_tasks() {
        let Some(task_id) = task.id else { continue };
        if let Some(deps) = dag.get_dependencies(task_id) {
            for dep_id in deps {
                println!("  \"{}\" -> \"{}\";", dep_id.inner(), task_id.inner());
            }
        }
    }

    println!("}}");
}

/// Print Mermaid diagram format.
pub fn print_mermaid_graph(dag: &TaskDag, spec: &DagSpecInput) {
    println!("```mermaid");
    println!("graph TD");
    println!("  subgraph {}", spec.name.as_deref().unwrap_or("Task DAG"));

    // Create task ID to safe name mapping
    let mut id_to_name: HashMap<TaskId, String> = HashMap::new();
    for task in dag.all_tasks() {
        let Some(task_id) = task.id else { continue };
        let safe_name = task.name.replace([' ', '-'], "_");
        id_to_name.insert(task_id, safe_name.clone());
        println!("    {}[{}]", safe_name, task.name);
    }

    for task in dag.all_tasks() {
        let Some(task_id) = task.id else { continue };
        if let Some(deps) = dag.get_dependencies(task_id) {
            for dep_id in deps {
                if let (Some(from), Some(to)) = (id_to_name.get(dep_id), id_to_name.get(&task_id)) {
                    println!("    {} --> {}", from, to);
                }
            }
        }
    }

    println!("  end");
    println!("```");
}
