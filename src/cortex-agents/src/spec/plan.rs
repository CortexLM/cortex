//! Specification plan structures for the spec mode.
//!
//! This module defines the data structures used to represent a specification
//! plan that the agent generates before implementing changes.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A specification plan describing proposed changes.
///
/// In Spec mode, the agent generates a `SpecPlan` before making any changes.
/// The user can then review and approve the plan before the agent proceeds
/// with implementation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecPlan {
    /// Title of the plan
    pub title: String,
    /// Brief summary of what the plan accomplishes
    pub summary: String,
    /// Ordered list of implementation steps
    pub steps: Vec<SpecStep>,
    /// List of all files that will be affected
    pub files_affected: Vec<PathBuf>,
    /// Estimated number of changes (file operations)
    pub estimated_changes: usize,
    /// When the plan was created (ISO 8601 timestamp)
    pub created_at: String,
}

impl SpecPlan {
    /// Create a new empty spec plan with the given title.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            summary: String::new(),
            steps: Vec::new(),
            files_affected: Vec::new(),
            estimated_changes: 0,
            created_at: chrono_now_iso(),
        }
    }

    /// Set the summary for the plan.
    pub fn with_summary(mut self, summary: impl Into<String>) -> Self {
        self.summary = summary.into();
        self
    }

    /// Add a step to the plan.
    ///
    /// This automatically updates the `estimated_changes` count and
    /// `files_affected` list.
    pub fn add_step(&mut self, step: SpecStep) {
        self.estimated_changes += step.file_changes.len();
        for change in &step.file_changes {
            if !self.files_affected.contains(&change.path) {
                self.files_affected.push(change.path.clone());
            }
        }
        self.steps.push(step);
    }

    /// Generate a markdown representation of the plan for display.
    pub fn to_markdown(&self) -> String {
        let mut md = format!("# {}\n\n", self.title);

        if !self.summary.is_empty() {
            md.push_str(&format!("{}\n\n", self.summary));
        }

        if !self.steps.is_empty() {
            md.push_str("## Steps\n\n");
            for (i, step) in self.steps.iter().enumerate() {
                md.push_str(&format!("{}. {}\n", i + 1, step.description));
                for change in &step.file_changes {
                    md.push_str(&format!(
                        "   - {} `{}`\n",
                        change.change_type.verb(),
                        change.path.display()
                    ));
                    if !change.description.is_empty() {
                        md.push_str(&format!("     _{}_\n", change.description));
                    }
                }
                md.push('\n');
            }
        }

        md.push_str(&format!(
            "**Files affected:** {}  \n**Estimated changes:** {}\n",
            self.files_affected.len(),
            self.estimated_changes
        ));

        md
    }

    /// Check if the plan is empty (no steps).
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Get the number of steps in the plan.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }
}

impl Default for SpecPlan {
    fn default() -> Self {
        Self::new("Untitled Plan")
    }
}

/// A single step in the specification plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecStep {
    /// Step identifier (1-indexed)
    pub id: usize,
    /// Human-readable description of the step
    pub description: String,
    /// File changes involved in this step
    pub file_changes: Vec<FileChange>,
    /// IDs of steps that must be completed before this one
    pub dependencies: Vec<usize>,
}

impl SpecStep {
    /// Create a new step with the given ID and description.
    pub fn new(id: usize, description: impl Into<String>) -> Self {
        Self {
            id,
            description: description.into(),
            file_changes: Vec::new(),
            dependencies: Vec::new(),
        }
    }

    /// Add a file change to this step.
    pub fn add_change(&mut self, change: FileChange) {
        self.file_changes.push(change);
    }

    /// Add a dependency to this step.
    pub fn add_dependency(&mut self, step_id: usize) {
        if !self.dependencies.contains(&step_id) {
            self.dependencies.push(step_id);
        }
    }

    /// Builder method to add a file change.
    pub fn with_change(mut self, change: FileChange) -> Self {
        self.add_change(change);
        self
    }

    /// Builder method to add a dependency.
    pub fn with_dependency(mut self, step_id: usize) -> Self {
        self.add_dependency(step_id);
        self
    }
}

/// Description of a file change within a step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// Path to the file being changed
    pub path: PathBuf,
    /// Type of change (create, modify, delete, rename)
    pub change_type: ChangeType,
    /// Human-readable description of the change
    pub description: String,
    /// Optional diff preview for modifications
    pub diff_preview: Option<String>,
}

impl FileChange {
    /// Create a new file change.
    pub fn new(path: impl Into<PathBuf>, change_type: ChangeType) -> Self {
        Self {
            path: path.into(),
            change_type,
            description: String::new(),
            diff_preview: None,
        }
    }

    /// Set the description for this change.
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    /// Set a diff preview for this change.
    pub fn with_diff(mut self, diff: impl Into<String>) -> Self {
        self.diff_preview = Some(diff.into());
        self
    }

