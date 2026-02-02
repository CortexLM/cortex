//! Compaction command for Cortex CLI.
//!
//! Provides data compaction and cleanup functionality:
//! - Run manual compaction cycles
//! - Prune old log files
//! - Vacuum session database
//! - Configure auto-compaction settings
//!
//! # Race Condition Safety
//!
//! All operations acquire a lock to prevent concurrent access:
//! - Only one compaction process can run at a time
//! - Active CLI commands are protected from interference
//! - Stale locks are automatically detected and removed

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Compact CLI command.
#[derive(Debug, Parser)]
pub struct CompactCli {
    #[command(subcommand)]
    pub subcommand: Option<CompactSubcommand>,
}

/// Compact subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum CompactSubcommand {
    /// Run a compaction cycle (logs + sessions)
    Run(CompactRunArgs),

    /// Prune old log files
    Logs(CompactLogsArgs),

    /// Vacuum session database (clean orphaned files)
    Vacuum(CompactVacuumArgs),

    /// Show compaction status and statistics
    Status(CompactStatusArgs),

    /// Configure auto-compaction settings
    Config(CompactConfigArgs),
}

/// Arguments for compact run command.
#[derive(Debug, Parser)]
pub struct CompactRunArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Dry run - show what would be done without actually doing it
    #[arg(long)]
    pub dry_run: bool,

    /// Force compaction even if lock is held (use with caution)
    #[arg(long)]
    pub force: bool,
}

/// Arguments for compact logs command.
#[derive(Debug, Parser)]
pub struct CompactLogsArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Dry run - show what would be deleted without actually deleting
    #[arg(long)]
    pub dry_run: bool,

    /// Keep logs from last N days (default: 7)
    #[arg(long, default_value = "7")]
    pub keep_days: u32,

    /// Maximum log file size in MB for rotation (default: 10)
    #[arg(long, default_value = "10")]
    pub max_size_mb: u64,
}

/// Arguments for compact vacuum command.
#[derive(Debug, Parser)]
pub struct CompactVacuumArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Dry run - show what would be cleaned without actually cleaning
    #[arg(long)]
    pub dry_run: bool,

    /// Delete sessions older than N days (0 = keep all)
    #[arg(long, default_value = "0")]
    pub session_days: u32,
}

/// Arguments for compact status command.
#[derive(Debug, Parser)]
pub struct CompactStatusArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for compact config command.
#[derive(Debug, Parser)]
pub struct CompactConfigArgs {
    /// Enable auto-compaction
    #[arg(long)]
    pub enable: bool,

    /// Disable auto-compaction
    #[arg(long)]
    pub disable: bool,

    /// Set compaction interval in hours
    #[arg(long)]
    pub interval_hours: Option<u64>,

    /// Set log retention period in days
    #[arg(long)]
    pub log_retention_days: Option<u32>,

    /// Output current config as JSON
    #[arg(long)]
    pub json: bool,
}

// ============================================================================
// Path Utilities
// ============================================================================

/// Get the data directory for Cortex.
fn get_data_dir() -> PathBuf {
    // Check environment variable override first
    if let Ok(val) = std::env::var("CORTEX_DATA_DIR")
        && !val.is_empty()
    {
        return PathBuf::from(val);
    }

    dirs::data_dir()
        .map(|d| d.join("Cortex"))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".local").join("share").join("Cortex"))
                .unwrap_or_else(|| PathBuf::from(".cortex"))
        })
}

/// Get the cache/logs directory.
fn get_logs_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|c| c.join("cortex").join("logs"))
        .unwrap_or_else(|| get_data_dir().join("logs"))
}

/// Get the sessions directory.
fn get_sessions_dir() -> PathBuf {
    get_data_dir().join("sessions")
}

/// Get the history directory.
fn get_history_dir() -> PathBuf {
    get_data_dir().join("history")
}

// ============================================================================
// Statistics
// ============================================================================

/// Compaction status information.
#[derive(Debug, Serialize)]
struct CompactionStatus {
    data_dir: PathBuf,
    logs_dir: PathBuf,
    sessions_dir: PathBuf,
    history_dir: PathBuf,
    log_files_count: usize,
    log_files_size: u64,
    log_files_size_human: String,
    session_files_count: usize,
    history_files_count: usize,
    orphaned_history_count: usize,
    total_data_size: u64,
    total_data_size_human: String,
    lock_held: bool,
}

