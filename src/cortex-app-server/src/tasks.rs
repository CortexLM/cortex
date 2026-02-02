//! Task tracking and progress broadcasting for cortex-app-server.
//!
//! Provides real-time task progress updates via WebSocket for todo lists
//! and long-running operations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

/// Task manager for tracking and broadcasting task progress.
#[derive(Debug)]
pub struct TaskManager {
    /// Tasks by session ID.
    tasks: RwLock<HashMap<String, Vec<Task>>>,
    /// Broadcast channel for task updates.
    broadcast_tx: broadcast::Sender<TaskEvent>,
}

impl TaskManager {
    /// Create a new task manager.
    pub fn new() -> Self {
        let (broadcast_tx, _) = broadcast::channel(1000);
        Self {
            tasks: RwLock::new(HashMap::new()),
            broadcast_tx,
        }
    }

    /// Subscribe to task updates.
    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.broadcast_tx.subscribe()
    }

    /// Get tasks for a session.
    pub async fn get_tasks(&self, session_id: &str) -> Vec<Task> {
        let tasks = self.tasks.read().await;
        tasks.get(session_id).cloned().unwrap_or_default()
    }

    /// Set all tasks for a session (replaces existing).
    pub async fn set_tasks(&self, session_id: &str, tasks: Vec<Task>) {
        let mut all_tasks = self.tasks.write().await;
        all_tasks.insert(session_id.to_string(), tasks.clone());

        // Broadcast the update
        let _ = self.broadcast_tx.send(TaskEvent::TasksUpdated {
            session_id: session_id.to_string(),
            tasks,
        });
    }

    /// Update a specific task.
    pub async fn update_task(&self, session_id: &str, task_id: &str, status: TaskStatus) {
        let mut all_tasks = self.tasks.write().await;
        if let Some(tasks) = all_tasks.get_mut(session_id)
            && let Some(task) = tasks.iter_mut().find(|t| t.id == task_id)
        {
            task.status = status.clone();
            task.updated_at = chrono::Utc::now().timestamp();

            // Broadcast the update
            let _ = self.broadcast_tx.send(TaskEvent::TaskUpdated {
                session_id: session_id.to_string(),
                task: task.clone(),
            });
        }
    }

    /// Add a task to a session.
    pub async fn add_task(&self, session_id: &str, description: String) -> Task {
        let task = Task {
            id: Uuid::new_v4().to_string(),
            description,
            status: TaskStatus::Pending,
            order: 0,
            created_at: chrono::Utc::now().timestamp(),
            updated_at: chrono::Utc::now().timestamp(),
        };

        let mut all_tasks = self.tasks.write().await;
        let tasks = all_tasks
            .entry(session_id.to_string())
            .or_insert_with(Vec::new);

        // Set order based on current count
        let mut new_task = task;
        new_task.order = tasks.len() as i32;
        tasks.push(new_task.clone());

        // Broadcast
        let _ = self.broadcast_tx.send(TaskEvent::TaskAdded {
            session_id: session_id.to_string(),
            task: new_task.clone(),
        });

        new_task
    }

    /// Remove a task.
    pub async fn remove_task(&self, session_id: &str, task_id: &str) -> bool {
        let mut all_tasks = self.tasks.write().await;
        if let Some(tasks) = all_tasks.get_mut(session_id)
            && let Some(pos) = tasks.iter().position(|t| t.id == task_id)
        {
            tasks.remove(pos);

            // Reorder remaining tasks
            for (i, task) in tasks.iter_mut().enumerate() {
                task.order = i as i32;
            }

            // Broadcast
            let _ = self.broadcast_tx.send(TaskEvent::TaskRemoved {
                session_id: session_id.to_string(),
                task_id: task_id.to_string(),
            });

            return true;
        }
        false
    }

    /// Clear all tasks for a session.
    pub async fn clear_tasks(&self, session_id: &str) {
        let mut all_tasks = self.tasks.write().await;
        all_tasks.remove(session_id);

        let _ = self.broadcast_tx.send(TaskEvent::TasksCleared {
            session_id: session_id.to_string(),
        });
    }

    /// Parse tasks from TodoWrite format.
    pub fn parse_todo_format(content: &str) -> Vec<Task> {
        let mut tasks = Vec::new();
        let now = chrono::Utc::now().timestamp();

        for (i, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Parse format: "1. [status] description"
            let (status, description) =
                if let Some(rest) = line.strip_prefix(|c: char| c.is_numeric()) {
                    // Remove the number and dot
                    let rest = rest.trim_start_matches(|c: char| c.is_numeric() || c == '.');
                    let rest = rest.trim();

                    if let Some(desc) = rest.strip_prefix("[completed]") {
                        (TaskStatus::Completed, desc.trim().to_string())
                    } else if let Some(desc) = rest.strip_prefix("[in_progress]") {
                        (TaskStatus::InProgress, desc.trim().to_string())
                    } else if let Some(desc) = rest.strip_prefix("[pending]") {
                        (TaskStatus::Pending, desc.trim().to_string())
                    } else {
                        (TaskStatus::Pending, rest.to_string())
                    }
                } else {
                    // No number prefix
                    if let Some(desc) = line.strip_prefix("[completed]") {
                        (TaskStatus::Completed, desc.trim().to_string())
                    } else if let Some(desc) = line.strip_prefix("[in_progress]") {
                        (TaskStatus::InProgress, desc.trim().to_string())
                    } else if let Some(desc) = line.strip_prefix("[pending]") {
                        (TaskStatus::Pending, desc.trim().to_string())
                    } else {
                        (TaskStatus::Pending, line.to_string())
                    }
                };

            if !description.is_empty() {
                tasks.push(Task {
                    id: Uuid::new_v4().to_string(),
                    description,
                    status,
                    order: i as i32,
                    created_at: now,
                    updated_at: now,
                });
            }
        }

        tasks
    }
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

