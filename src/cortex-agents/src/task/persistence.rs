//! Task DAG persistence for serialization and storage.
//!
//! This module provides functionality to save and load task DAGs,
//! enabling session recovery and task state persistence.
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::task::dag::TaskDag;
//! use cortex_agents::task::persistence::DagStore;
//!
//! let dag = TaskDag::new();
//! // ... add tasks ...
//!
//! // Save to file
//! let store = DagStore::new("/path/to/store");
//! store.save("session-123", &dag).await?;
//!
//! // Load from file
//! let loaded = store.load("session-123").await?;
//! ```

use super::dag::{DagError, Task, TaskDag, TaskId, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use thiserror::Error;

/// Errors for persistence operations.
#[derive(Debug, Error)]
pub enum PersistenceError {
    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization error.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// DAG not found.
    #[error("DAG not found: {0}")]
    NotFound(String),

    /// DAG error.
    #[error("DAG error: {0}")]
    Dag(#[from] DagError),
}

/// Result type for persistence operations.
pub type PersistenceResult<T> = std::result::Result<T, PersistenceError>;

/// Serializable representation of a TaskDag.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedDag {
    /// Version of the serialization format.
    pub version: u32,
    /// All tasks.
    pub tasks: Vec<SerializedTask>,
    /// Dependencies as (task_id, depends_on_id) pairs.
    pub dependencies: Vec<(u64, u64)>,
    /// Metadata.
    pub metadata: HashMap<String, serde_json::Value>,
    /// Creation timestamp.
    pub created_at: chrono::DateTime<chrono::Utc>,
    /// Last modified timestamp.
    pub modified_at: chrono::DateTime<chrono::Utc>,
}

/// Serializable representation of a Task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializedTask {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub status: TaskStatus,
    pub affected_files: Vec<String>,
    pub agent_id: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub priority: i32,
    pub estimated_duration: Option<u64>,
    pub actual_duration: Option<u64>,
    pub metadata: HashMap<String, serde_json::Value>,
}

impl From<&Task> for SerializedTask {
    fn from(task: &Task) -> Self {
        Self {
            id: task.id.map(|id| id.inner()).unwrap_or(0),
            name: task.name.clone(),
            description: task.description.clone(),
            status: task.status,
            affected_files: task.affected_files.clone(),
            agent_id: task.agent_id.clone(),
            result: task.result.clone(),
            error: task.error.clone(),
            priority: task.priority,
            estimated_duration: task.estimated_duration,
            actual_duration: task.actual_duration,
            metadata: task.metadata.clone(),
        }
    }
}

impl SerializedDag {
    /// Current serialization format version.
    pub const CURRENT_VERSION: u32 = 1;

    /// Create a serialized representation of a TaskDag.
    pub fn from_dag(dag: &TaskDag) -> Self {
        let now = chrono::Utc::now();

        let tasks: Vec<SerializedTask> = dag.all_tasks().map(SerializedTask::from).collect();

        let mut dependencies = Vec::new();
        for task in dag.all_tasks() {
            if let Some(id) = task.id {
                if let Some(deps) = dag.get_dependencies(id) {
                    for &dep_id in deps {
                        dependencies.push((id.inner(), dep_id.inner()));
                    }
                }
            }
        }

        Self {
            version: Self::CURRENT_VERSION,
            tasks,
            dependencies,
            metadata: HashMap::new(),
            created_at: now,
            modified_at: now,
        }
    }

