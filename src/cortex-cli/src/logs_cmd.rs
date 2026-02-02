//! Logs command for Cortex CLI.
//!
//! Provides log viewing functionality:
//! - View recent logs
//! - Tail logs in real-time
//! - Filter logs by level
//! - Clear old logs

use anyhow::Result;
use clap::Parser;
use serde::Serialize;
use std::path::PathBuf;

/// Logs CLI command.
#[derive(Debug, Parser)]
pub struct LogsCli {
    /// Number of lines to show (default: 100)
    #[arg(long, short = 'n', default_value = "100")]
    pub lines: usize,

    /// Follow logs in real-time (like tail -f)
    #[arg(long, short = 'f')]
    pub follow: bool,

    /// Filter by log level (error, warn, info, debug, trace)
    #[arg(long, short = 'l')]
    pub level: Option<String>,

    /// Show logs from a specific session
    #[arg(long, short = 's')]
    pub session: Option<String>,

    /// Output as JSON
    #[arg(long)]
    pub json: bool,

    /// Show log file paths instead of content
    #[arg(long)]
    pub paths: bool,

    /// Clear old log files
    #[arg(long)]
    pub clear: bool,

    /// Keep logs from last N days when clearing (default: 7)
    #[arg(long, default_value = "7")]
    pub keep_days: u32,
}

/// Log file information.
#[derive(Debug, Serialize)]
struct LogFileInfo {
    path: PathBuf,
    size_bytes: u64,
    size_human: String,
    modified: String,
}

/// Get the logs directory.
fn get_logs_dir() -> PathBuf {
    dirs::cache_dir()
        .map(|c| c.join("cortex").join("logs"))
        .unwrap_or_else(|| {
            dirs::home_dir()
                .map(|h| h.join(".cache").join("cortex").join("logs"))
                .unwrap_or_else(|| PathBuf::from(".cache/cortex/logs"))
        })
}

/// Format bytes as human-readable string.
fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;

    if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

