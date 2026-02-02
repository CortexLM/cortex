//! Auto-compaction scheduler for periodic compaction tasks.

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};

use crate::Result;

use super::config::AutoCompactionConfig;
use super::lock::CompactionLock;
use super::log_pruner::{LogPruner, LogPruningResult};
use super::utils::timestamp_now;
use super::vacuumer::{DatabaseVacuumer, VacuumResult};

/// Stats from a compaction run.
#[derive(Debug, Clone, Serialize)]
pub struct CompactionStats {
    /// Timestamp of compaction.
    pub timestamp: u64,
    /// Log pruning results.
    pub log_pruning: Option<LogPruningResult>,
    /// Database vacuum results.
    pub vacuum: Option<VacuumResult>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Whether the run was successful overall.
    pub success: bool,
}

/// The auto-compaction scheduler handles periodic compaction tasks.
pub struct AutoCompactionScheduler {
    config: AutoCompactionConfig,
    data_dir: PathBuf,
    logs_dir: PathBuf,
    sessions_dir: PathBuf,
    history_dir: PathBuf,
    running: Arc<AtomicBool>,
}

impl AutoCompactionScheduler {
    /// Create a new auto-compaction scheduler.
    pub fn new(
        config: AutoCompactionConfig,
        data_dir: PathBuf,
        logs_dir: PathBuf,
        sessions_dir: PathBuf,
        history_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            data_dir,
            logs_dir,
            sessions_dir,
            history_dir,
            running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Get the running flag for cancellation.
    pub fn running(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.running)
    }

    /// Run a single compaction cycle.
    ///
    /// This acquires a lock to prevent concurrent runs, then:
    /// 1. Prunes log files
    /// 2. Vacuums session database
    pub fn run_once(&self) -> Result<CompactionStats> {
        let start = std::time::Instant::now();
        let mut stats = CompactionStats {
            timestamp: timestamp_now(),
            log_pruning: None,
            vacuum: None,
            duration_ms: 0,
            success: false,
        };

        // Try to acquire lock
        let lock = match CompactionLock::try_acquire(&self.data_dir)? {
            Some(lock) => lock,
            None => {
                debug!("Another compaction process is running, skipping");
                return Ok(stats);
            }
        };

        self.running.store(true, Ordering::SeqCst);

        // Run log pruning
        let log_pruner = LogPruner::new(self.config.clone());
        match log_pruner.prune(&self.logs_dir) {
            Ok(result) => stats.log_pruning = Some(result),
            Err(e) => {
                error!(error = %e, "Log pruning failed");
            }
        }

        // Run database vacuum
        let vacuumer = DatabaseVacuumer::new(self.config.clone());
        match vacuumer.vacuum(&self.sessions_dir, &self.history_dir) {
            Ok(result) => stats.vacuum = Some(result),
            Err(e) => {
                error!(error = %e, "Database vacuum failed");
            }
        }

        self.running.store(false, Ordering::SeqCst);
        stats.duration_ms = start.elapsed().as_millis() as u64;
        stats.success = true;

        // Lock is automatically released when dropped
        drop(lock);

        info!(
            duration_ms = stats.duration_ms,
            "Auto-compaction cycle completed"
        );

        Ok(stats)
    }

    /// Start the scheduler in a background thread.
    ///
    /// Returns a handle that can be used to stop the scheduler.
    pub fn start(self) -> CompactionHandle {
        let running = Arc::clone(&self.running);
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_flag_clone = Arc::clone(&stop_flag);

        let handle = std::thread::spawn(move || {
            info!(
                interval_secs = self.config.interval_secs,
                "Auto-compaction scheduler started"
            );

            // Run initial compaction if configured
            if self.config.vacuum_on_startup {
                if let Err(e) = self.run_once() {
                    error!(error = %e, "Initial compaction failed");
                }
            }

            loop {
                // Sleep for the configured interval
                for _ in 0..self.config.interval_secs {
                    if stop_flag_clone.load(Ordering::SeqCst) {
                        info!("Auto-compaction scheduler stopping");
                        return;
                    }
                    std::thread::sleep(Duration::from_secs(1));
                }

                // Run compaction cycle
                if !stop_flag_clone.load(Ordering::SeqCst) {
                    if let Err(e) = self.run_once() {
                        error!(error = %e, "Compaction cycle failed");
                    }
                }
            }
        });

        CompactionHandle {
            stop_flag,
            _running: running,
            thread: Some(handle),
        }
    }
}

/// Handle for controlling the compaction scheduler.
pub struct CompactionHandle {
    stop_flag: Arc<AtomicBool>,
    _running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CompactionHandle {
    /// Request the scheduler to stop.
    pub fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }

    /// Check if the scheduler is still running.
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl Drop for CompactionHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_dirs() -> (TempDir, PathBuf, PathBuf, PathBuf, PathBuf) {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = temp_dir.path().to_path_buf();
        let logs_dir = data_dir.join("logs");
        let sessions_dir = data_dir.join("sessions");
        let history_dir = data_dir.join("history");

        fs::create_dir_all(&logs_dir).unwrap();
        fs::create_dir_all(&sessions_dir).unwrap();
        fs::create_dir_all(&history_dir).unwrap();

        (temp_dir, data_dir, logs_dir, sessions_dir, history_dir)
    }

    #[test]
    fn test_scheduler_run_once() {
        let (_temp, data_dir, logs_dir, sessions_dir, history_dir) = create_test_dirs();

        let config = AutoCompactionConfig {
            vacuum_on_startup: false,
            ..Default::default()
        };

        let scheduler =
            AutoCompactionScheduler::new(config, data_dir, logs_dir, sessions_dir, history_dir);

        let stats = scheduler.run_once().unwrap();
        assert!(stats.success);
    }
}