    /// Convert back to a TaskDag.
    pub fn to_dag(&self) -> PersistenceResult<TaskDag> {
        let mut dag = TaskDag::new();
        let mut id_map: HashMap<u64, TaskId> = HashMap::new();

        // First pass: add all tasks
        for st in &self.tasks {
            let task = Task {
                id: None,
                name: st.name.clone(),
                description: st.description.clone(),
                status: st.status,
                affected_files: st.affected_files.clone(),
                agent_id: st.agent_id.clone(),
                result: st.result.clone(),
                error: st.error.clone(),
                priority: st.priority,
                estimated_duration: st.estimated_duration,
                actual_duration: st.actual_duration,
                metadata: st.metadata.clone(),
            };
            let new_id = dag.add_task(task);
            id_map.insert(st.id, new_id);
        }

        // Second pass: add dependencies
        for &(task_id, depends_on) in &self.dependencies {
            if let (Some(&new_task_id), Some(&new_dep_id)) =
                (id_map.get(&task_id), id_map.get(&depends_on))
            {
                dag.add_dependency(new_task_id, new_dep_id)?;
            }
        }

        // Restore task statuses (they may have been changed by add_dependency)
        for st in &self.tasks {
            if let Some(&new_id) = id_map.get(&st.id) {
                if let Some(task) = dag.get_task_mut(new_id) {
                    task.status = st.status;
                }
            }
        }

        Ok(dag)
    }

    /// Serialize to JSON string.
    pub fn to_json(&self) -> PersistenceResult<String> {
        serde_json::to_string_pretty(self).map_err(PersistenceError::from)
    }

    /// Deserialize from JSON string.
    pub fn from_json(json: &str) -> PersistenceResult<Self> {
        serde_json::from_str(json).map_err(PersistenceError::from)
    }

    /// Set metadata.
    pub fn with_metadata(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }
}

/// Store for persisting DAGs to the filesystem.
pub struct DagStore {
    /// Base directory for storage.
    base_path: PathBuf,
}

impl DagStore {
    /// Create a new DAG store.
    pub fn new(base_path: impl Into<PathBuf>) -> Self {
        Self {
            base_path: base_path.into(),
        }
    }

    /// Get the file path for a DAG.
    fn dag_path(&self, id: &str) -> PathBuf {
        self.base_path.join(format!("{}.dag.json", id))
    }

    /// Save a DAG to the store.
    pub async fn save(&self, id: &str, dag: &TaskDag) -> PersistenceResult<()> {
        // Ensure directory exists
        tokio::fs::create_dir_all(&self.base_path).await?;

        let serialized = SerializedDag::from_dag(dag);
        let json = serialized.to_json()?;

        let path = self.dag_path(id);
        tokio::fs::write(&path, json).await?;

        tracing::debug!(id = id, path = ?path, "Saved DAG");
        Ok(())
    }

    /// Load a DAG from the store.
    pub async fn load(&self, id: &str) -> PersistenceResult<TaskDag> {
        let path = self.dag_path(id);

        if !path.exists() {
            return Err(PersistenceError::NotFound(id.to_string()));
        }

        let json = tokio::fs::read_to_string(&path).await?;
        let serialized = SerializedDag::from_json(&json)?;
        let dag = serialized.to_dag()?;

        tracing::debug!(id = id, path = ?path, "Loaded DAG");
        Ok(dag)
    }

    /// Check if a DAG exists in the store.
    pub fn exists(&self, id: &str) -> bool {
        self.dag_path(id).exists()
    }

    /// Delete a DAG from the store.
    pub async fn delete(&self, id: &str) -> PersistenceResult<()> {
        let path = self.dag_path(id);

        if path.exists() {
            tokio::fs::remove_file(&path).await?;
            tracing::debug!(id = id, "Deleted DAG");
        }

        Ok(())
    }

    /// List all DAG IDs in the store.
    pub async fn list(&self) -> PersistenceResult<Vec<String>> {
        let mut ids = Vec::new();

        if !self.base_path.exists() {
            return Ok(ids);
        }

        let mut entries = tokio::fs::read_dir(&self.base_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if let Some(id) = name.strip_suffix(".dag.json") {
                    ids.push(id.to_string());
                }
            }
        }

        Ok(ids)
    }
}

/// In-memory store for testing and temporary storage.
#[derive(Debug, Clone, Default)]
pub struct InMemoryDagStore {
    dags: std::sync::Arc<tokio::sync::RwLock<HashMap<String, SerializedDag>>>,
}