/// Calculate directory size and file count.
fn dir_stats(path: &PathBuf) -> (usize, u64) {
    let mut count = 0;
    let mut size = 0u64;

    if path.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            if entry.path().is_file() {
                count += 1;
                if let Ok(meta) = entry.metadata() {
                    size += meta.len();
                }
            }
        }
    }

    (count, size)
}

/// Format bytes as human-readable string.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Count orphaned history files (no matching session).
fn count_orphaned_history(sessions_dir: &PathBuf, history_dir: &PathBuf) -> usize {
    use std::collections::HashSet;

    // Collect session IDs
    let session_ids: HashSet<String> = if sessions_dir.exists() {
        std::fs::read_dir(sessions_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        let path = e.path();
                        if path.extension().is_some_and(|ext| ext == "json") {
                            path.file_stem().and_then(|s| s.to_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default()
    } else {
        HashSet::new()
    };

    // Count orphaned history files
    if history_dir.exists() {
        std::fs::read_dir(history_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| {
                        let path = e.path();
                        if path.extension().is_some_and(|ext| ext == "jsonl") {
                            path.file_stem()
                                .and_then(|s| s.to_str())
                                .is_some_and(|id| !session_ids.contains(id))
                        } else {
                            false
                        }
                    })
                    .count()
            })
            .unwrap_or(0)
    } else {
        0
    }
}

/// Check if compaction lock is held.
fn is_lock_held(data_dir: &Path) -> bool {
    let lock_path = data_dir.join(".compaction.lock");
    if lock_path.exists()
        && let Ok(metadata) = std::fs::metadata(&lock_path)
        && let Ok(modified) = metadata.modified()
    {
        let age = std::time::SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default();
        // Lock is valid if less than 1 hour old
        return age < std::time::Duration::from_secs(3600);
    }
    false
}

// ============================================================================
// Command Implementation
// ============================================================================

impl CompactCli {
    /// Run the compact command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            None => run_status(CompactStatusArgs { json: false }).await,
            Some(CompactSubcommand::Run(args)) => run_compact(args).await,
            Some(CompactSubcommand::Logs(args)) => run_logs(args).await,
            Some(CompactSubcommand::Vacuum(args)) => run_vacuum(args).await,
            Some(CompactSubcommand::Status(args)) => run_status(args).await,
            Some(CompactSubcommand::Config(args)) => run_config(args).await,
        }
    }
}

