//! AGENTS.md Hierarchy Support.
//!
//! This module provides support for loading multiple AGENTS.md files from
//! different scopes (global, project, local) and combining them into a
//! unified context for AI agents.
//!
//! # Hierarchy Order (from general to specific)
//!
//! 1. **Global** - `~/.config/cortex/AGENTS.md` - User-wide settings
//! 2. **Project** - `<project>/AGENTS.md` - Project root instructions
//! 3. **Local** - `<project>/.cortex/AGENTS.md` - Project-specific overrides
//!
//! # Supported Filenames
//!
//! - `AGENTS.md` - Primary format
//! - `CLAUDE.md` - Claude Code compatibility
//! - `.cortex/AGENTS.md` - Local overrides
//! - `.claude/CLAUDE.md` - Claude Code local format

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Errors that can occur when loading the agents hierarchy.
#[derive(Debug, Error)]
pub enum HierarchyError {
    /// Failed to read file.
    #[error("Failed to read agents file '{path}': {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to get config directory.
    #[error("Could not determine config directory")]
    NoConfigDir,
}

/// The scope of an agents file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentsScope {
    /// Global user configuration (~/.config/cortex/AGENTS.md).
    Global,
    /// Project root (AGENTS.md or CLAUDE.md in project root).
    Project,
    /// Local project overrides (.cortex/AGENTS.md or .claude/CLAUDE.md).
    Local,
}

impl AgentsScope {
    /// Get the display name for this scope.
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::Project => "Project",
            Self::Local => "Local",
        }
    }

    /// Get the priority (higher = more specific).
    pub fn priority(&self) -> u8 {
        match self {
            Self::Global => 0,
            Self::Project => 1,
            Self::Local => 2,
        }
    }
}

impl std::fmt::Display for AgentsScope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// An AGENTS.md file in the hierarchy.
#[derive(Debug, Clone)]
pub struct AgentsFile {
    /// Path to the file.
    pub path: PathBuf,
    /// The scope of this file.
    pub scope: AgentsScope,
    /// The file content.
    pub content: String,
    /// Optional source filename (e.g., "AGENTS.md" vs "CLAUDE.md").
    pub filename: String,
}

impl AgentsFile {
    /// Create a new agents file entry.
    pub fn new(path: PathBuf, scope: AgentsScope, content: String) -> Self {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("AGENTS.md")
            .to_string();

        Self {
            path,
            scope,
            content,
            filename,
        }
    }

    /// Get the file size in bytes.
    pub fn size(&self) -> usize {
        self.content.len()
    }

    /// Get the line count.
    pub fn line_count(&self) -> usize {
        self.content.lines().count()
    }
}

/// The complete agents hierarchy for a project.
#[derive(Debug, Clone, Default)]
pub struct AgentsHierarchy {
    /// All loaded agents files, ordered from general to specific.
    files: Vec<AgentsFile>,
}

impl AgentsHierarchy {
    /// Create a new empty hierarchy.
    pub fn new() -> Self {
        Self { files: Vec::new() }
    }

    /// Load all applicable AGENTS.md files for the given project root.
    ///
    /// This loads files from:
    /// - Global config directory
    /// - Project root
    /// - Local .cortex/ or .claude/ directories
    pub fn load(project_root: &Path) -> Result<Self, HierarchyError> {
        let mut files = Vec::new();

        // Load global agents file
        if let Some(global_file) = Self::load_global()? {
            files.push(global_file);
        }

        // Load project-level files
        files.extend(Self::load_project(project_root)?);

        // Load local overrides
        files.extend(Self::load_local(project_root)?);

        Ok(Self { files })
    }

    /// Load all applicable AGENTS.md files asynchronously.
    pub async fn load_async(project_root: &Path) -> Result<Self, HierarchyError> {
        // For now, delegate to sync version
        // In production, this would use tokio::fs
        Self::load(project_root)
    }