impl InMemoryDagStore {
    /// Create a new in-memory store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Save a DAG to the store.
    pub async fn save(&self, id: &str, dag: &TaskDag) -> PersistenceResult<()> {
        let serialized = SerializedDag::from_dag(dag);
        self.dags.write().await.insert(id.to_string(), serialized);
        Ok(())
    }

    /// Load a DAG from the store.
    pub async fn load(&self, id: &str) -> PersistenceResult<TaskDag> {
        let dags = self.dags.read().await;
        let serialized = dags
            .get(id)
            .ok_or_else(|| PersistenceError::NotFound(id.to_string()))?;
        serialized.to_dag()
    }

    /// Check if a DAG exists.
    pub async fn exists(&self, id: &str) -> bool {
        self.dags.read().await.contains_key(id)
    }

    /// Delete a DAG.
    pub async fn delete(&self, id: &str) -> PersistenceResult<()> {
        self.dags.write().await.remove(id);
        Ok(())
    }

    /// List all DAG IDs.
    pub async fn list(&self) -> Vec<String> {
        self.dags.read().await.keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_dag() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("Task 1", "First task").with_priority(5));
        let t2 = dag.add_task(Task::new("Task 2", "Second task"));
        dag.add_dependency(t2, t1).unwrap();

        let serialized = SerializedDag::from_dag(&dag);

        assert_eq!(serialized.version, SerializedDag::CURRENT_VERSION);
        assert_eq!(serialized.tasks.len(), 2);
        assert_eq!(serialized.dependencies.len(), 1);
    }

    #[test]
    fn test_deserialize_dag() {
        let mut dag = TaskDag::new();
        let t1 = dag.add_task(Task::new("Task 1", "First").with_priority(5));
        let t2 = dag.add_task(Task::new("Task 2", "Second"));
        dag.add_dependency(t2, t1).unwrap();

        // Mark t1 as completed
        dag.start_task(t1, None).unwrap();
        dag.complete_task(t1, Some("Done".to_string())).unwrap();

        let serialized = SerializedDag::from_dag(&dag);
        let restored = serialized.to_dag().unwrap();

        assert_eq!(restored.len(), 2);

        // Check that statuses were restored
        let tasks: Vec<_> = restored.all_tasks().collect();
        let completed = tasks.iter().find(|t| t.name == "Task 1").unwrap();
        assert_eq!(completed.status, TaskStatus::Completed);
    }

    #[test]
    fn test_json_roundtrip() {
        let mut dag = TaskDag::new();
        dag.add_task(Task::new("Test", "Test task"));

        let serialized = SerializedDag::from_dag(&dag);
        let json = serialized.to_json().unwrap();
        let restored_serialized = SerializedDag::from_json(&json).unwrap();
        let restored = restored_serialized.to_dag().unwrap();

        assert_eq!(restored.len(), 1);
    }

    #[tokio::test]
    async fn test_in_memory_store() {
        let store = InMemoryDagStore::new();

        let mut dag = TaskDag::new();
        dag.add_task(Task::new("Test", "Test task"));

        // Save
        store.save("test-dag", &dag).await.unwrap();
        assert!(store.exists("test-dag").await);

        // Load
        let loaded = store.load("test-dag").await.unwrap();
        assert_eq!(loaded.len(), 1);

        // List
        let ids = store.list().await;
        assert_eq!(ids, vec!["test-dag"]);

        // Delete
        store.delete("test-dag").await.unwrap();
        assert!(!store.exists("test-dag").await);
    }

    #[test]
    fn test_serialized_task_from_task() {
        let task = Task::new("Test", "Description")
            .with_priority(10)
            .with_affected_files(vec!["file.rs".to_string()])
            .with_metadata("key", serde_json::json!("value"));

        let serialized = SerializedTask::from(&task);

        assert_eq!(serialized.name, "Test");
        assert_eq!(serialized.priority, 10);
        assert_eq!(serialized.affected_files, vec!["file.rs"]);
    }
}
