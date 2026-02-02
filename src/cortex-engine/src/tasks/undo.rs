//! Undo task for reverting changes.
//!
//! Handles undoing file modifications, command executions, and other changes.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::client::Message;
use crate::diff::UnifiedDiff;
use crate::error::Result;

use super::{TaskMeta, TaskType};

/// Undo task for reverting changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoTask {
    /// Task metadata.
    pub meta: TaskMeta,
    /// Target to undo (turn ID or snapshot ID).
    pub target: UndoTarget,
    /// Actions to undo.
    pub actions: Vec<UndoAction>,
    /// Actions to redo.
    pub redo_actions: Vec<UndoAction>,
    /// Forward diff for redo.
    pub forward_diff: Option<UnifiedDiff>,
    /// Messages removed during undo (for redo).
    pub messages: Vec<Message>,
    /// Dry run mode.
    pub dry_run: bool,
    /// Confirm before undoing.
    pub confirm: bool,
}

impl UndoTask {
    /// Create a new undo task.
    pub fn new(id: impl Into<String>, target: UndoTarget) -> Self {
        Self {
            meta: TaskMeta::new(id, TaskType::Undo),
            target,
            actions: Vec::new(),
            redo_actions: Vec::new(),
            forward_diff: None,
            messages: Vec::new(),
            dry_run: false,
            confirm: true,
        }
    }

    /// Set dry run mode.
    pub fn dry_run(mut self, dry: bool) -> Self {
        self.dry_run = dry;
        self
    }

    /// Set confirm mode.
    pub fn confirm(mut self, confirm: bool) -> Self {
        self.confirm = confirm;
        self
    }

    /// Add an undo action.
    pub fn add_action(mut self, action: UndoAction) -> Self {
        self.actions.push(action);
        self
    }

    /// Get description of what will be undone.
    pub fn description(&self) -> String {
        match &self.target {
            UndoTarget::LastTurn => "Undo last turn".to_string(),
            UndoTarget::Turn(id) => format!("Undo turn: {id}"),
            UndoTarget::Snapshot(id) => format!("Restore snapshot: {id}"),
            UndoTarget::LastN(n) => format!("Undo last {n} turns"),
        }
    }

    /// Get summary of actions.
    pub fn action_summary(&self) -> Vec<String> {
        self.actions.iter().map(UndoAction::description).collect()
    }
}

/// Undo target.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum UndoTarget {
    /// Undo the last turn.
    LastTurn,
    /// Undo a specific turn.
    Turn(String),
    /// Restore to a specific snapshot.
    Snapshot(String),
    /// Undo last N turns.
    LastN(usize),
}

/// Undo action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UndoAction {
    /// Restore a file to previous state.
    RestoreFile {
        path: PathBuf,
        original_content: Option<Vec<u8>>,
        was_created: bool,
    },
    /// Delete a created file.
    DeleteFile { path: PathBuf },
    /// Restore a deleted file.
    RecreateFile { path: PathBuf, content: Vec<u8> },
    /// Restore a directory.
    RestoreDirectory { path: PathBuf, was_created: bool },
    /// Revert a command execution (if possible).
    RevertCommand {
        original_command: String,
        revert_command: Option<String>,
    },
    /// Restore git state.
    RestoreGit {
        commit: String,
        branch: Option<String>,
    },
}

impl UndoAction {
    /// Get description of the action.
    pub fn description(&self) -> String {
        match self {
            Self::RestoreFile {
                path, was_created, ..
            } => {
                if *was_created {
                    format!("Delete created file: {}", path.display())
                } else {
                    format!("Restore file: {}", path.display())
                }
            }
            Self::DeleteFile { path } => {
                format!("Delete file: {}", path.display())
            }
            Self::RecreateFile { path, .. } => {
                format!("Recreate file: {}", path.display())
            }
            Self::RestoreDirectory { path, was_created } => {
                if *was_created {
                    format!("Remove created directory: {}", path.display())
                } else {
                    format!("Restore directory: {}", path.display())
                }
            }
            Self::RevertCommand {
                original_command,
                revert_command,
            } => {
                if let Some(revert) = revert_command {
                    format!("Revert '{original_command}' with '{revert}'")
                } else {
                    format!("Cannot revert: {original_command}")
                }
            }
            Self::RestoreGit { commit, branch } => {
                if let Some(br) = branch {
                    format!("Restore git to {commit} on {br}")
                } else {
                    format!("Restore git to {commit}")
                }
            }
        }
    }

    /// Check if action is reversible.
    pub fn is_reversible(&self) -> bool {
        match self {
            Self::RevertCommand { revert_command, .. } => revert_command.is_some(),
            _ => true,
        }
    }

