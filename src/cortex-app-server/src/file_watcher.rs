//! File watcher for monitoring /workspace changes and notifying clients.
//!
//! Uses `notify-debouncer-mini` to handle rapid file changes properly,
//! especially on macOS where FSEvents can coalesce rapid saves.

use notify::RecursiveMode;
use notify_debouncer_mini::{DebouncedEventKind, new_debouncer};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;
use tokio::sync::broadcast;
use tracing::{debug, error, info, warn};

/// Debounce timeout for file change events.
/// Shorter timeout (100ms) ensures rapid saves (e.g., from Vim/Neovim) are detected
/// while still coalescing truly duplicate events.
const DEBOUNCE_TIMEOUT_MS: u64 = 100;

/// File change event sent to clients.
#[derive(Debug, Clone, serde::Serialize)]
pub struct FileChangeEvent {
    /// Type of change: "create", "modify", "delete", "rename"
    #[serde(rename = "type")]
    pub change_type: String,
    /// Path that changed
    pub path: String,
    /// Timestamp of the change
    pub timestamp: String,
}

/// File watcher that monitors /workspace and broadcasts changes.
///
/// Uses `notify-debouncer-mini` for reliable detection of rapid file changes,
/// which is particularly important on macOS where FSEvents can miss rapid saves.
pub struct FileWatcher {
    /// Broadcast sender for file change events
    sender: broadcast::Sender<FileChangeEvent>,
    /// Watch handle (kept alive via background task)
    /// Note: The actual debouncer is owned by the spawned thread
    _watch_active: bool,
}

impl FileWatcher {
    /// Create a new file watcher for the given path.
    pub fn new(watch_path: &str) -> Self {
        let (sender, _) = broadcast::channel(100);

        let tx = sender.clone();
        let path = watch_path.to_string();

        // Start the watcher in a background thread
        let watch_active = Self::start_watcher(tx, &path);

        Self {
            sender,
            _watch_active: watch_active,
        }
    }

    fn start_watcher(tx: broadcast::Sender<FileChangeEvent>, watch_path: &str) -> bool {
        let path = watch_path.to_string();

        // Create a channel for the debouncer to send events
        let (event_tx, event_rx) = mpsc::channel();

        // Create the debouncer with a short timeout for rapid change detection
        let debounce_duration = Duration::from_millis(DEBOUNCE_TIMEOUT_MS);
        let mut debouncer = match new_debouncer(debounce_duration, event_tx) {
            Ok(d) => d,
            Err(e) => {
                error!(
                    error = %e,
                    watch_path = %path,
                    "Failed to create file watcher debouncer"
                );
                return false;
            }
        };

        // Start watching the path
        let watch_path_obj = Path::new(&path);
        if watch_path_obj.exists() {
            if let Err(e) = debouncer
                .watcher()
                .watch(watch_path_obj, RecursiveMode::Recursive)
            {
                error!(
                    error = %e,
                    watch_path = %path,
                    "Failed to start watching path"
                );
                return false;
            }
            info!(
                watch_path = %path,
                debounce_ms = DEBOUNCE_TIMEOUT_MS,
                "File watcher started"
            );
        } else {
            warn!(
                watch_path = %path,
                "Watch path does not exist yet, watcher will not detect changes until path is created"
            );
            return false;
        }

        // Spawn a thread to process events from the debouncer
        // The debouncer is moved into this thread to keep it alive
        std::thread::Builder::new()
            .name("file-watcher".to_string())
            .spawn(move || {
                // Keep debouncer alive for the lifetime of this thread
                let _debouncer = debouncer;

                loop {
                    match event_rx.recv() {
                        Ok(result) => {
                            match result {
                                Ok(events) => {
                                    for event in events {
                                        let path_str = event.path.to_string_lossy().to_string();

                                        // Skip hidden files and common noise
                                        if Self::should_skip_path(&path_str) {
                                            continue;
                                        }

                                        let change_type = match event.kind {
                                            DebouncedEventKind::Any => "modify",
                                            DebouncedEventKind::AnyContinuous => continue, // Skip continuous events
                                            _ => continue, // Handle future variants gracefully
                                        };

                                        debug!(
                                            change_type = change_type,
                                            path = %path_str,
                                            "File change detected"
                                        );

                                        let file_event = FileChangeEvent {
                                            change_type: change_type.to_string(),
                                            path: path_str,
                                            timestamp: chrono::Utc::now().to_rfc3339(),
                                        };

                                        if tx.send(file_event).is_err() {
                                            debug!("No active subscribers for file change events");
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!(
                                        error = %e,
                                        "File watch error"
                                    );
                                }
                            }
                        }
                        Err(_) => {
                            // Channel closed, watcher is shutting down
                            info!("File watcher channel closed, stopping watcher thread");
                            break;
                        }
                    }
                }
            })
            .map(|_| true)
            .unwrap_or_else(|e| {
                error!(
                    error = %e,
                    "Failed to spawn file watcher thread"
                );
                false
            })
    }

    /// Check if a path should be skipped from change notifications.
    fn should_skip_path(path_str: &str) -> bool {
        // Skip hidden files and directories
        if path_str.contains("/.") {
            return true;
        }

        // Skip common noise directories
        if path_str.contains("/node_modules/")
            || path_str.contains("/__pycache__/")
            || path_str.contains("/target/")
            || path_str.contains("/.git/")
        {
            return true;
        }

        // Skip editor backup/swap files
        if path_str.ends_with(".swp")
            || path_str.ends_with(".swo")
            || path_str.ends_with("~")
            || path_str.ends_with(".tmp")
            || path_str.contains(".#")
        {
            return true;
        }

        false
    }

    /// Subscribe to file change events.
    pub fn subscribe(&self) -> broadcast::Receiver<FileChangeEvent> {
        self.sender.subscribe()
    }

    /// Get a clone of the sender for sharing.
    pub fn sender(&self) -> broadcast::Sender<FileChangeEvent> {
        self.sender.clone()
    }
}

impl Default for FileWatcher {
    fn default() -> Self {
        Self::new(
            &std::env::current_dir()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()),
        )
    }
}
