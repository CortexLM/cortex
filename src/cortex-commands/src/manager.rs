//! Command manager for TUI integration.
//!
//! Provides a centralized manager for loading, reloading, and managing
//! custom commands with support for multiple sources and hot reloading.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::command::Command;
use crate::loader::{CommandLoader, LoaderError};

/// Errors that can occur in the command manager.
#[derive(Debug, Error)]
pub enum ManagerError {
    /// Loader error.
    #[error("Loader error: {0}")]
    Loader(#[from] LoaderError),

    /// IO error.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Command error.
    #[error("Command error: {0}")]
    Command(#[from] crate::command::CommandError),

    /// Path not found.
    #[error("Path not found: {0}")]
    PathNotFound(PathBuf),

    /// Invalid configuration.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Event sent when commands are reloaded.
#[derive(Debug, Clone)]
pub struct ReloadEvent {
    /// Number of commands loaded.
    pub count: usize,
    /// Paths that were scanned.
    pub paths: Vec<PathBuf>,
}

/// State of the command manager in the TUI.
pub struct CommandManager {
    /// Loaded commands.
    commands: Vec<Command>,
    /// Current project path.
    project_path: Option<PathBuf>,
    /// Channel for reload notifications.
    reload_tx: Option<mpsc::Sender<ReloadEvent>>,
    /// Custom search paths.
    custom_paths: Vec<PathBuf>,
}

impl Default for CommandManager {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandManager {
    /// Create a new command manager.
    pub fn new() -> Self {
        Self {
            commands: Vec::new(),
            project_path: None,
            reload_tx: None,
            custom_paths: Vec::new(),
        }
    }

    /// Create a new command manager with a reload notification channel.
    pub fn with_reload_channel(reload_tx: mpsc::Sender<ReloadEvent>) -> Self {
        Self {
            commands: Vec::new(),
            project_path: None,
            reload_tx: Some(reload_tx),
            custom_paths: Vec::new(),
        }
    }

    /// Set the current project path.
    pub fn set_project_path(&mut self, path: impl Into<PathBuf>) {
        self.project_path = Some(path.into());
    }

    /// Get the current project path.
    pub fn project_path(&self) -> Option<&Path> {
        self.project_path.as_deref()
    }

    /// Add a custom search path.
    pub fn add_search_path(&mut self, path: PathBuf) {
        if !self.custom_paths.contains(&path) {
            self.custom_paths.push(path);
        }
    }

    /// Get all search paths in priority order.
    pub fn search_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // 1. Project-local commands (.cortex/commands/)
        if let Some(ref project) = self.project_path {
            paths.push(project.join(".cortex/commands"));
        }

        // 2. Custom paths
        paths.extend(self.custom_paths.clone());

        // 3. Global commands (~/.config/cortex/commands/)
        if let Some(config_dir) = dirs::config_dir() {
            paths.push(config_dir.join("cortex/commands"));
        }

        paths
    }

    /// Load commands from all search paths.
    pub async fn load_all(&mut self) -> Result<usize, ManagerError> {
        let search_paths = self.search_paths();
        let loader = CommandLoader::with_dirs(search_paths.clone());

        self.commands = loader.load_all().await?;
        let count = self.commands.len();

        info!("Loaded {} commands from {:?}", count, search_paths);

        // Notify reload listeners
        if let Some(ref tx) = self.reload_tx {
            let _ = tx
                .send(ReloadEvent {
                    count,
                    paths: search_paths,
                })
                .await;
        }

        Ok(count)
    }

    /// Reload commands (hotkey R).
    pub async fn reload(&mut self) -> Result<usize, ManagerError> {
        debug!("Reloading commands...");
        self.load_all().await
    }

    /// Import commands from an external directory (e.g., .claude/).
    pub async fn import_from(&mut self, source_dir: &Path) -> Result<usize, ManagerError> {
        if !source_dir.exists() {
            return Err(ManagerError::PathNotFound(source_dir.to_path_buf()));
        }

        let imported = self.scan_and_convert(source_dir).await?;
        let count = imported.len();

        // Determine target directory
        let target = self
            .project_path
            .as_ref()
            .map(|p| p.join(".cortex/commands"))
            .unwrap_or_else(|| {
                dirs::config_dir()
                    .expect("Config directory should exist")
                    .join("cortex/commands")
            });

        // Create target directory if needed
        tokio::fs::create_dir_all(&target).await?;

        // Save imported commands
        for cmd in &imported {
            let dest = target.join(format!("{}.md", cmd.name));
            self.save_command(cmd, &dest).await?;
            debug!("Imported command '{}' to {:?}", cmd.name, dest);
        }

        // Reload to include imported commands
        self.reload().await?;

        info!("Imported {} commands from {:?}", count, source_dir);
        Ok(count)
    }

    /// Scan and convert commands from external format.
    async fn scan_and_convert(&self, dir: &Path) -> Result<Vec<Command>, ManagerError> {
        let mut commands = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Only process .md files
            if path.extension().is_some_and(|ext| ext == "md") {
                match self.convert_external_command(&path).await {
                    Ok(cmd) => {
                        debug!("Converted command '{}' from {:?}", cmd.name, path);
                        commands.push(cmd);
                    }
                    Err(e) => {
                        warn!("Failed to convert command from {:?}: {}", path, e);
                    }
                }
            }
        }

        // Also check subdirectories
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                // Recursively scan subdirectories
                if let Ok(sub_commands) = Box::pin(self.scan_and_convert(&path)).await {
                    commands.extend(sub_commands);
                }
            }
        }

        Ok(commands)
    }