    /// Execute the undo action.
    pub fn execute(&self) -> Result<UndoActionResult> {
        match self {
            Self::RestoreFile {
                path,
                original_content,
                was_created,
            } => {
                if *was_created {
                    // Delete the file that was created
                    if path.exists() {
                        std::fs::remove_file(path)?;
                        Ok(UndoActionResult::FileDeleted(path.clone()))
                    } else {
                        Ok(UndoActionResult::NoChange)
                    }
                } else if let Some(content) = original_content {
                    // Restore original content
                    std::fs::write(path, content)?;
                    Ok(UndoActionResult::FileRestored(path.clone()))
                } else {
                    Ok(UndoActionResult::NoChange)
                }
            }
            Self::DeleteFile { path } => {
                if path.exists() {
                    std::fs::remove_file(path)?;
                    Ok(UndoActionResult::FileDeleted(path.clone()))
                } else {
                    Ok(UndoActionResult::NoChange)
                }
            }
            Self::RecreateFile { path, content } => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(path, content)?;
                Ok(UndoActionResult::FileRestored(path.clone()))
            }
            Self::RestoreDirectory { path, was_created } => {
                if *was_created && path.exists() && path.is_dir() {
                    // Only remove if empty
                    if std::fs::read_dir(path)?.next().is_none() {
                        std::fs::remove_dir(path)?;
                        Ok(UndoActionResult::DirectoryRemoved(path.clone()))
                    } else {
                        Ok(UndoActionResult::DirectoryNotEmpty(path.clone()))
                    }
                } else {
                    Ok(UndoActionResult::NoChange)
                }
            }
            Self::RevertCommand { revert_command, .. } => {
                if let Some(cmd) = revert_command {
                    // Parse command safely using shlex to prevent command injection
                    // This avoids passing user input directly to shell
                    let args = shlex::split(cmd).ok_or_else(|| {
                        crate::error::CortexError::ToolExecution {
                            tool: "undo".to_string(),
                            message: format!("Invalid command syntax (unmatched quotes): {cmd}"),
                        }
                    })?;

                    if args.is_empty() {
                        return Ok(UndoActionResult::CommandFailed(
                            "Empty command after parsing".to_string(),
                        ));
                    }

                    let (program, program_args) = args.split_first().expect("args is non-empty");

                    let output = std::process::Command::new(program)
                        .args(program_args)
                        .output()?;

                    if output.status.success() {
                        Ok(UndoActionResult::CommandReverted(cmd.clone()))
                    } else {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        Ok(UndoActionResult::CommandFailed(stderr.to_string()))
                    }
                } else {
                    Ok(UndoActionResult::NotReversible)
                }
            }
            Self::RestoreGit { commit, branch } => {
                // Checkout the commit
                let mut cmd = std::process::Command::new("git");
                cmd.arg("checkout");

                if let Some(br) = branch {
                    cmd.arg(br);
                } else {
                    cmd.arg(commit);
                }

                let output = cmd.output()?;

                if output.status.success() {
                    Ok(UndoActionResult::GitRestored(commit.clone()))
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    Ok(UndoActionResult::CommandFailed(stderr.to_string()))
                }
            }
        }
    }
}

/// Result of an undo action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoActionResult {
    /// File was restored.
    FileRestored(PathBuf),
    /// File was deleted.
    FileDeleted(PathBuf),
    /// Directory was removed.
    DirectoryRemoved(PathBuf),
    /// Directory not empty, couldn't remove.
    DirectoryNotEmpty(PathBuf),
    /// Command was reverted.
    CommandReverted(String),
    /// Command failed.
    CommandFailed(String),
    /// Git state was restored.
    GitRestored(String),
    /// Action not reversible.
    NotReversible,
    /// No change needed.
    NoChange,
}

impl UndoActionResult {
    /// Check if action succeeded.
    pub fn is_success(&self) -> bool {
        !matches!(
            self,
            Self::CommandFailed(_) | Self::NotReversible | Self::DirectoryNotEmpty(_)
        )
    }

    /// Get description.
    pub fn description(&self) -> String {
        match self {
            Self::FileRestored(p) => format!("Restored: {}", p.display()),
            Self::FileDeleted(p) => format!("Deleted: {}", p.display()),
            Self::DirectoryRemoved(p) => format!("Removed directory: {}", p.display()),
            Self::DirectoryNotEmpty(p) => format!("Directory not empty: {}", p.display()),
            Self::CommandReverted(cmd) => format!("Reverted command: {cmd}"),
            Self::CommandFailed(err) => format!("Command failed: {err}"),
            Self::GitRestored(commit) => format!("Git restored to: {commit}"),
            Self::NotReversible => "Action not reversible".to_string(),
            Self::NoChange => "No change needed".to_string(),
        }
    }
}

/// Result of undo operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoResult {
    /// Whether undo was successful.
    pub success: bool,
    /// Actions performed.
    pub actions: Vec<UndoActionResult>,
    /// Errors encountered.
    pub errors: Vec<String>,
    /// Summary message.
    pub summary: String,
}

impl UndoResult {
    /// Create a new undo result.
    pub fn new() -> Self {
        Self {
            success: true,
            actions: Vec::new(),
            errors: Vec::new(),
            summary: String::new(),
        }
    }

    /// Add an action result.
    pub fn add_action(&mut self, result: UndoActionResult) {
        if !result.is_success() {
            self.success = false;
            self.errors.push(result.description());
        }
        self.actions.push(result);
    }

