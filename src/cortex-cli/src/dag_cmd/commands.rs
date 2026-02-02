//! Command handlers for DAG operations.

use anyhow::{Result, bail};
use cortex_agents::task::{DagHydrator, DagStore, TaskStatus};
use std::collections::HashMap;
use std::io::{self, Write};

use crate::styled_output::{print_error, print_info, print_success};

use super::args::{
    DagCreateArgs, DagDeleteArgs, DagGraphArgs, DagListArgs, DagResumeArgs, DagRunArgs,
    DagStatusArgs, DagValidateArgs,
};
use super::helpers::{
    convert_specs, get_dag_store_path, load_spec, print_ascii_graph, print_dag_summary,
    print_dot_graph, print_execution_order, print_execution_summary, print_mermaid_graph,
};
use super::scheduler::DagScheduler;
use super::types::{DagOutputFormat, ExecutionStrategy};

/// Create a DAG from specification.
pub async fn run_create(args: DagCreateArgs) -> Result<()> {
    let spec = load_spec(&args.file)?;
    let specs = convert_specs(&spec);

    let dag = DagHydrator::new()
        .hydrate_from_specs(&specs)
        .map_err(|e| anyhow::anyhow!("Failed to create DAG: {}", e))?;

    if args.dry_run {
        print_success(&format!("✓ DAG is valid ({} tasks)", dag.len()));
        return Ok(());
    }

    // Generate ID if not provided
    let id = args.id.unwrap_or_else(|| {
        let now = chrono::Utc::now();
        format!(
            "dag-{}-{}",
            spec.name.as_deref().unwrap_or("unnamed"),
            now.format("%Y%m%d-%H%M%S")
        )
    });

    // Get storage path
    let store_path = get_dag_store_path()?;
    let store = DagStore::new(&store_path);
    store.save(&id, &dag).await?;

    match args.format {
        DagOutputFormat::Text => {
            print_success(&format!("✓ Created DAG '{}' with {} tasks", id, dag.len()));
            println!();
            print_dag_summary(&dag);
        }
        DagOutputFormat::Json => {
            let output = serde_json::json!({
                "id": id,
                "task_count": dag.len(),
                "status": "created"
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        DagOutputFormat::Compact => {
            println!("{} {} tasks", id, dag.len());
        }
    }

    Ok(())
}

/// Execute a DAG.
pub async fn run_execute(args: DagRunArgs) -> Result<()> {
    let spec = load_spec(&args.file)?;
    let specs = convert_specs(&spec);

    let hydrator = if args.infer_deps {
        DagHydrator::new().with_file_inference()
    } else {
        DagHydrator::new()
    };

    let dag = hydrator
        .hydrate_from_specs(&specs)
        .map_err(|e| anyhow::anyhow!("Failed to create DAG: {}", e))?;

    if matches!(args.strategy, ExecutionStrategy::DryRun) {
        // Dry run - just validate
        dag.topological_sort()
            .map_err(|e| anyhow::anyhow!("DAG validation failed: {}", e))?;
        print_success(&format!(
            "✓ DAG is valid ({} tasks in execution order)",
            dag.len()
        ));
        println!();
        print_execution_order(&dag)?;
        return Ok(());
    }

    if !args.quiet {
        println!();
        print_info(&format!(
            "Executing DAG with {} tasks (max {} concurrent)",
            dag.len(),
            args.max_concurrent
        ));
        println!();
    }

    let scheduler = DagScheduler::new(
        dag.clone(),
        args.max_concurrent,
        args.timeout,
        args.failure_mode,
        args.verbose,
        args.quiet,
    );

    let stats = match args.strategy {
        ExecutionStrategy::Parallel => scheduler.execute().await?,
        ExecutionStrategy::Sequential => scheduler.execute_sequential().await?,
        ExecutionStrategy::DryRun => unreachable!(),
    };

    // Save if requested
    if args.save {
        let id = args.id.unwrap_or_else(|| {
            let now = chrono::Utc::now();
            format!(
                "dag-{}-{}",
                spec.name.as_deref().unwrap_or("run"),
                now.format("%Y%m%d-%H%M%S")
            )
        });
        let store_path = get_dag_store_path()?;
        let store = DagStore::new(&store_path);
        let dag = scheduler.dag.read().await;
        store.save(&id, &dag).await?;
        if !args.quiet {
            print_info(&format!("DAG saved with ID: {}", id));
        }
    }

    // Print summary
    if !args.quiet {
        println!();
        print_execution_summary(&stats, args.format);
    }

    // Exit with error if any tasks failed
    if stats.failed_tasks > 0 {
        bail!(
            "{} task(s) failed, {} skipped",
            stats.failed_tasks,
            stats.skipped_tasks
        );
    }

    Ok(())
}

/// Show DAG status.
pub async fn run_status(args: DagStatusArgs) -> Result<()> {
    let store_path = get_dag_store_path()?;
    let store = DagStore::new(&store_path);

    let dag = store.load(&args.id).await.map_err(|e| match e {
        cortex_agents::task::PersistenceError::NotFound(_) => {
            anyhow::anyhow!("DAG '{}' not found", args.id)
        }
        e => anyhow::anyhow!("Failed to load DAG: {}", e),
    })?;

    let counts = dag.status_counts();

    match args.format {
        DagOutputFormat::Text => {
            println!("DAG: {}", args.id);
            println!("Total tasks: {}", dag.len());
            println!();
            println!("Status:");
            for (status, count) in counts {
                let icon = match status {
                    TaskStatus::Pending => "⏳",
                    TaskStatus::Ready => "🔵",
                    TaskStatus::Running => "🔄",
                    TaskStatus::Completed => "✓",
                    TaskStatus::Failed => "✗",
                    TaskStatus::Skipped => "⏭",
                    TaskStatus::Cancelled => "⛔",
                };
                println!("  {} {:?}: {}", icon, status, count);
            }

            if args.verbose {
                println!();
                println!("Tasks:");
                for task in dag.all_tasks() {
                    let status_icon = match task.status {
                        TaskStatus::Pending => "⏳",
                        TaskStatus::Ready => "🔵",
                        TaskStatus::Running => "🔄",
                        TaskStatus::Completed => "✓",
                        TaskStatus::Failed => "✗",
                        TaskStatus::Skipped => "⏭",
                        TaskStatus::Cancelled => "⛔",
                    };
                    println!("  {} {} - {}", status_icon, task.name, task.description);
                    if let Some(error) = &task.error {
                        println!("      Error: {}", error);
                    }
                }
            }
        }
        DagOutputFormat::Json => {
            let status_counts: HashMap<String, usize> = counts
                .into_iter()
                .map(|(k, v)| (format!("{:?}", k).to_lowercase(), v))
                .collect();

            let output = serde_json::json!({
                "id": args.id,
                "total_tasks": dag.len(),
                "status_counts": status_counts,
                "is_complete": dag.is_complete(),
                "all_succeeded": dag.all_succeeded()
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        DagOutputFormat::Compact => {
            let completed = counts.get(&TaskStatus::Completed).unwrap_or(&0);
            let failed = counts.get(&TaskStatus::Failed).unwrap_or(&0);
            println!(
                "{} {}/{} completed, {} failed",
                args.id,
                completed,
                dag.len(),
                failed
            );
        }
    }

    Ok(())
}

/// List all DAGs.
pub async fn run_list(args: DagListArgs) -> Result<()> {
    let store_path = get_dag_store_path()?;
    let store = DagStore::new(&store_path);

    let ids = store.list().await?;

    if ids.is_empty() {
        print_info("No DAGs found");
        return Ok(());
    }

    let limit = args.limit.unwrap_or(ids.len());
    let ids: Vec<_> = ids.into_iter().take(limit).collect();

    match args.format {
        DagOutputFormat::Text => {
            println!("DAGs:");
            for id in &ids {
                if let Ok(dag) = store.load(id).await {
                    let counts = dag.status_counts();
                    let completed = counts.get(&TaskStatus::Completed).unwrap_or(&0);
                    let failed = counts.get(&TaskStatus::Failed).unwrap_or(&0);
                    let status = if dag.all_succeeded() {
                        "✓"
                    } else if *failed > 0 {
                        "✗"
                    } else if dag.is_complete() {
                        "⏭"
                    } else {
                        "⏳"
                    };
                    println!(
                        "  {} {} ({}/{} tasks, {} failed)",
                        status,
                        id,
                        completed,
                        dag.len(),
                        failed
                    );
                } else {
                    println!("  ? {} (error loading)", id);
                }
            }
        }
        DagOutputFormat::Json => {
            let mut dags = Vec::new();
            for id in &ids {
                if let Ok(dag) = store.load(id).await {
                    let counts = dag.status_counts();
                    dags.push(serde_json::json!({
                        "id": id,
                        "task_count": dag.len(),
                        "is_complete": dag.is_complete(),
                        "all_succeeded": dag.all_succeeded(),
                        "completed": counts.get(&TaskStatus::Completed).unwrap_or(&0),
                        "failed": counts.get(&TaskStatus::Failed).unwrap_or(&0)
                    }));
                }
            }
            println!("{}", serde_json::to_string_pretty(&dags)?);
        }
        DagOutputFormat::Compact => {
            for id in &ids {
                if let Ok(dag) = store.load(id).await {
                    let counts = dag.status_counts();
                    let completed = counts.get(&TaskStatus::Completed).unwrap_or(&0);
                    println!("{} {}/{}", id, completed, dag.len());
                }
            }
        }
    }

    Ok(())
}

/// Validate a DAG specification.
pub async fn run_validate(args: DagValidateArgs) -> Result<()> {
    let spec = load_spec(&args.file)?;
    let specs = convert_specs(&spec);

    // Check for cycle detection and other issues
    match DagHydrator::new().hydrate_from_specs(&specs) {
        Ok(dag) => {
            // Verify topological sort works
            match dag.topological_sort() {
                Ok(order) => {
                    print_success(&format!(
                        "✓ DAG is valid ({} tasks, no cycles detected)",
                        dag.len()
                    ));

                    if args.verbose {
                        println!();
                        println!("Execution order:");
                        for (i, task_id) in order.iter().enumerate() {
                            if let Some(task) = dag.get_task(*task_id) {
                                println!("  {}. {}", i + 1, task.name);
                            }
                        }

                        // Check for independent tasks (parallel opportunities)
                        let ready = dag.get_ready_tasks();
                        if ready.len() > 1 {
                            println!();
                            println!("Parallelization: {} tasks can run immediately", ready.len());
                            for task in ready {
                                println!("  - {}", task.name);
                            }
                        }
                    }
                }
                Err(e) => {
                    print_error(&format!("✗ DAG validation failed: {}", e));
                    bail!("Validation failed");
                }
            }
        }
        Err(e) => {
            print_error(&format!("✗ DAG creation failed: {}", e));
            bail!("Validation failed");
        }
    }

    Ok(())
}

/// Visualize DAG structure.
pub async fn run_graph(args: DagGraphArgs) -> Result<()> {
    let spec = load_spec(&args.file)?;
    let specs = convert_specs(&spec);

    let dag = DagHydrator::new()
        .hydrate_from_specs(&specs)
        .map_err(|e| anyhow::anyhow!("Failed to create DAG: {}", e))?;

    match args.output.as_str() {
        "dot" => print_dot_graph(&dag, &spec),
        "mermaid" => print_mermaid_graph(&dag, &spec),
        _ => print_ascii_graph(&dag, &spec),
    }

    Ok(())
}

/// Delete a DAG.
pub async fn run_delete(args: DagDeleteArgs) -> Result<()> {
    let store_path = get_dag_store_path()?;
    let store = DagStore::new(&store_path);

    if !store.exists(&args.id) {
        bail!("DAG '{}' not found", args.id);
    }

    if !args.yes {
        print!("Delete DAG '{}'? [y/N] ", args.id);
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            print_info("Cancelled");
            return Ok(());
        }
    }

    store.delete(&args.id).await?;
    print_success(&format!("✓ Deleted DAG '{}'", args.id));

    Ok(())
}

/// Resume a partially executed DAG.
pub async fn run_resume(args: DagResumeArgs) -> Result<()> {
    let store_path = get_dag_store_path()?;
    let store = DagStore::new(&store_path);

    let dag = store.load(&args.id).await.map_err(|e| match e {
        cortex_agents::task::PersistenceError::NotFound(_) => {
            anyhow::anyhow!("DAG '{}' not found", args.id)
        }
        e => anyhow::anyhow!("Failed to load DAG: {}", e),
    })?;

    if dag.is_complete() {
        print_info("DAG has already completed");
        return Ok(());
    }

    let counts = dag.status_counts();
    let pending = counts.get(&TaskStatus::Pending).unwrap_or(&0)
        + counts.get(&TaskStatus::Ready).unwrap_or(&0);
    let completed = counts.get(&TaskStatus::Completed).unwrap_or(&0);

    print_info(&format!(
        "Resuming DAG '{}' ({} completed, {} remaining)",
        args.id, completed, pending
    ));
    println!();

    let scheduler = DagScheduler::new(
        dag,
        args.max_concurrent,
        args.timeout,
        args.failure_mode,
        true,
        false,
    );

    let stats = scheduler.execute().await?;

    // Save updated state
    let dag = scheduler.dag.read().await;
    store.save(&args.id, &dag).await?;

    println!();
    print_execution_summary(&stats, args.format);

    if stats.failed_tasks > 0 {
        bail!(
            "{} task(s) failed, {} skipped",
            stats.failed_tasks,
            stats.skipped_tasks
        );
    }

    Ok(())
}