/// A task in the todo list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique task ID.
    pub id: String,
    /// Task description.
    pub description: String,
    /// Current status.
    pub status: TaskStatus,
    /// Order in the list (0-indexed).
    pub order: i32,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last update timestamp.
    pub updated_at: i64,
}

/// Task status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Task not started.
    Pending,
    /// Task in progress.
    InProgress,
    /// Task completed.
    Completed,
}

/// Task events for broadcasting.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TaskEvent {
    /// All tasks replaced/updated.
    TasksUpdated {
        session_id: String,
        tasks: Vec<Task>,
    },
    /// Single task updated.
    TaskUpdated { session_id: String, task: Task },
    /// Task added.
    TaskAdded { session_id: String, task: Task },
    /// Task removed.
    TaskRemoved { session_id: String, task_id: String },
    /// All tasks cleared.
    TasksCleared { session_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_todo_format() {
        let content = r#"
1. [completed] First task that is done
2. [in_progress] Currently working on this
3. [pending] Not started yet
4. Task without status marker
"#;

        let tasks = TaskManager::parse_todo_format(content);
        assert_eq!(tasks.len(), 4);

        assert_eq!(tasks[0].status, TaskStatus::Completed);
        assert_eq!(tasks[0].description, "First task that is done");

        assert_eq!(tasks[1].status, TaskStatus::InProgress);
        assert_eq!(tasks[1].description, "Currently working on this");

        assert_eq!(tasks[2].status, TaskStatus::Pending);
        assert_eq!(tasks[2].description, "Not started yet");

        assert_eq!(tasks[3].status, TaskStatus::Pending);
        assert_eq!(tasks[3].description, "Task without status marker");
    }

    #[tokio::test]
    async fn test_task_manager() {
        let manager = TaskManager::new();

        // Add tasks
        let task1 = manager.add_task("session-1", "Task 1".to_string()).await;
        let task2 = manager.add_task("session-1", "Task 2".to_string()).await;

        // Get tasks
        let tasks = manager.get_tasks("session-1").await;
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].order, 0);
        assert_eq!(tasks[1].order, 1);

        // Update task
        manager
            .update_task("session-1", &task1.id, TaskStatus::InProgress)
            .await;
        let tasks = manager.get_tasks("session-1").await;
        assert_eq!(tasks[0].status, TaskStatus::InProgress);

        // Remove task
        assert!(manager.remove_task("session-1", &task1.id).await);
        let tasks = manager.get_tasks("session-1").await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, task2.id);

        // Clear tasks
        manager.clear_tasks("session-1").await;
        let tasks = manager.get_tasks("session-1").await;
        assert!(tasks.is_empty());
    }

    #[tokio::test]
    async fn test_set_tasks() {
        let manager = TaskManager::new();

        let tasks = vec![
            Task {
                id: "1".to_string(),
                description: "Task 1".to_string(),
                status: TaskStatus::Completed,
                order: 0,
                created_at: 0,
                updated_at: 0,
            },
            Task {
                id: "2".to_string(),
                description: "Task 2".to_string(),
                status: TaskStatus::Pending,
                order: 1,
                created_at: 0,
                updated_at: 0,
            },
        ];

        manager.set_tasks("session-1", tasks).await;

        let retrieved = manager.get_tasks("session-1").await;
        assert_eq!(retrieved.len(), 2);
        assert_eq!(retrieved[0].id, "1");
        assert_eq!(retrieved[1].id, "2");
    }
}