    /// Create a "create file" change.
    pub fn create(path: impl Into<PathBuf>) -> Self {
        Self::new(path, ChangeType::Create)
    }

    /// Create a "modify file" change.
    pub fn modify(path: impl Into<PathBuf>) -> Self {
        Self::new(path, ChangeType::Modify)
    }

    /// Create a "delete file" change.
    pub fn delete(path: impl Into<PathBuf>) -> Self {
        Self::new(path, ChangeType::Delete)
    }

    /// Create a "rename file" change.
    pub fn rename(from: impl Into<PathBuf>, to: impl Into<PathBuf>) -> Self {
        Self::new(to, ChangeType::Rename { from: from.into() })
    }
}

/// Type of file change operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    /// Create a new file
    Create,
    /// Modify an existing file
    Modify,
    /// Delete a file
    Delete,
    /// Rename a file (with original path)
    Rename { from: PathBuf },
}

impl ChangeType {
    /// Get the verb form of the change type for display.
    pub fn verb(&self) -> &'static str {
        match self {
            ChangeType::Create => "Create",
            ChangeType::Modify => "Modify",
            ChangeType::Delete => "Delete",
            ChangeType::Rename { .. } => "Rename",
        }
    }

    /// Get an icon for the change type.
    pub fn icon(&self) -> &'static str {
        match self {
            ChangeType::Create => "[+]",
            ChangeType::Modify => "[~]",
            ChangeType::Delete => "[-]",
            ChangeType::Rename { .. } => "[>]",
        }
    }

    /// Check if this is a destructive operation.
    pub fn is_destructive(&self) -> bool {
        matches!(self, ChangeType::Delete | ChangeType::Modify)
    }
}

/// Get current timestamp in ISO 8601 format.
///
/// Uses a simple format without requiring chrono dependency.
fn chrono_now_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple ISO-ish timestamp
    format!("{}000", secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spec_plan_new() {
        let plan = SpecPlan::new("Test Plan");
        assert_eq!(plan.title, "Test Plan");
        assert!(plan.summary.is_empty());
        assert!(plan.steps.is_empty());
        assert!(plan.files_affected.is_empty());
        assert_eq!(plan.estimated_changes, 0);
    }

    #[test]
    fn test_spec_plan_add_step() {
        let mut plan = SpecPlan::new("Add feature");
        plan.summary = "Adding new feature X".to_string();

        let mut step = SpecStep::new(1, "Create component");
        step.add_change(
            FileChange::create("src/component.rs").with_description("New component file"),
        );

        plan.add_step(step);

        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.estimated_changes, 1);
        assert_eq!(plan.files_affected.len(), 1);
    }

    #[test]
    fn test_spec_plan_markdown() {
        let mut plan = SpecPlan::new("Add feature");
        plan.summary = "Adding new feature X".to_string();

        let step = SpecStep::new(1, "Create component")
            .with_change(FileChange::create("src/component.rs").with_description("New component"));

        plan.add_step(step);

        let md = plan.to_markdown();
        assert!(md.contains("# Add feature"));
        assert!(md.contains("Adding new feature X"));
        assert!(md.contains("Create `src/component.rs`"));
        assert!(md.contains("**Files affected:** 1"));
    }

    #[test]
    fn test_change_type_verb() {
        assert_eq!(ChangeType::Create.verb(), "Create");
        assert_eq!(ChangeType::Modify.verb(), "Modify");
        assert_eq!(ChangeType::Delete.verb(), "Delete");
        assert_eq!(
            ChangeType::Rename {
                from: "old.rs".into()
            }
            .verb(),
            "Rename"
        );
    }

    #[test]
    fn test_change_type_destructive() {
        assert!(!ChangeType::Create.is_destructive());
        assert!(ChangeType::Modify.is_destructive());
        assert!(ChangeType::Delete.is_destructive());
        assert!(!ChangeType::Rename { from: "x".into() }.is_destructive());
    }

    #[test]
    fn test_file_change_builders() {
        let create = FileChange::create("test.rs");
        assert!(matches!(create.change_type, ChangeType::Create));

        let modify = FileChange::modify("test.rs");
        assert!(matches!(modify.change_type, ChangeType::Modify));

        let delete = FileChange::delete("test.rs");
        assert!(matches!(delete.change_type, ChangeType::Delete));

        let rename = FileChange::rename("old.rs", "new.rs");
        assert!(matches!(rename.change_type, ChangeType::Rename { .. }));
    }

    #[test]
    fn test_spec_step_dependencies() {
        let step = SpecStep::new(2, "Implement feature")
            .with_dependency(1)
            .with_dependency(1); // Duplicate should be ignored

        assert_eq!(step.dependencies.len(), 1);
        assert!(step.dependencies.contains(&1));
    }
}