    /// Convert a command from external format (e.g., Claude Code) to Cortex format.
    async fn convert_external_command(&self, path: &Path) -> Result<Command, ManagerError> {
        let content = tokio::fs::read_to_string(path).await?;

        // Extract name from filename
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| ManagerError::InvalidConfig("Invalid filename".to_string()))?
            .to_string();

        // Parse using existing Command parser - it already handles frontmatter
        let cmd = Command::parse(&name, &content, path.to_path_buf())?;

        Ok(cmd)
    }

    /// Save a command to a file.
    async fn save_command(&self, cmd: &Command, path: &Path) -> Result<(), ManagerError> {
        let mut content = String::new();

        // Write frontmatter
        content.push_str("---\n");

        if let Some(ref desc) = cmd.config.description {
            content.push_str(&format!("description: \"{}\"\n", desc));
        }

        if let Some(ref agent) = cmd.config.agent {
            content.push_str(&format!("agent: \"{}\"\n", agent));
        }

        if let Some(ref model) = cmd.config.model {
            content.push_str(&format!("model: \"{}\"\n", model));
        }

        if let Some(subtask) = cmd.config.subtask {
            content.push_str(&format!("subtask: {}\n", subtask));
        }

        if !cmd.config.aliases.is_empty() {
            content.push_str("aliases:\n");
            for alias in &cmd.config.aliases {
                content.push_str(&format!("  - \"{}\"\n", alias));
            }
        }

        content.push_str("---\n\n");
        content.push_str(&cmd.template);

        tokio::fs::write(path, content).await?;
        Ok(())
    }

    /// List all loaded commands.
    pub fn list(&self) -> &[Command] {
        &self.commands
    }

    /// Find a command by name.
    pub fn find(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.name == name)
    }

    /// Find commands by prefix (for autocompletion).
    pub fn find_by_prefix(&self, prefix: &str) -> Vec<&Command> {
        let prefix_lower = prefix.to_lowercase();
        self.commands
            .iter()
            .filter(|c| {
                c.name.to_lowercase().starts_with(&prefix_lower)
                    || c.config
                        .aliases
                        .iter()
                        .any(|a| a.to_lowercase().starts_with(&prefix_lower))
            })
            .collect()
    }

    /// Get the number of loaded commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// Check if no commands are loaded.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Group commands by source directory.
    pub fn by_source(&self) -> std::collections::HashMap<PathBuf, Vec<&Command>> {
        let mut grouped = std::collections::HashMap::new();

        for cmd in &self.commands {
            if let Some(parent) = cmd.source_path.parent() {
                grouped
                    .entry(parent.to_path_buf())
                    .or_insert_with(Vec::new)
                    .push(cmd);
            }
        }

        grouped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_command_manager_new() {
        let manager = CommandManager::new();
        assert!(manager.is_empty());
        assert!(manager.project_path().is_none());
    }

    #[tokio::test]
    async fn test_set_project_path() {
        let mut manager = CommandManager::new();
        manager.set_project_path("/test/project");

        assert_eq!(manager.project_path(), Some(Path::new("/test/project")));
    }

    #[tokio::test]
    async fn test_search_paths() {
        let mut manager = CommandManager::new();
        manager.set_project_path("/test/project");
        manager.add_search_path(PathBuf::from("/custom/path"));

        let paths = manager.search_paths();

        assert!(paths.contains(&PathBuf::from("/test/project/.cortex/commands")));
        assert!(paths.contains(&PathBuf::from("/custom/path")));
    }

    #[tokio::test]
    async fn test_load_from_directory() {
        let temp = TempDir::new().unwrap();

        // Create a command file
        std::fs::write(
            temp.path().join("test-cmd.md"),
            r#"---
description: "Test command"
---

Test template $ARGUMENTS"#,
        )
        .unwrap();

        let mut manager = CommandManager::new();
        manager.add_search_path(temp.path().to_path_buf());

        let count = manager.load_all().await.unwrap();

        assert_eq!(count, 1);
        assert!(!manager.is_empty());

        let cmd = manager.find("test-cmd");
        assert!(cmd.is_some());
        assert_eq!(cmd.unwrap().description(), "Test command");
    }

    #[tokio::test]
    async fn test_find_by_prefix() {
        let temp = TempDir::new().unwrap();

        std::fs::write(
            temp.path().join("build.md"),
            "---\ndescription: Build\n---\nBuild",
        )
        .unwrap();

        std::fs::write(
            temp.path().join("bump.md"),
            "---\ndescription: Bump\n---\nBump",
        )
        .unwrap();

        std::fs::write(
            temp.path().join("test.md"),
            "---\ndescription: Test\n---\nTest",
        )
        .unwrap();

        let mut manager = CommandManager::new();
        manager.add_search_path(temp.path().to_path_buf());
        manager.load_all().await.unwrap();

        let found = manager.find_by_prefix("bu");
        assert_eq!(found.len(), 2);

        let found = manager.find_by_prefix("te");
        assert_eq!(found.len(), 1);
    }

    #[tokio::test]
    async fn test_reload_channel() {
        let (tx, mut rx) = mpsc::channel(1);
        let temp = TempDir::new().unwrap();

        std::fs::write(temp.path().join("cmd.md"), "---\n---\nTemplate").unwrap();

        let mut manager = CommandManager::with_reload_channel(tx);
        manager.add_search_path(temp.path().to_path_buf());
        manager.load_all().await.unwrap();

        // Should receive reload event
        let event = rx.try_recv().unwrap();
        assert_eq!(event.count, 1);
    }
}
