//! File system watcher for hot reloading skills.
//!
//! Monitors skill directories for changes and triggers reloads.

use std::path::PathBuf;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::error::SkillResult;

/// Events emitted by the skill watcher.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// A skill file was created.
    Created(PathBuf),
    /// A skill file was modified.
    Modified(PathBuf),
    /// A skill file was deleted.
    Deleted(PathBuf),
    /// Multiple changes detected, full reload recommended.
    ReloadAll,
    /// Watcher error occurred.
    Error(String),
}

/// File system watcher for skill directories.
pub struct SkillWatcher {
    /// Directories being watched.
    watched_dirs: Vec<PathBuf>,
    /// The underlying watcher.
    watcher: Option<RecommendedWatcher>,
    /// Channel for sending events.
    event_tx: mpsc::Sender<WatchEvent>,
    /// Whether watching is active.
    active: bool,
}

impl SkillWatcher {
    /// Creates a new skill watcher.
    ///
    /// # Arguments
    ///
    /// * `event_tx` - Channel to send watch events
    pub fn new(event_tx: mpsc::Sender<WatchEvent>) -> Self {
        Self {
            watched_dirs: Vec::new(),
            watcher: None,
            event_tx,
            active: false,
        }
    }

    /// Starts watching the specified directories.
    ///
    /// # Arguments
    ///
    /// * `dirs` - Directories to watch for skill changes
    pub fn start(&mut self, dirs: Vec<PathBuf>) -> SkillResult<()> {
        if self.active {
            warn!("Watcher already active, restarting...");
            self.stop();
        }

        self.watched_dirs = dirs.clone();

        let event_tx = self.event_tx.clone();

        // Create a channel for notify events
        let (notify_tx, notify_rx) = std::sync::mpsc::channel();

        // Configure watcher with debouncing
        let config = Config::default()
            .with_poll_interval(Duration::from_secs(2))
            .with_compare_contents(false);

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                if let Err(e) = notify_tx.send(result) {
                    error!("Failed to send notify event: {}", e);
                }
            },
            config,
        )?;

        // Watch each directory
        for dir in &dirs {
            if dir.exists() {
                debug!("Watching directory: {:?}", dir);
                if let Err(e) = watcher.watch(dir, RecursiveMode::Recursive) {
                    warn!("Failed to watch directory {:?}: {}", dir, e);
                }
            } else {
                debug!("Directory does not exist, skipping: {:?}", dir);
            }
        }

        self.watcher = Some(watcher);
        self.active = true;

        // Spawn handler task
        let event_tx_clone = event_tx;
        tokio::spawn(async move {
            Self::handle_events(notify_rx, event_tx_clone).await;
        });

        info!("Skill watcher started for {} directories", dirs.len());
        Ok(())
    }

    /// Handles incoming file system events.
    async fn handle_events(
        rx: std::sync::mpsc::Receiver<Result<Event, notify::Error>>,
        event_tx: mpsc::Sender<WatchEvent>,
    ) {
        // Debounce timer - collect events and process in batches
        let mut pending_paths: Vec<PathBuf> = Vec::new();
        let debounce_duration = Duration::from_millis(500);

        loop {
            match rx.recv_timeout(debounce_duration) {
                Ok(Ok(event)) => {
                    // Collect paths that are skill-related
                    for path in event.paths {
                        if Self::is_skill_file(&path) && !pending_paths.contains(&path) {
                            pending_paths.push(path);
                        }
                    }
                }
                Ok(Err(e)) => {
                    error!("Watch error: {}", e);
                    let _ = event_tx.send(WatchEvent::Error(e.to_string())).await;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Process accumulated events
                    if !pending_paths.is_empty() {
                        if pending_paths.len() > 5 {
                            // Many changes, recommend full reload
                            debug!("Many changes detected, triggering full reload");
                            let _ = event_tx.send(WatchEvent::ReloadAll).await;
                        } else {
                            // Individual changes
                            for path in &pending_paths {
                                let event = if path.exists() {
                                    WatchEvent::Modified(path.clone())
                                } else {
                                    WatchEvent::Deleted(path.clone())
                                };
                                let _ = event_tx.send(event).await;
                            }
                        }
                        pending_paths.clear();
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    debug!("Watcher channel disconnected");
                    break;
                }
            }
        }
    }

    /// Checks if a path is a skill-related file.
    fn is_skill_file(path: &PathBuf) -> bool {
        let filename = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
        filename == "SKILL.toml" || filename == "skill.md"
    }

    /// Stops the watcher.
    pub fn stop(&mut self) {
        if let Some(mut watcher) = self.watcher.take() {
            for dir in &self.watched_dirs {
                let _ = watcher.unwatch(dir);
            }
        }
        self.active = false;
        info!("Skill watcher stopped");
    }

    /// Returns true if the watcher is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns the directories being watched.
    pub fn watched_dirs(&self) -> &[PathBuf] {
        &self.watched_dirs
    }

    /// Adds a new directory to watch.
    pub fn add_dir(&mut self, dir: PathBuf) -> SkillResult<()> {
        if !self.watched_dirs.contains(&dir)
            && let Some(ref mut watcher) = self.watcher
            && dir.exists()
        {
            watcher.watch(&dir, RecursiveMode::Recursive)?;
            self.watched_dirs.push(dir);
        }
        Ok(())
    }

    /// Removes a directory from watching.
    pub fn remove_dir(&mut self, dir: &PathBuf) -> SkillResult<()> {
        if let Some(ref mut watcher) = self.watcher {
            let _ = watcher.unwatch(dir);
        }
        self.watched_dirs.retain(|d| d != dir);
        Ok(())
    }
}