    /// Load the global agents file if it exists.
    fn load_global() -> Result<Option<AgentsFile>, HierarchyError> {
        let config_dir = dirs::config_dir().ok_or(HierarchyError::NoConfigDir)?;

        // Try cortex config first
        let cortex_path = config_dir.join("cortex/AGENTS.md");
        if let Some(file) = Self::try_load_file(&cortex_path, AgentsScope::Global)? {
            return Ok(Some(file));
        }

        // Try claude config as fallback
        let claude_path = config_dir.join("claude/CLAUDE.md");
        if let Some(file) = Self::try_load_file(&claude_path, AgentsScope::Global)? {
            return Ok(Some(file));
        }

        Ok(None)
    }

    /// Load project-level agents files.
    fn load_project(project_root: &Path) -> Result<Vec<AgentsFile>, HierarchyError> {
        let mut files = Vec::new();

        // Check for AGENTS.md
        let agents_path = project_root.join("AGENTS.md");
        if let Some(file) = Self::try_load_file(&agents_path, AgentsScope::Project)? {
            files.push(file);
        }

        // Check for CLAUDE.md (if no AGENTS.md found)
        if files.is_empty() {
            let claude_path = project_root.join("CLAUDE.md");
            if let Some(file) = Self::try_load_file(&claude_path, AgentsScope::Project)? {
                files.push(file);
            }
        }

        Ok(files)
    }

    /// Load local override files.
    fn load_local(project_root: &Path) -> Result<Vec<AgentsFile>, HierarchyError> {
        let mut files = Vec::new();

        // Check .cortex/AGENTS.md
        let cortex_local = project_root.join(".cortex/AGENTS.md");
        if let Some(file) = Self::try_load_file(&cortex_local, AgentsScope::Local)? {
            files.push(file);
        }

        // Check .claude/CLAUDE.md
        let claude_local = project_root.join(".claude/CLAUDE.md");
        if let Some(file) = Self::try_load_file(&claude_local, AgentsScope::Local)? {
            files.push(file);
        }

        Ok(files)
    }

    /// Try to load a file if it exists.
    fn try_load_file(
        path: &Path,
        scope: AgentsScope,
    ) -> Result<Option<AgentsFile>, HierarchyError> {
        if !path.exists() {
            return Ok(None);
        }

        let content = std::fs::read_to_string(path).map_err(|source| HierarchyError::ReadFile {
            path: path.to_path_buf(),
            source,
        })?;

        Ok(Some(AgentsFile::new(path.to_path_buf(), scope, content)))
    }

    /// Get all loaded files.
    pub fn files(&self) -> &[AgentsFile] {
        &self.files
    }

    /// Get files by scope.
    pub fn files_by_scope(&self, scope: AgentsScope) -> Vec<&AgentsFile> {
        self.files.iter().filter(|f| f.scope == scope).collect()
    }

