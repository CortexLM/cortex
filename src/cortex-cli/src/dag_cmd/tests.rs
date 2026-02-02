//! Tests for DAG command functionality.

use super::helpers::convert_specs;
use super::types::{DagSpecInput, TaskSpecInput};
use cortex_agents::task::{DagHydrator, Task, TaskId, TaskSpec};
use std::collections::HashMap;

use super::executor::TaskExecutor;

#[test]
fn test_load_yaml_spec() {
    let yaml = r#"
name: test-dag
description: Test DAG
tasks:
  - name: setup
    description: Setup environment
  - name: build
    description: Build project
    depends_on:
      - setup
  - name: test
    description: Run tests
    depends_on:
      - build
"#;

    let spec: DagSpecInput = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(spec.name, Some("test-dag".to_string()));
    assert_eq!(spec.tasks.len(), 3);
    assert_eq!(spec.tasks[1].depends_on, vec!["setup"]);
}

#[test]
fn test_convert_specs() {
    let input = DagSpecInput {
        name: Some("test".to_string()),
        description: None,
        tasks: vec![
            TaskSpecInput {
                name: "a".to_string(),
                description: "Task A".to_string(),
                command: Some("echo A".to_string()),
                depends_on: vec![],
                affected_files: vec![],
                priority: 10,
                estimated_duration: None,
                metadata: HashMap::new(),
            },
            TaskSpecInput {
                name: "b".to_string(),
                description: "Task B".to_string(),
                command: None,
                depends_on: vec!["a".to_string()],
                affected_files: vec!["file.txt".to_string()],
                priority: 5,
                estimated_duration: Some(60),
                metadata: HashMap::new(),
            },
        ],
    };

    let specs = convert_specs(&input);
    assert_eq!(specs.len(), 2);
    assert_eq!(specs[0].priority, 10);
    assert_eq!(specs[1].depends_on, vec!["a"]);
}

#[test]
fn test_dag_creation_with_cycle_detection() {
    let specs = vec![
        TaskSpec::new("a", "A").depends_on("c"),
        TaskSpec::new("b", "B").depends_on("a"),
        TaskSpec::new("c", "C").depends_on("b"),
    ];

    let result = DagHydrator::new().hydrate_from_specs(&specs);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_task_executor() {
    let executor = TaskExecutor::new(30, false);
    let task =
        Task::new("test", "Test task").with_metadata("command", serde_json::json!("echo hello"));

    // Note: This test requires a running shell
    let mut task_with_id = task;
    task_with_id.id = Some(TaskId::new(1));

    let result = executor.execute(&task_with_id).await;
    assert_eq!(result.task_id, TaskId::new(1));
    // Status depends on whether shell is available
}
