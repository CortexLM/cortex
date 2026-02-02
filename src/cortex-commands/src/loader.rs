//! Command loader for reading commands from filesystem.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::fs;
use tracing::{debug, warn};

use crate::command::{Command, CommandError};

/// Errors that can occur when loading commands.
#[derive(Debug, Error)]
pub enum LoaderError {
    /// IO error reading files.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Command parsing error.
    #[error("Command error: {0}")]
    Command(#[from] CommandError),

    /// Invalid path.
    #[error("Invalid path: {0}")]
    InvalidPath(String),
}

/// Loader for custom commands from filesystem.
#[derive(Debug, Clone)]
pub struct CommandLoader {
    /// Search directories in order of priority.
    search_dirs: Vec<PathBuf>,
}

impl Default for CommandLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandLoader {
    /// Create a new loader with default search paths.
    ///
    /// Default paths:
    /// 1. `.cortex/command/` (project-local)
    /// 2. `~/.config/cortex/command/` (global)
    pub fn new() -> Self {
        let mut search_dirs = Vec::new();

        // Project-local commands
        search_dirs.push(PathBuf::from(".cortex/command"));

        // Global commands
        if let Some(config_dir) = dirs::config_dir() {
            search_dirs.push(config_dir.join("Cortex").join("command"));
        }

        Self { search_dirs }
    }

    /// Create a loader with custom search directories.
    pub fn with_dirs(dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            search_dirs: dirs.into_iter().collect(),
        }
    }

    /// Add a search directory.
    pub fn add_dir(&mut self, dir: PathBuf) {
        if !self.search_dirs.contains(&dir) {
            self.search_dirs.push(dir);
        }
    }

    /// Insert a search directory at the beginning (highest priority).
    pub fn prepend_dir(&mut self, dir: PathBuf) {
        if !self.search_dirs.contains(&dir) {
            self.search_dirs.insert(0, dir);
        }
    }

    /// Get the search directories.
    pub fn search_dirs(&self) -> &[PathBuf] {
        &self.search_dirs
    }

    /// Load all commands from all search directories.
    ///
    /// Commands from earlier directories take precedence over later ones.
    pub async fn load_all(&self) -> Result<Vec<Command>, LoaderError> {
        let mut commands = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for dir in &self.search_dirs {
            match self.load_from_directory(dir).await {
                Ok(dir_commands) => {
                    for cmd in dir_commands {
                        if !seen_names.contains(&cmd.name) {
                            seen_names.insert(cmd.name.clone());
                            commands.push(cmd);
                        } else {
                            debug!(
                                "Skipping duplicate command '{}' from {:?}",
                                cmd.name, cmd.source_path
                            );
                        }
                    }
                }
                Err(LoaderError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Directory doesn't exist, skip silently
                    debug!("Command directory {:?} does not exist, skipping", dir);
                }
                Err(e) => {
                    warn!("Error loading commands from {:?}: {}", dir, e);
                }
            }
        }

        Ok(commands)
    }