impl Drop for SkillWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[tokio::test]
    async fn test_watcher_creation() {
        let (tx, _rx) = mpsc::channel(10);
        let watcher = SkillWatcher::new(tx);
        assert!(!watcher.is_active());
    }

    #[tokio::test]
    async fn test_is_skill_file() {
        assert!(SkillWatcher::is_skill_file(&PathBuf::from(
            "/test/SKILL.toml"
        )));
        assert!(SkillWatcher::is_skill_file(&PathBuf::from(
            "/test/skill.md"
        )));
        assert!(!SkillWatcher::is_skill_file(&PathBuf::from(
            "/test/other.txt"
        )));
        assert!(!SkillWatcher::is_skill_file(&PathBuf::from(
            "/test/README.md"
        )));
    }

    #[tokio::test]
    async fn test_watcher_start_stop() {
        let temp = TempDir::new().unwrap();
        let (tx, _rx) = mpsc::channel(10);
        let mut watcher = SkillWatcher::new(tx);

        // Start watching
        watcher.start(vec![temp.path().to_path_buf()]).unwrap();
        assert!(watcher.is_active());

        // Stop watching
        watcher.stop();
        assert!(!watcher.is_active());
    }

    #[tokio::test]
    async fn test_watcher_add_remove_dir() {
        let temp1 = TempDir::new().unwrap();
        let temp2 = TempDir::new().unwrap();
        let (tx, _rx) = mpsc::channel(10);
        let mut watcher = SkillWatcher::new(tx);

        watcher.start(vec![temp1.path().to_path_buf()]).unwrap();
        assert_eq!(watcher.watched_dirs().len(), 1);

        watcher.add_dir(temp2.path().to_path_buf()).unwrap();
        assert_eq!(watcher.watched_dirs().len(), 2);

        watcher.remove_dir(&temp1.path().to_path_buf()).unwrap();
        assert_eq!(watcher.watched_dirs().len(), 1);
    }

    #[tokio::test]
    async fn test_watcher_nonexistent_dir() {
        let (tx, _rx) = mpsc::channel(10);
        let mut watcher = SkillWatcher::new(tx);

        // Should not fail for nonexistent directories
        let result = watcher.start(vec![PathBuf::from("/nonexistent/path")]);
        assert!(result.is_ok());
    }
}