    /// Set summary.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Get counts.
    pub fn counts(&self) -> UndoCounts {
        let mut counts = UndoCounts::default();

        for action in &self.actions {
            match action {
                UndoActionResult::FileRestored(_) => counts.files_restored += 1,
                UndoActionResult::FileDeleted(_) => counts.files_deleted += 1,
                UndoActionResult::DirectoryRemoved(_) => counts.directories_removed += 1,
                UndoActionResult::CommandReverted(_) => counts.commands_reverted += 1,
                UndoActionResult::CommandFailed(_) => counts.failed += 1,
                UndoActionResult::NotReversible => counts.not_reversible += 1,
                _ => {}
            }
        }

        counts
    }
}

impl Default for UndoResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Undo operation counts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UndoCounts {
    /// Files restored.
    pub files_restored: usize,
    /// Files deleted.
    pub files_deleted: usize,
    /// Directories removed.
    pub directories_removed: usize,
    /// Commands reverted.
    pub commands_reverted: usize,
    /// Failed actions.
    pub failed: usize,
    /// Not reversible actions.
    pub not_reversible: usize,
}

/// Undo history tracker.
pub struct UndoHistory {
    /// Stack of undo tasks.
    stack: Vec<UndoTask>,
    /// Maximum history size.
    max_size: usize,
}

impl UndoHistory {
    /// Create a new undo history.
    pub fn new(max_size: usize) -> Self {
        Self {
            stack: Vec::new(),
            max_size,
        }
    }

    /// Push an undo task.
    pub fn push(&mut self, task: UndoTask) {
        self.stack.push(task);

        // Trim if needed
        while self.stack.len() > self.max_size {
            self.stack.remove(0);
        }
    }

    /// Pop the last undo task.
    pub fn pop(&mut self) -> Option<UndoTask> {
        self.stack.pop()
    }

    /// Peek at the last task.
    pub fn peek(&self) -> Option<&UndoTask> {
        self.stack.last()
    }

    /// Get history size.
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Clear history.
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// Get all tasks.
    pub fn all(&self) -> &[UndoTask] {
        &self.stack
    }
}

/// Redo history tracker.
pub struct RedoHistory {
    /// Stack of redo tasks.
    stack: Vec<UndoTask>,
    /// Maximum history size.
    max_size: usize,
}

impl RedoHistory {
    /// Create a new redo history.
    pub fn new(max_size: usize) -> Self {
        Self {
            stack: Vec::new(),
            max_size,
        }
    }

    /// Push a redo task.
    pub fn push(&mut self, task: UndoTask) {
        self.stack.push(task);

        // Trim if needed
        while self.stack.len() > self.max_size {
            self.stack.remove(0);
        }
    }

    /// Pop the last redo task.
    pub fn pop(&mut self) -> Option<UndoTask> {
        self.stack.pop()
    }

    /// Peek at the last task.
    pub fn peek(&self) -> Option<&UndoTask> {
        self.stack.last()
    }

    /// Get history size.
    pub fn len(&self) -> usize {
        self.stack.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Clear history.
    pub fn clear(&mut self) {
        self.stack.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_undo_task() {
        let task = UndoTask::new("undo-1", UndoTarget::LastTurn)
            .dry_run(true)
            .confirm(false);

        assert!(task.dry_run);
        assert!(!task.confirm);
        assert_eq!(task.description(), "Undo last turn");
    }

    #[test]
    fn test_undo_action_restore_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test.txt");
        std::fs::write(&file, "new content").unwrap();

        let action = UndoAction::RestoreFile {
            path: file.clone(),
            original_content: Some(b"original".to_vec()),
            was_created: false,
        };

        let result = action.execute().unwrap();
        assert!(result.is_success());

        let content = std::fs::read_to_string(&file).unwrap();
        assert_eq!(content, "original");
    }

    #[test]
    fn test_undo_action_delete_created() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("created.txt");
        std::fs::write(&file, "content").unwrap();

        let action = UndoAction::RestoreFile {
            path: file.clone(),
            original_content: None,
            was_created: true,
        };

        let result = action.execute().unwrap();
        assert!(result.is_success());
        assert!(!file.exists());
    }

    #[test]
    fn test_undo_history() {
        let mut history = UndoHistory::new(5);

        for i in 0..10 {
            history.push(UndoTask::new(format!("undo-{}", i), UndoTarget::LastTurn));
        }

        assert_eq!(history.len(), 5);
        assert_eq!(history.peek().unwrap().meta.id, "undo-9");
    }

    #[test]
    fn test_undo_task_redo_data() {
        let mut task = UndoTask::new("undo-1", UndoTarget::LastTurn);
        let msg = Message::user("hello");
        task.messages.push(msg.clone());

        assert_eq!(task.messages.len(), 1);
        assert!(task.forward_diff.is_none());
    }

    #[test]
    fn test_undo_result() {
        let mut result = UndoResult::new();
        result.add_action(UndoActionResult::FileRestored(PathBuf::from("/test")));
        result.add_action(UndoActionResult::FileDeleted(PathBuf::from("/test2")));

        let counts = result.counts();
        assert_eq!(counts.files_restored, 1);
        assert_eq!(counts.files_deleted, 1);
        assert!(result.success);
    }
}