    /// Load commands from a specific directory.
    pub async fn load_from_directory(&self, dir: &Path) -> Result<Vec<Command>, LoaderError> {
        let mut commands = Vec::new();

        let mut entries = fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Only process .md files
            if path.extension().is_some_and(|ext| ext == "md") {
                match self.load_from_file(&path).await {
                    Ok(cmd) => {
                        debug!("Loaded command '{}' from {:?}", cmd.name, path);
                        commands.push(cmd);
                    }
                    Err(e) => {
                        warn!("Failed to load command from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(commands)
    }

    /// Load a single command from a file.
    pub async fn load_from_file(&self, path: &Path) -> Result<Command, LoaderError> {
        // Extract command name from filename
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| LoaderError::InvalidPath(format!("Invalid filename: {:?}", path)))?
            .to_string();

        // Read file content
        let content = fs::read_to_string(path).await?;

        // Parse the command
        let command = Command::parse(name, &content, path.to_path_buf())?;

        Ok(command)
    }

    /// Check if a command file exists.
    pub async fn exists(&self, name: &str) -> bool {
        for dir in &self.search_dirs {
            let path = dir.join(format!("{name}.md"));
            if fs::try_exists(&path).await.unwrap_or(false) {
                return true;
            }
        }
        false
    }

    /// Get the path where a command would be saved.
    ///
    /// By default, uses the first search directory (project-local).
    pub fn command_path(&self, name: &str) -> Option<PathBuf> {
        self.search_dirs
            .first()
            .map(|dir| dir.join(format!("{name}.md")))
    }

    /// Get the path to the global commands directory.
    pub fn global_command_dir(&self) -> Option<&PathBuf> {
        self.search_dirs.get(1)
    }

    /// Get the path to the project-local commands directory.
    pub fn project_command_dir(&self) -> Option<&PathBuf> {
        self.search_dirs.first()
    }
}

/// Synchronous version of the loader for contexts where async is not available.
pub mod sync {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::LoaderError;
    use crate::command::Command;

    /// Load all commands from a directory synchronously.
    pub fn load_from_directory(dir: &Path) -> Result<Vec<Command>, LoaderError> {
        let mut commands = Vec::new();

        if !dir.exists() {
            return Ok(commands);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "md") {
                match load_from_file(&path) {
                    Ok(cmd) => commands.push(cmd),
                    Err(e) => {
                        tracing::warn!("Failed to load command from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(commands)
    }

    /// Load a single command from a file synchronously.
    pub fn load_from_file(path: &Path) -> Result<Command, LoaderError> {
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| LoaderError::InvalidPath(format!("Invalid filename: {:?}", path)))?
            .to_string();

        let content = fs::read_to_string(path)?;
        let command = Command::parse(name, &content, path.to_path_buf())?;

        Ok(command)
    }

    /// Get default search directories.
    pub fn default_search_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Project-local
        dirs.push(PathBuf::from(".cortex/command"));

        // Global
        if let Some(config_dir) = dirs::config_dir() {
            dirs.push(config_dir.join("Cortex").join("command"));
        }

        dirs
    }

    /// Load all commands from default directories.
    pub fn load_all() -> Vec<Command> {
        let mut commands = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for dir in default_search_dirs() {
            if let Ok(dir_commands) = load_from_directory(&dir) {
                for cmd in dir_commands {
                    if !seen.contains(&cmd.name) {
                        seen.insert(cmd.name.clone());
                        commands.push(cmd);
                    }
                }
            }
        }

        commands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_from_file() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test-cmd.md");

        std::fs::write(
            &file_path,
            r#"---
description: "A test command"
---

Hello $ARGUMENTS!"#,
        )
        .unwrap();

        let loader = CommandLoader::new();
        let cmd = loader.load_from_file(&file_path).await.unwrap();

        assert_eq!(cmd.name, "test-cmd");
        assert_eq!(cmd.description(), "A test command");
        assert!(cmd.expects_arguments());
    }

    #[tokio::test]
    async fn test_load_from_directory() {
        let temp = TempDir::new().unwrap();

        // Create some command files
        std::fs::write(
            temp.path().join("cmd1.md"),
            "---\ndescription: Cmd 1\n---\nTemplate 1",
        )
        .unwrap();

        std::fs::write(
            temp.path().join("cmd2.md"),
            "---\ndescription: Cmd 2\n---\nTemplate 2",
        )
        .unwrap();

        // Create a non-md file (should be ignored)
        std::fs::write(temp.path().join("not-a-command.txt"), "ignored").unwrap();

        let loader = CommandLoader::new();
        let commands = loader.load_from_directory(temp.path()).await.unwrap();

        assert_eq!(commands.len(), 2);
    }

    #[tokio::test]
    async fn test_command_precedence() {
        let temp1 = TempDir::new().unwrap();
        let temp2 = TempDir::new().unwrap();

        // Create same command in both directories with different content
        std::fs::write(
            temp1.path().join("dupe.md"),
            "---\ndescription: First\n---\nFirst template",
        )
        .unwrap();

        std::fs::write(
            temp2.path().join("dupe.md"),
            "---\ndescription: Second\n---\nSecond template",
        )
        .unwrap();

        let loader =
            CommandLoader::with_dirs([temp1.path().to_path_buf(), temp2.path().to_path_buf()]);
        let commands = loader.load_all().await.unwrap();

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].description(), "First");
    }

    #[test]
    fn test_sync_load() {
        let temp = TempDir::new().unwrap();

        std::fs::write(
            temp.path().join("sync-cmd.md"),
            "---\ndescription: Sync\n---\nSync template",
        )
        .unwrap();

        let commands = sync::load_from_directory(temp.path()).unwrap();

        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].name, "sync-cmd");
    }
}