    /// Check if any files were loaded.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Get the number of loaded files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Combine all files into a single context string.
    ///
    /// Files are combined with scope markers for clarity.
    pub fn combined(&self) -> String {
        if self.files.is_empty() {
            return String::new();
        }

        self.files
            .iter()
            .map(|f| {
                format!(
                    "<!-- {} ({}) -->\n{}",
                    f.path.display(),
                    f.scope.display_name(),
                    f.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }

    /// Combine files using priority (later files override earlier).
    ///
    /// This returns sections in order from general to specific,
    /// which allows more specific configurations to take precedence
    /// in the AI's context window (recency bias).
    pub fn combined_by_priority(&self) -> String {
        let mut sorted_files = self.files.clone();
        sorted_files.sort_by_key(|f| f.scope.priority());

        sorted_files
            .iter()
            .map(|f| {
                format!(
                    "<!-- {} ({}) -->\n{}",
                    f.path.display(),
                    f.scope.display_name(),
                    f.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n")
    }

    /// Get a summary of the loaded hierarchy.
    pub fn summary(&self) -> String {
        if self.is_empty() {
            return "No AGENTS.md files found.".to_string();
        }

        let mut summary = format!("Loaded {} agents file(s):\n", self.files.len());

        for file in &self.files {
            summary.push_str(&format!(
                "  - {} ({}, {} lines)\n",
                file.path.display(),
                file.scope.display_name(),
                file.line_count()
            ));
        }

        summary
    }

    /// Find the most specific file (highest priority).
    pub fn most_specific(&self) -> Option<&AgentsFile> {
        self.files.iter().max_by_key(|f| f.scope.priority())
    }

    /// Find the global file if present.
    pub fn global(&self) -> Option<&AgentsFile> {
        self.files.iter().find(|f| f.scope == AgentsScope::Global)
    }

    /// Find the project file if present.
    pub fn project(&self) -> Option<&AgentsFile> {
        self.files.iter().find(|f| f.scope == AgentsScope::Project)
    }

    /// Find local files.
    pub fn local(&self) -> Vec<&AgentsFile> {
        self.files_by_scope(AgentsScope::Local)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_project_with_agents() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create project AGENTS.md
        fs::write(
            dir.path().join("AGENTS.md"),
            "# Project AGENTS.md\n\nProject-level instructions.",
        )
        .unwrap();

        dir
    }

    fn setup_project_with_hierarchy() -> TempDir {
        let dir = TempDir::new().unwrap();

        // Create project AGENTS.md
        fs::write(
            dir.path().join("AGENTS.md"),
            "# Project AGENTS.md\n\nProject-level instructions.",
        )
        .unwrap();

        // Create local override
        fs::create_dir_all(dir.path().join(".cortex")).unwrap();
        fs::write(
            dir.path().join(".cortex/AGENTS.md"),
            "# Local AGENTS.md\n\nLocal overrides.",
        )
        .unwrap();

        dir
    }

    #[test]
    fn test_load_project_file() {
        let dir = setup_project_with_agents();

        let hierarchy = AgentsHierarchy::load(dir.path()).unwrap();

        assert_eq!(hierarchy.len(), 1);
        assert!(hierarchy.project().is_some());

        let project_file = hierarchy.project().unwrap();
        assert_eq!(project_file.scope, AgentsScope::Project);
        assert!(project_file.content.contains("Project-level instructions"));
    }

    #[test]
    fn test_load_hierarchy() {
        let dir = setup_project_with_hierarchy();

        let hierarchy = AgentsHierarchy::load(dir.path()).unwrap();

        // Should have project and local files (no global in test)
        assert_eq!(hierarchy.len(), 2);

        assert!(hierarchy.project().is_some());
        assert!(!hierarchy.local().is_empty());

        // Local should be most specific
        let most_specific = hierarchy.most_specific().unwrap();
        assert_eq!(most_specific.scope, AgentsScope::Local);
    }

    #[test]
    fn test_combined_output() {
        let dir = setup_project_with_hierarchy();

        let hierarchy = AgentsHierarchy::load(dir.path()).unwrap();
        let combined = hierarchy.combined();

        assert!(combined.contains("Project-level instructions"));
        assert!(combined.contains("Local overrides"));
        assert!(combined.contains("---")); // Separator
    }

    #[test]
    fn test_scope_priority() {
        assert!(AgentsScope::Local.priority() > AgentsScope::Project.priority());
        assert!(AgentsScope::Project.priority() > AgentsScope::Global.priority());
    }

    #[test]
    fn test_empty_hierarchy() {
        let dir = TempDir::new().unwrap();

        let hierarchy = AgentsHierarchy::load(dir.path()).unwrap();

        assert!(hierarchy.is_empty());
        assert_eq!(hierarchy.combined(), "");
    }

    #[test]
    fn test_summary() {
        let dir = setup_project_with_hierarchy();

        let hierarchy = AgentsHierarchy::load(dir.path()).unwrap();
        let summary = hierarchy.summary();

        assert!(summary.contains("2 agents file(s)"));
        assert!(summary.contains("Project"));
        assert!(summary.contains("Local"));
    }

    #[test]
    fn test_claude_fallback() {
        let dir = TempDir::new().unwrap();

        // Create CLAUDE.md instead of AGENTS.md
        fs::write(
            dir.path().join("CLAUDE.md"),
            "# Claude Instructions\n\nClaude-specific config.",
        )
        .unwrap();

        let hierarchy = AgentsHierarchy::load(dir.path()).unwrap();

        assert_eq!(hierarchy.len(), 1);
        let file = hierarchy.project().unwrap();
        assert!(file.filename.contains("CLAUDE"));
    }
}