impl LogsCli {
    /// Run the logs command.
    pub async fn run(self) -> Result<()> {
        let logs_dir = get_logs_dir();

        // Handle clear command
        if self.clear {
            return self.run_clear(&logs_dir).await;
        }

        // Handle paths command
        if self.paths {
            return self.run_paths(&logs_dir).await;
        }

        // Check if logs directory exists
        if !logs_dir.exists() {
            if self.json {
                println!("[]");
            } else {
                println!("No logs found.");
                println!("\nLogs directory: {}", logs_dir.display());
                println!("Logs are created when running cortex with --debug flag.");
            }
            return Ok(());
        }

        // Find log files
        let mut log_files: Vec<PathBuf> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&logs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if name.ends_with(".log") || name.ends_with(".txt") {
                        log_files.push(path);
                    }
                }
            }
        }

        // Also check for debug.txt in current directory
        let debug_txt = std::env::current_dir().map(|d| d.join("debug.txt")).ok();
        if let Some(ref debug_path) = debug_txt
            && debug_path.exists()
        {
            log_files.push(debug_path.clone());
        }

        if log_files.is_empty() {
            if self.json {
                println!("[]");
            } else {
                println!("No log files found.");
                println!("\nLogs directory: {}", logs_dir.display());
                println!("Create logs by running cortex with --debug flag.");
            }
            return Ok(());
        }

        // Sort by modification time (newest first)
        log_files.sort_by(|a, b| {
            let a_time = std::fs::metadata(a)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let b_time = std::fs::metadata(b)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            b_time.cmp(&a_time)
        });

        // Get the most recent log file
        let log_file = &log_files[0];

        // Handle follow mode
        if self.follow {
            return self.run_follow(log_file).await;
        }

        // Read and display log content
        self.run_show(log_file).await
    }

    async fn run_show(&self, log_file: &PathBuf) -> Result<()> {
        let content = std::fs::read_to_string(log_file)?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // Apply level filter if specified
        let filtered_lines: Vec<&str> = if let Some(ref level) = self.level {
            let level_upper = level.to_uppercase();
            lines
                .iter()
                .filter(|line| line.to_uppercase().contains(&level_upper))
                .copied()
                .collect()
        } else {
            lines
        };

        // Get last N lines
        let start = if filtered_lines.len() > self.lines {
            filtered_lines.len() - self.lines
        } else {
            0
        };
        let display_lines = &filtered_lines[start..];

        if self.json {
            let output = serde_json::json!({
                "log_file": log_file.display().to_string(),
                "total_lines": total_lines,
                "filtered_lines": filtered_lines.len(),
                "displayed_lines": display_lines.len(),
                "lines": display_lines,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
        } else {
            println!("Log file: {}", log_file.display());
            println!("{}", "-".repeat(60));
            for line in display_lines {
                println!("{}", line);
            }
            if filtered_lines.len() > self.lines {
                println!(
                    "\n... showing last {} of {} lines",
                    self.lines,
                    filtered_lines.len()
                );
            }
        }

        Ok(())
    }

    async fn run_follow(&self, log_file: &PathBuf) -> Result<()> {
        use std::io::{BufRead, BufReader, Seek, SeekFrom};

        println!("Following log file: {}", log_file.display());
        println!("Press Ctrl+C to stop.\n");

        let file = std::fs::File::open(log_file)?;
        let mut reader = BufReader::new(file);

        // Seek to end
        reader.seek(SeekFrom::End(0))?;

        let level_filter = self.level.clone();

        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    // No new content, wait a bit
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Ok(_) => {
                    // Apply level filter if specified
                    if let Some(ref level) = level_filter
                        && !line.to_uppercase().contains(&level.to_uppercase())
                    {
                        continue;
                    }
                    print!("{}", line);
                }
                Err(e) => {
                    eprintln!("Error reading log: {}", e);
                    break;
                }
            }
        }

        #[allow(unreachable_code)]
        Ok(())
    }

    async fn run_paths(&self, logs_dir: &PathBuf) -> Result<()> {
        let mut log_files: Vec<LogFileInfo> = Vec::new();

        if logs_dir.exists()
            && let Ok(entries) = std::fs::read_dir(logs_dir)
        {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let Ok(meta) = entry.metadata()
                {
                    let modified = meta
                        .modified()
                        .map(|t| {
                            chrono::DateTime::<chrono::Utc>::from(t)
                                .format("%Y-%m-%d %H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_else(|_| "unknown".to_string());

                    log_files.push(LogFileInfo {
                        path,
                        size_bytes: meta.len(),
                        size_human: format_size(meta.len()),
                        modified,
                    });
                }
            }
        }

        // Sort by modification time
        log_files.sort_by(|a, b| b.modified.cmp(&a.modified));

        if self.json {
            println!("{}", serde_json::to_string_pretty(&log_files)?);
        } else {
            println!("Log Files:");
            println!("{}", "-".repeat(60));
            if log_files.is_empty() {
                println!("  No log files found.");
            } else {
                for file in &log_files {
                    println!(
                        "  {} ({}) - {}",
                        file.path.display(),
                        file.size_human,
                        file.modified
                    );
                }
            }
            println!("\nLogs directory: {}", logs_dir.display());
        }

        Ok(())
    }

    async fn run_clear(&self, logs_dir: &PathBuf) -> Result<()> {
        if !logs_dir.exists() {
            println!("No logs to clear.");
            return Ok(());
        }

        let cutoff = chrono::Utc::now() - chrono::Duration::days(self.keep_days as i64);
        let cutoff_time = std::time::SystemTime::from(cutoff);

        let mut cleared = 0usize;
        let mut cleared_bytes = 0u64;

        if let Ok(entries) = std::fs::read_dir(logs_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && let Ok(meta) = entry.metadata()
                    && let Ok(modified) = meta.modified()
                    && modified < cutoff_time
                {
                    let size = meta.len();
                    if let Ok(()) = std::fs::remove_file(&path) {
                        cleared += 1;
                        cleared_bytes += size;
                    }
                }
            }
        }

        if cleared > 0 {
            println!(
                "Cleared {} log file(s) ({}) older than {} days.",
                cleared,
                format_size(cleared_bytes),
                self.keep_days
            );
        } else {
            println!("No old log files to clear.");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Tests for format_size()
    // =========================================================================

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
        assert_eq!(format_size(10240), "10.00 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        assert_eq!(format_size(MB), "1.00 MB");
        assert_eq!(format_size(MB + 512 * KB), "1.50 MB");
        assert_eq!(format_size(5 * MB), "5.00 MB");
        assert_eq!(format_size(100 * MB), "100.00 MB");
    }

    #[test]
    fn test_format_size_boundary_values() {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;

        // Just below 1 KB
        assert_eq!(format_size(1023), "1023 B");
        // Exactly 1 KB
        assert_eq!(format_size(1024), "1.00 KB");
        // Just below 1 MB
        assert_eq!(format_size(MB - 1), "1024.00 KB");
        // Exactly 1 MB
        assert_eq!(format_size(MB), "1.00 MB");
    }

    // =========================================================================
    // Tests for LogsCli default values
    // =========================================================================

    #[test]
    fn test_logs_cli_default_lines() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert_eq!(cli.lines, 100, "Default lines should be 100");
    }

    #[test]
    fn test_logs_cli_default_follow() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert!(!cli.follow, "Follow should be false by default");
    }

    #[test]
    fn test_logs_cli_default_json() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert!(!cli.json, "JSON should be false by default");
    }

    #[test]
    fn test_logs_cli_default_paths() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert!(!cli.paths, "Paths should be false by default");
    }

    #[test]
    fn test_logs_cli_default_clear() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert!(!cli.clear, "Clear should be false by default");
    }

    #[test]
    fn test_logs_cli_default_keep_days() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert_eq!(cli.keep_days, 7, "Default keep_days should be 7");
    }

    #[test]
    fn test_logs_cli_default_level() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert!(cli.level.is_none(), "Level should be None by default");
    }

    #[test]
    fn test_logs_cli_default_session() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs"]);
        assert!(cli.session.is_none(), "Session should be None by default");
    }

    // =========================================================================
    // Tests for LogsCli with custom values
    // =========================================================================

    #[test]
    fn test_logs_cli_custom_lines_short() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "-n", "50"]);
        assert_eq!(cli.lines, 50, "Lines should be set to 50");
    }

    #[test]
    fn test_logs_cli_custom_lines_long() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--lines", "200"]);
        assert_eq!(cli.lines, 200, "Lines should be set to 200");
    }

    #[test]
    fn test_logs_cli_follow_short() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "-f"]);
        assert!(cli.follow, "Follow should be true when -f is passed");
    }

    #[test]
    fn test_logs_cli_follow_long() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--follow"]);
        assert!(cli.follow, "Follow should be true when --follow is passed");
    }

    #[test]
    fn test_logs_cli_level_filter_short() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "-l", "error"]);
        assert_eq!(
            cli.level,
            Some("error".to_string()),
            "Level should be 'error'"
        );
    }

    #[test]
    fn test_logs_cli_level_filter_long() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--level", "debug"]);
        assert_eq!(
            cli.level,
            Some("debug".to_string()),
            "Level should be 'debug'"
        );
    }

    #[test]
    fn test_logs_cli_session_short() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "-s", "my-session-123"]);
        assert_eq!(
            cli.session,
            Some("my-session-123".to_string()),
            "Session should be set"
        );
    }

    #[test]
    fn test_logs_cli_session_long() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--session", "another-session"]);
        assert_eq!(
            cli.session,
            Some("another-session".to_string()),
            "Session should be set"
        );
    }

    #[test]
    fn test_logs_cli_json_flag() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--json"]);
        assert!(cli.json, "JSON should be true when --json is passed");
    }

    #[test]
    fn test_logs_cli_paths_flag() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--paths"]);
        assert!(cli.paths, "Paths should be true when --paths is passed");
    }

    #[test]
    fn test_logs_cli_clear_flag() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--clear"]);
        assert!(cli.clear, "Clear should be true when --clear is passed");
    }

    #[test]
    fn test_logs_cli_custom_keep_days() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "--keep-days", "14"]);
        assert_eq!(cli.keep_days, 14, "Keep days should be 14");
    }

    #[test]
    fn test_logs_cli_combined_flags() {
        use clap::Parser;

        let cli = LogsCli::parse_from(["logs", "-n", "25", "-l", "warn", "--json"]);
        assert_eq!(cli.lines, 25, "Lines should be 25");
        assert_eq!(
            cli.level,
            Some("warn".to_string()),
            "Level should be 'warn'"
        );
        assert!(cli.json, "JSON should be true");
    }

    // =========================================================================
    // Tests for LogFileInfo serialization
    // =========================================================================

    #[test]
    fn test_log_file_info_serialization() {
        let info = LogFileInfo {
            path: PathBuf::from("/var/log/test.log"),
            size_bytes: 1024,
            size_human: "1.00 KB".to_string(),
            modified: "2024-01-15 10:30:00".to_string(),
        };

        let json = serde_json::to_string(&info).expect("serialization should succeed");
        assert!(
            json.contains("test.log"),
            "JSON should contain the file name"
        );
        assert!(json.contains("1024"), "JSON should contain size_bytes");
        assert!(json.contains("1.00 KB"), "JSON should contain size_human");
        assert!(
            json.contains("2024-01-15 10:30:00"),
            "JSON should contain modified timestamp"
        );
    }

    #[test]
    fn test_log_file_info_serialization_fields() {
        let info = LogFileInfo {
            path: PathBuf::from("/logs/debug.log"),
            size_bytes: 2048,
            size_human: "2.00 KB".to_string(),
            modified: "2024-06-20 14:00:00".to_string(),
        };

        let json = serde_json::to_string(&info).expect("serialization should succeed");
        assert!(
            json.contains("\"path\""),
            "JSON should contain path field name"
        );
        assert!(
            json.contains("\"size_bytes\""),
            "JSON should contain size_bytes field name"
        );
        assert!(
            json.contains("\"size_human\""),
            "JSON should contain size_human field name"
        );
        assert!(
            json.contains("\"modified\""),
            "JSON should contain modified field name"
        );
    }

    #[test]
    fn test_log_file_info_serialization_with_large_file() {
        const MB: u64 = 1024 * 1024;
        let info = LogFileInfo {
            path: PathBuf::from("/var/log/large.log"),
            size_bytes: 50 * MB,
            size_human: "50.00 MB".to_string(),
            modified: "2024-12-31 23:59:59".to_string(),
        };

        let json = serde_json::to_string(&info).expect("serialization should succeed");
        assert!(
            json.contains("large.log"),
            "JSON should contain the file name"
        );
        assert!(
            json.contains(&(50 * MB).to_string()),
            "JSON should contain size_bytes"
        );
        assert!(json.contains("50.00 MB"), "JSON should contain size_human");
    }

    // =========================================================================
    // Tests for get_logs_dir()
    // =========================================================================

    #[test]
    fn test_get_logs_dir_returns_valid_path() {
        let logs_dir = get_logs_dir();
        // The path should contain "cortex" and "logs" somewhere
        let path_str = logs_dir.to_string_lossy();
        assert!(
            path_str.contains("cortex") && path_str.contains("logs"),
            "Logs directory should contain 'cortex' and 'logs' in path: {}",
            path_str
        );
    }

    #[test]
    fn test_get_logs_dir_is_absolute_or_relative() {
        let logs_dir = get_logs_dir();
        // The function should return a non-empty path
        assert!(
            !logs_dir.as_os_str().is_empty(),
            "Logs directory path should not be empty"
        );
    }
}