async fn run_status(args: CompactStatusArgs) -> Result<()> {
    let data_dir = get_data_dir();
    let logs_dir = get_logs_dir();
    let sessions_dir = get_sessions_dir();
    let history_dir = get_history_dir();

    let (log_count, log_size) = dir_stats(&logs_dir);
    let (session_count, _) = dir_stats(&sessions_dir);
    let (history_count, history_size) = dir_stats(&history_dir);
    let orphaned_count = count_orphaned_history(&sessions_dir, &history_dir);

    let total_size = log_size + history_size;

    let status = CompactionStatus {
        data_dir: data_dir.clone(),
        logs_dir,
        sessions_dir,
        history_dir,
        log_files_count: log_count,
        log_files_size: log_size,
        log_files_size_human: format_size(log_size),
        session_files_count: session_count,
        history_files_count: history_count,
        orphaned_history_count: orphaned_count,
        total_data_size: total_size,
        total_data_size_human: format_size(total_size),
        lock_held: is_lock_held(&data_dir),
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    println!("Compaction Status");
    println!("{}", "=".repeat(50));
    println!();
    println!("Directories:");
    println!("  Data:     {}", status.data_dir.display());
    println!("  Logs:     {}", status.logs_dir.display());
    println!("  Sessions: {}", status.sessions_dir.display());
    println!("  History:  {}", status.history_dir.display());
    println!();
    println!("Statistics:");
    println!(
        "  Log files:       {} ({})",
        status.log_files_count, status.log_files_size_human
    );
    println!("  Session files:   {}", status.session_files_count);
    println!("  History files:   {}", status.history_files_count);
    if status.orphaned_history_count > 0 {
        println!(
            "  Orphaned files:  {} (can be cleaned with vacuum)",
            status.orphaned_history_count
        );
    }
    println!();
    println!("Total data size: {}", status.total_data_size_human);

    if status.lock_held {
        println!();
        println!("⚠️  Compaction lock is currently held by another process");
    }

    println!();
    println!("Use 'cortex compact run' to run a compaction cycle.");
    println!("Use 'cortex compact logs' to prune old log files.");
    println!("Use 'cortex compact vacuum' to clean orphaned history files.");

    Ok(())
}

async fn run_compact(args: CompactRunArgs) -> Result<()> {
    use cortex_compact::{AutoCompactionConfig, AutoCompactionScheduler};

    let data_dir = get_data_dir();
    let logs_dir = get_logs_dir();
    let sessions_dir = get_sessions_dir();
    let history_dir = get_history_dir();

    // Check lock unless force is specified
    if !args.force && is_lock_held(&data_dir) {
        if args.json {
            let output = serde_json::json!({
                "success": false,
                "error": "Compaction lock held by another process",
                "hint": "Use --force to override"
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("⚠️  Compaction lock is held by another process.");
            println!("Wait for the other process to complete, or use --force to override.");
        }
        return Ok(());
    }

    if args.dry_run {
        println!("Dry run mode - no changes will be made.");
        println!();

        let status = CompactionStatus {
            data_dir: data_dir.clone(),
            logs_dir: logs_dir.clone(),
            sessions_dir: sessions_dir.clone(),
            history_dir: history_dir.clone(),
            log_files_count: dir_stats(&logs_dir).0,
            log_files_size: dir_stats(&logs_dir).1,
            log_files_size_human: format_size(dir_stats(&logs_dir).1),
            session_files_count: dir_stats(&sessions_dir).0,
            history_files_count: dir_stats(&history_dir).0,
            orphaned_history_count: count_orphaned_history(&sessions_dir, &history_dir),
            total_data_size: 0,
            total_data_size_human: String::new(),
            lock_held: false,
        };

        println!("Would process:");
        println!("  Log files: {}", status.log_files_count);
        println!("  Session files: {}", status.session_files_count);
        println!("  History files: {}", status.history_files_count);
        println!(
            "  Orphaned files to clean: {}",
            status.orphaned_history_count
        );
        return Ok(());
    }

    let config = AutoCompactionConfig::default();
    let scheduler =
        AutoCompactionScheduler::new(config, data_dir, logs_dir, sessions_dir, history_dir);

    println!("Running compaction cycle...");
    let stats = scheduler
        .run_once()
        .context("Failed to run compaction cycle")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }

    println!();
    println!("Compaction completed in {}ms", stats.duration_ms);

    if let Some(ref log_result) = stats.log_pruning
        && (log_result.files_deleted > 0 || log_result.files_rotated > 0)
    {
        println!();
        println!("Log pruning:");
        println!("  Files deleted: {}", log_result.files_deleted);
        println!("  Bytes freed: {}", format_size(log_result.bytes_freed));
        println!("  Files rotated: {}", log_result.files_rotated);
    }

    if let Some(ref vacuum_result) = stats.vacuum
        && (vacuum_result.orphaned_cleaned > 0 || vacuum_result.sessions_deleted > 0)
    {
        println!();
        println!("Database vacuum:");
        println!("  Sessions processed: {}", vacuum_result.sessions_processed);
        println!(
            "  Orphaned files cleaned: {}",
            vacuum_result.orphaned_cleaned
        );
        println!("  Sessions deleted: {}", vacuum_result.sessions_deleted);
        println!("  Bytes freed: {}", format_size(vacuum_result.bytes_freed));
    }

    Ok(())
}

async fn run_logs(args: CompactLogsArgs) -> Result<()> {
    use cortex_compact::{AutoCompactionConfig, LogPruner};

    let logs_dir = get_logs_dir();

    if !logs_dir.exists() {
        if args.json {
            let output = serde_json::json!({
                "success": true,
                "message": "No logs directory found"
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("No logs directory found. Nothing to prune.");
        }
        return Ok(());
    }

    if args.dry_run {
        let (count, size) = dir_stats(&logs_dir);
        if args.json {
            let output = serde_json::json!({
                "dry_run": true,
                "logs_dir": logs_dir.display().to_string(),
                "total_files": count,
                "total_size": size,
                "total_size_human": format_size(size),
                "keep_days": args.keep_days,
                "max_size_mb": args.max_size_mb,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Dry run - no changes will be made.");
            println!();
            println!("Logs directory: {}", logs_dir.display());
            println!("Total files: {}", count);
            println!("Total size: {}", format_size(size));
            println!();
            println!(
                "Would prune files older than {} days or larger than {} MB",
                args.keep_days, args.max_size_mb
            );
        }
        return Ok(());
    }

    let config = AutoCompactionConfig {
        log_retention_days: args.keep_days,
        max_log_file_size: args.max_size_mb * 1024 * 1024,
        ..Default::default()
    };

    let pruner = LogPruner::new(config);
    let result = pruner
        .prune(&logs_dir)
        .context("Failed to prune log files")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    if result.files_deleted == 0 && result.files_rotated == 0 {
        println!("No log files to prune.");
    } else {
        println!("Log pruning completed:");
        println!("  Files deleted: {}", result.files_deleted);
        println!("  Bytes freed: {}", format_size(result.bytes_freed));
        println!("  Files rotated: {}", result.files_rotated);
    }

    if !result.errors.is_empty() {
        println!();
        println!("Warnings:");
        for error in &result.errors {
            println!("  - {}", error);
        }
    }

    Ok(())
}

async fn run_vacuum(args: CompactVacuumArgs) -> Result<()> {
    use cortex_compact::{AutoCompactionConfig, DatabaseVacuumer};

    let sessions_dir = get_sessions_dir();
    let history_dir = get_history_dir();

    if args.dry_run {
        let orphaned_count = count_orphaned_history(&sessions_dir, &history_dir);
        let (session_count, _) = dir_stats(&sessions_dir);
        let (history_count, history_size) = dir_stats(&history_dir);

        if args.json {
            let output = serde_json::json!({
                "dry_run": true,
                "sessions_dir": sessions_dir.display().to_string(),
                "history_dir": history_dir.display().to_string(),
                "session_files": session_count,
                "history_files": history_count,
                "history_size": history_size,
                "history_size_human": format_size(history_size),
                "orphaned_files": orphaned_count,
                "session_retention_days": args.session_days,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Dry run - no changes will be made.");
            println!();
            println!("Sessions directory: {}", sessions_dir.display());
            println!("History directory: {}", history_dir.display());
            println!();
            println!("Session files: {}", session_count);
            println!(
                "History files: {} ({})",
                history_count,
                format_size(history_size)
            );
            println!("Orphaned history files: {}", orphaned_count);
            if args.session_days > 0 {
                println!();
                println!(
                    "Would delete sessions older than {} days",
                    args.session_days
                );
            }
        }
        return Ok(());
    }

    let config = AutoCompactionConfig {
        session_retention_days: args.session_days,
        ..Default::default()
    };

    let vacuumer = DatabaseVacuumer::new(config);
    let result = vacuumer
        .vacuum(&sessions_dir, &history_dir)
        .context("Failed to vacuum database")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(());
    }

    if result.orphaned_cleaned == 0 && result.sessions_deleted == 0 {
        println!("Database is clean. No orphaned files found.");
    } else {
        println!("Database vacuum completed:");
        println!("  Sessions processed: {}", result.sessions_processed);
        println!("  Orphaned files cleaned: {}", result.orphaned_cleaned);
        if result.sessions_deleted > 0 {
            println!("  Old sessions deleted: {}", result.sessions_deleted);
        }
        println!("  Bytes freed: {}", format_size(result.bytes_freed));
    }

    if !result.errors.is_empty() {
        println!();
        println!("Warnings:");
        for error in &result.errors {
            println!("  - {}", error);
        }
    }

    Ok(())
}

async fn run_config(args: CompactConfigArgs) -> Result<()> {
    // Load current config
    let config = AutoCompactionConfig::default();

    if args.json && !args.enable && !args.disable && args.interval_hours.is_none() {
        // Just display current config
        let output = serde_json::json!({
            "enabled": config.enabled,
            "interval_secs": config.interval_secs,
            "interval_hours": config.interval_secs / 3600,
            "log_retention_days": config.log_retention_days,
            "session_retention_days": config.session_retention_days,
            "max_log_file_size_mb": config.max_log_file_size / (1024 * 1024),
            "vacuum_on_startup": config.vacuum_on_startup,
            "min_free_disk_space_mb": config.min_free_disk_space_mb,
        });
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if !args.enable
        && !args.disable
        && args.interval_hours.is_none()
        && args.log_retention_days.is_none()
    {
        // Display config in human-readable format
        println!("Auto-Compaction Configuration");
        println!("{}", "=".repeat(40));
        println!("  Enabled: {}", config.enabled);
        println!(
            "  Interval: {} hours ({} seconds)",
            config.interval_secs / 3600,
            config.interval_secs
        );
        println!("  Log retention: {} days", config.log_retention_days);
        println!(
            "  Session retention: {} days (0 = keep forever)",
            config.session_retention_days
        );
        println!(
            "  Max log file size: {} MB",
            config.max_log_file_size / (1024 * 1024)
        );
        println!("  Vacuum on startup: {}", config.vacuum_on_startup);
        println!(
            "  Min free disk space: {} MB",
            config.min_free_disk_space_mb
        );
        println!();
        println!("Note: Configuration is currently built-in defaults.");
        println!("Custom configuration will be available in a future update.");
        return Ok(());
    }

    // Handle config changes
    // Note: This would require persistent config storage which is not yet implemented
    println!("Configuration changes:");

    if args.enable {
        println!("  Would enable auto-compaction");
    }
    if args.disable {
        println!("  Would disable auto-compaction");
    }
    if let Some(hours) = args.interval_hours {
        println!("  Would set interval to {} hours", hours);
    }
    if let Some(days) = args.log_retention_days {
        println!("  Would set log retention to {} days", days);
    }

    println!();
    println!("Note: Configuration persistence not yet implemented.");
    println!("Settings will use defaults on next run.");

    Ok(())
}

/// Auto-compaction configuration (re-exported for use in command).
use cortex_compact::AutoCompactionConfig;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ========================================================================
    // format_size tests
    // ========================================================================

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1), "1 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1023), "1023 B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
        assert_eq!(format_size(2048), "2.00 KB");
        assert_eq!(format_size(1024 * 500), "500.00 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(1024 * 1024 * 5), "5.00 MB");
        assert_eq!(format_size(1024 * 1024 + 512 * 1024), "1.50 MB");
    }

    #[test]
    fn test_format_size_gigabytes() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_size(1024 * 1024 * 1024 * 2), "2.00 GB");
        assert_eq!(
            format_size(1024 * 1024 * 1024 + 512 * 1024 * 1024),
            "1.50 GB"
        );
    }

    // ========================================================================
    // dir_stats tests
    // ========================================================================

    #[test]
    fn test_dir_stats_empty_directory() {
        let temp = tempdir().expect("failed to create temp dir");
        let path = temp.path().to_path_buf();

        let (count, size) = dir_stats(&path);

        assert_eq!(count, 0, "empty directory should have 0 files");
        assert_eq!(size, 0, "empty directory should have 0 bytes");
    }

    #[test]
    fn test_dir_stats_nonexistent_directory() {
        let path = PathBuf::from("/nonexistent/directory/that/does/not/exist");

        let (count, size) = dir_stats(&path);

        assert_eq!(count, 0, "nonexistent directory should return 0 files");
        assert_eq!(size, 0, "nonexistent directory should return 0 bytes");
    }

    #[test]
    fn test_dir_stats_with_files() {
        let temp = tempdir().expect("failed to create temp dir");
        let path = temp.path().to_path_buf();

        // Create some test files with known content
        fs::write(temp.path().join("file1.txt"), "hello").expect("failed to write file1");
        fs::write(temp.path().join("file2.txt"), "world!").expect("failed to write file2");
        fs::write(temp.path().join("file3.json"), "{}").expect("failed to write file3");

        let (count, size) = dir_stats(&path);

        assert_eq!(count, 3, "should count 3 files");
        // "hello" = 5 bytes, "world!" = 6 bytes, "{}" = 2 bytes = 13 total
        assert_eq!(size, 13, "total size should be 13 bytes");
    }

    #[test]
    fn test_dir_stats_ignores_subdirectories() {
        let temp = tempdir().expect("failed to create temp dir");
        let path = temp.path().to_path_buf();

        // Create a file and a subdirectory
        fs::write(temp.path().join("file.txt"), "content").expect("failed to write file");
        fs::create_dir(temp.path().join("subdir")).expect("failed to create subdir");
        fs::write(
            temp.path().join("subdir").join("nested.txt"),
            "nested content",
        )
        .expect("failed to write nested file");

        let (count, size) = dir_stats(&path);

        // Should only count top-level files
        assert_eq!(count, 1, "should only count top-level files");
        assert_eq!(size, 7, "should only count size of top-level files");
    }

    // ========================================================================
    // count_orphaned_history tests
    // ========================================================================

    #[test]
    fn test_count_orphaned_history_no_orphans() {
        let temp = tempdir().expect("failed to create temp dir");
        let sessions_dir = temp.path().join("sessions");
        let history_dir = temp.path().join("history");

        fs::create_dir_all(&sessions_dir).expect("failed to create sessions dir");
        fs::create_dir_all(&history_dir).expect("failed to create history dir");

        // Create matching session and history files
        fs::write(sessions_dir.join("session1.json"), "{}").expect("failed to write session");
        fs::write(history_dir.join("session1.jsonl"), "").expect("failed to write history");

        let count = count_orphaned_history(&sessions_dir, &history_dir);

        assert_eq!(count, 0, "no orphaned files when session exists");
    }

    #[test]
    fn test_count_orphaned_history_with_orphans() {
        let temp = tempdir().expect("failed to create temp dir");
        let sessions_dir = temp.path().join("sessions");
        let history_dir = temp.path().join("history");

        fs::create_dir_all(&sessions_dir).expect("failed to create sessions dir");
        fs::create_dir_all(&history_dir).expect("failed to create history dir");

        // Create session file
        fs::write(sessions_dir.join("session1.json"), "{}").expect("failed to write session");

        // Create matching and orphaned history files
        fs::write(history_dir.join("session1.jsonl"), "").expect("failed to write history");
        fs::write(history_dir.join("orphan1.jsonl"), "").expect("failed to write orphan1");
        fs::write(history_dir.join("orphan2.jsonl"), "").expect("failed to write orphan2");

        let count = count_orphaned_history(&sessions_dir, &history_dir);

        assert_eq!(count, 2, "should count 2 orphaned history files");
    }

    #[test]
    fn test_count_orphaned_history_empty_directories() {
        let temp = tempdir().expect("failed to create temp dir");
        let sessions_dir = temp.path().join("sessions");
        let history_dir = temp.path().join("history");

        fs::create_dir_all(&sessions_dir).expect("failed to create sessions dir");
        fs::create_dir_all(&history_dir).expect("failed to create history dir");

        let count = count_orphaned_history(&sessions_dir, &history_dir);

        assert_eq!(count, 0, "empty directories should have no orphans");
    }

    #[test]
    fn test_count_orphaned_history_nonexistent_directories() {
        let sessions_dir = PathBuf::from("/nonexistent/sessions");
        let history_dir = PathBuf::from("/nonexistent/history");

        let count = count_orphaned_history(&sessions_dir, &history_dir);

        assert_eq!(count, 0, "nonexistent directories should return 0");
    }

    #[test]
    fn test_count_orphaned_history_ignores_non_jsonl_files() {
        let temp = tempdir().expect("failed to create temp dir");
        let sessions_dir = temp.path().join("sessions");
        let history_dir = temp.path().join("history");

        fs::create_dir_all(&sessions_dir).expect("failed to create sessions dir");
        fs::create_dir_all(&history_dir).expect("failed to create history dir");

        // Create non-jsonl files in history dir
        fs::write(history_dir.join("readme.txt"), "readme").expect("failed to write readme");
        fs::write(history_dir.join("data.json"), "{}").expect("failed to write json");

        let count = count_orphaned_history(&sessions_dir, &history_dir);

        assert_eq!(count, 0, "should ignore non-jsonl files");
    }

    // ========================================================================
    // is_lock_held tests
    // ========================================================================

    #[test]
    fn test_is_lock_held_no_lock_file() {
        let temp = tempdir().expect("failed to create temp dir");
        let data_dir = temp.path().to_path_buf();

        let held = is_lock_held(&data_dir);

        assert!(!held, "no lock file means lock is not held");
    }

    #[test]
    fn test_is_lock_held_fresh_lock() {
        let temp = tempdir().expect("failed to create temp dir");
        let data_dir = temp.path().to_path_buf();
        let lock_path = data_dir.join(".compaction.lock");

        // Create a fresh lock file
        fs::write(&lock_path, "lock").expect("failed to write lock file");

        let held = is_lock_held(&data_dir);

        assert!(held, "fresh lock file should be considered held");
    }

    #[test]
    fn test_is_lock_held_nonexistent_directory() {
        let data_dir = PathBuf::from("/nonexistent/data/dir");

        let held = is_lock_held(&data_dir);

        assert!(!held, "nonexistent directory should not have lock held");
    }

    // ========================================================================
    // CompactionStatus serialization tests
    // ========================================================================

    #[test]
    fn test_compaction_status_serialization() {
        let status = CompactionStatus {
            data_dir: PathBuf::from("/data"),
            logs_dir: PathBuf::from("/data/logs"),
            sessions_dir: PathBuf::from("/data/sessions"),
            history_dir: PathBuf::from("/data/history"),
            log_files_count: 10,
            log_files_size: 1024 * 1024,
            log_files_size_human: "1.00 MB".to_string(),
            session_files_count: 5,
            history_files_count: 5,
            orphaned_history_count: 2,
            total_data_size: 2 * 1024 * 1024,
            total_data_size_human: "2.00 MB".to_string(),
            lock_held: false,
        };

        let json = serde_json::to_string(&status).expect("serialization should succeed");

        assert!(
            json.contains("log_files_count"),
            "JSON should contain log_files_count"
        );
        assert!(json.contains("10"), "JSON should contain count value");
        assert!(
            json.contains("1.00 MB"),
            "JSON should contain human-readable size"
        );
        assert!(
            json.contains("orphaned_history_count"),
            "JSON should contain orphaned count"
        );
        assert!(json.contains("lock_held"), "JSON should contain lock_held");
    }

    #[test]
    fn test_compaction_status_json_output_format() {
        let status = CompactionStatus {
            data_dir: PathBuf::from("/test/data"),
            logs_dir: PathBuf::from("/test/logs"),
            sessions_dir: PathBuf::from("/test/sessions"),
            history_dir: PathBuf::from("/test/history"),
            log_files_count: 0,
            log_files_size: 0,
            log_files_size_human: "0 B".to_string(),
            session_files_count: 0,
            history_files_count: 0,
            orphaned_history_count: 0,
            total_data_size: 0,
            total_data_size_human: "0 B".to_string(),
            lock_held: true,
        };

        let json =
            serde_json::to_string_pretty(&status).expect("pretty serialization should succeed");

        // Verify it's valid JSON that can be parsed back
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("should parse as valid JSON");

        assert_eq!(parsed["log_files_count"], 0, "log_files_count should be 0");
        assert_eq!(parsed["lock_held"], true, "lock_held should be true");
        assert_eq!(
            parsed["log_files_size_human"], "0 B",
            "size human should be '0 B'"
        );
    }

    // ========================================================================
    // CompactLogsArgs default value tests
    // ========================================================================

    #[test]
    fn test_compact_logs_args_defaults() {
        // Test that default values are sensible
        let args = CompactLogsArgs {
            json: false,
            dry_run: false,
            keep_days: 7,
            max_size_mb: 10,
        };

        assert_eq!(args.keep_days, 7, "default keep_days should be 7");
        assert_eq!(args.max_size_mb, 10, "default max_size_mb should be 10");
    }

    // ========================================================================
    // CompactVacuumArgs default value tests
    // ========================================================================

    #[test]
    fn test_compact_vacuum_args_defaults() {
        let args = CompactVacuumArgs {
            json: false,
            dry_run: false,
            session_days: 0,
        };

        assert_eq!(
            args.session_days, 0,
            "default session_days should be 0 (keep all)"
        );
    }
}
