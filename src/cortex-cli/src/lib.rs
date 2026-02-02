//! Cortex CLI library module.
//!
//! This module provides shared CLI functionality including:
//! - ACP server for IDE integration
//! - Agent management commands
//! - Completion setup (first-run prompt for shell completions)
//! - Debug sandbox commands
//! - Exec command (complete headless execution mode)
//! - GitHub integration commands
//! - Login management
//! - MCP commands
//! - Models listing
//! - PR checkout commands
//! - Run command (non-interactive execution)
//! - Session export/import
//! - Upgrade management
//! - Usage statistics
//! - WSL path handling
//! - Terminal cleanup for graceful shutdown
//! - SIGTERM handling for graceful process termination
//!
//! # Module Organization
//!
//! The crate is organized into the following structure:
//!
//! - `cli/` - CLI argument parsing and command dispatch
//! - `utils/` - Shared utilities (validation, paths, clipboard, terminal, model, etc.)
//! - Command modules - Individual CLI commands (`*_cmd.rs`)
//! - `styled_output` - Themed terminal output formatting
//! - `login` - Authentication management

use std::panic;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

// Re-export the CLI module for command-line parsing
pub mod cli;

// Re-export the utilities module for shared functionality
pub mod utils;

static CLEANUP_REGISTERED: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Global flag to track if any background thread has panicked.
/// This allows the main thread to detect panics and exit with a non-zero code.
static BACKGROUND_PANIC_OCCURRED: AtomicBool = AtomicBool::new(false);

/// Exit code to use when a background panic was detected (101 is conventional for panics).
static PANIC_EXIT_CODE: AtomicI32 = AtomicI32::new(101);

/// Install a Ctrl+C handler that restores the terminal before exiting.
/// This ensures the cursor is visible and terminal state is restored
/// even when interrupting a spinner or loading animation.
///
/// Also handles SIGTERM on Unix systems for graceful container shutdown.
pub fn install_cleanup_handler() {
    // Only install once
    if CLEANUP_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }

    // Install panic hook to track background thread panics (#2805)
    install_panic_hook();

    // Install Ctrl+C (SIGINT) handler
    let _ = ctrlc::set_handler(move || {
        // Perform cleanup
        perform_cleanup();

        // Restore terminal state
        restore_terminal();

        // Exit with standard interrupt code
        std::process::exit(130);
    });

    // Install SIGTERM handler on Unix systems for graceful container shutdown
    #[cfg(unix)]
    {
        use std::sync::Once;
        static SIGTERM_HANDLER: Once = Once::new();
        SIGTERM_HANDLER.call_once(|| {
            // Use a simple approach: spawn a thread that waits for SIGTERM
            std::thread::spawn(|| {
                // Create a signal iterator for SIGTERM
                let mut signals =
                    signal_hook::iterator::Signals::new([signal_hook::consts::SIGTERM])
                        .expect("Failed to create signal handler");

                for sig in signals.forever() {
                    if sig == signal_hook::consts::SIGTERM {
                        // Print graceful shutdown message
                        eprintln!("\nShutting down gracefully...");

                        // Perform cleanup
                        perform_cleanup();

                        // Restore terminal state
                        restore_terminal();

                        // Exit with SIGTERM code (128 + 15 = 143)
                        std::process::exit(143);
                    }
                }
            });
        });
    }
}

/// Perform cleanup operations before exit.
/// Removes lock files and temporary files.
fn perform_cleanup() {
    // Clean up lock files in cortex home directory
    if let Some(home) = dirs::home_dir() {
        let cortex_home = home.join(".cortex");
        cleanup_lock_files(&cortex_home);
    }

    // Clean up temporary files
    let temp_dir = std::env::temp_dir();
    cleanup_temp_files(&temp_dir);
}

/// Clean up lock files in the given directory.
fn cleanup_lock_files(dir: &std::path::Path) {
    if !dir.exists() {
        return;
    }

    // Remove .lock files
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "lock") {
                let _ = std::fs::remove_file(&path);
            }
        }
    }
}

/// Clean up temporary files created by cortex.
fn cleanup_temp_files(temp_dir: &std::path::Path) {
    if let Ok(entries) = std::fs::read_dir(temp_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            // Remove cortex-prefixed temp files
            if file_name.starts_with("cortex-") || file_name.starts_with(".cortex") {
                if path.is_dir() {
                    let _ = std::fs::remove_dir_all(&path);
                } else {
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
    }
}

/// Install a panic hook that tracks panics in background threads.
/// This ensures the main thread can detect background panics and exit
/// with an appropriate error code (#2805).
pub fn install_panic_hook() {
    // Only install once
    if PANIC_HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }

    let original_hook = panic::take_hook();

    panic::set_hook(Box::new(move |panic_info| {
        // Mark that a panic occurred
        BACKGROUND_PANIC_OCCURRED.store(true, Ordering::SeqCst);

        // Restore terminal state before printing panic message
        restore_terminal();

        // Extract panic message to check for resource limit issues (Issue #1983)
        let panic_message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            String::new()
        };

        // Check for common resource limit error patterns
        let is_resource_limit = panic_message.contains("Resource temporarily unavailable")
            || panic_message.contains("EAGAIN")
            || panic_message.contains("Cannot allocate memory")
            || panic_message.contains("ENOMEM")
            || panic_message.contains("Too many open files")
            || panic_message.contains("EMFILE")
            || panic_message.contains("No space left on device")
            || panic_message.contains("ENOSPC")
            || panic_message.contains("cannot spawn")
            || panic_message.contains("thread 'main' panicked")
                && panic_message.contains("Os { code: 11");

        if is_resource_limit {
            eprintln!("\nError: System resource limit reached.");
            eprintln!("This can happen when:");
            eprintln!("  - Process limit (ulimit -u) is too low");
            eprintln!("  - File descriptor limit (ulimit -n) is too low");
            eprintln!("  - System memory is exhausted");
            eprintln!("\nTry increasing limits:");
            eprintln!("  ulimit -u 4096   # Increase process limit");
            eprintln!("  ulimit -n 4096   # Increase file descriptor limit");
            eprintln!("\nOr run with fewer concurrent operations.");
            return;
        }

        // Log the panic location for debugging
        if let Some(location) = panic_info.location() {
            eprintln!(
                "Panic in thread '{}' at {}:{}:{}",
                std::thread::current().name().unwrap_or("<unnamed>"),
                location.file(),
                location.line(),
                location.column()
            );
        }

        // Call original hook for standard panic output
        original_hook(panic_info);
    }));
}

/// Check if any background thread has panicked.
/// Call this before exiting to ensure proper exit code propagation.
pub fn has_background_panic() -> bool {
    BACKGROUND_PANIC_OCCURRED.load(Ordering::SeqCst)
}

/// Get the exit code to use if a background panic occurred.
pub fn get_panic_exit_code() -> i32 {
    PANIC_EXIT_CODE.load(Ordering::SeqCst)
}

/// Restore terminal state (cursor visibility, mouse mode, etc.).
/// Called on Ctrl+C or panic to ensure clean terminal state.
/// This addresses issue #2766 where mouse mode wasn't reset after abnormal termination.
pub fn restore_terminal() {
    // Show cursor (in case it was hidden by a spinner)
    eprint!("\x1b[?25h");
    // Disable mouse tracking modes that may have been enabled (#2766)
    // CSI ?1000l - Disable mouse click tracking
    // CSI ?1002l - Disable mouse button tracking
    // CSI ?1003l - Disable all mouse tracking
    // CSI ?1006l - Disable SGR extended mouse mode
    eprint!("\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l");
    // Disable bracketed paste mode
    eprint!("\x1b[?2004l");
    // Reset colors/styles
    eprint!("\x1b[0m");
    // Move to new line (in case we were mid-line)
    eprintln!();
    // Flush stderr
    let _ = std::io::Write::flush(&mut std::io::stderr());
}

pub mod acp_cmd;
pub mod agent_cmd;
pub mod alias_cmd;
pub mod cache_cmd;
pub mod compact_cmd;
pub mod completion_setup;
pub mod dag_cmd;
pub mod debug_cmd;
pub mod debug_sandbox;
pub mod exec_cmd;
pub mod export_cmd;
pub mod feedback_cmd;
pub mod github_cmd;
pub mod import_cmd;
pub mod lock_cmd;
pub mod login;
pub mod logs_cmd;
pub mod mcp_cmd;
pub mod models_cmd;
pub mod plugin_cmd;
pub mod pr_cmd;
pub mod run_cmd;
pub mod scrape_cmd;
pub mod shell_cmd;
pub mod stats_cmd;
pub mod styled_output;
pub mod uninstall_cmd;
pub mod upgrade_cmd;
pub mod workspace_cmd;

#[cfg(not(windows))]
pub mod wsl_paths;

use clap::Parser;
use cortex_common::CliConfigOverrides;

/// Seatbelt sandbox command (macOS).
#[derive(Debug, Parser)]
pub struct SeatbeltCommand {
    /// Convenience alias for low-friction sandboxed automatic execution.
    #[arg(long = "full-auto", default_value_t = false)]
    pub full_auto: bool,

    /// While the command runs, capture macOS sandbox denials.
    #[arg(long = "log-denials", default_value_t = false)]
    pub log_denials: bool,

    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Full command args to run under seatbelt.
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

/// Landlock sandbox command (Linux).
#[derive(Debug, Parser)]
pub struct LandlockCommand {
    /// Convenience alias for low-friction sandboxed automatic execution.
    #[arg(long = "full-auto", default_value_t = false)]
    pub full_auto: bool,

    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Full command args to run under landlock.
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

/// Windows sandbox command.
#[derive(Debug, Parser)]
pub struct WindowsCommand {
    /// Convenience alias for low-friction sandboxed automatic execution.
    #[arg(long = "full-auto", default_value_t = false)]
    pub full_auto: bool,

    #[clap(skip)]
    pub config_overrides: CliConfigOverrides,

    /// Full command args to run under Windows restricted token sandbox.
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    // =========================================================================
    // Atomic flag tests
    // =========================================================================

    #[test]
    fn test_cleanup_registered_initial_state() {
        // Note: The initial state depends on whether install_cleanup_handler has been called
        // We can't reset atomics, so we just verify we can read the value
        let _ = CLEANUP_REGISTERED.load(Ordering::SeqCst);
    }

    #[test]
    fn test_panic_hook_installed_initial_state() {
        let _ = PANIC_HOOK_INSTALLED.load(Ordering::SeqCst);
    }

    #[test]
    fn test_background_panic_flag() {
        // Initially should be false unless something panicked
        // We can't reset this, but we can verify the function works
        let has_panic = has_background_panic();
        // Just verify we can call the function and it returns a valid boolean
        let _ = has_panic;
    }

    #[test]
    fn test_get_panic_exit_code() {
        let exit_code = get_panic_exit_code();
        // Default is 101 (conventional panic exit code)
        assert_eq!(exit_code, 101);
    }

    // =========================================================================
    // Restore terminal function tests
    // =========================================================================

    #[test]
    fn test_restore_terminal_does_not_panic() {
        // This test just ensures restore_terminal doesn't panic
        // It writes ANSI escape codes to stderr
        restore_terminal();
    }

    // =========================================================================
    // Cleanup functions tests
    // =========================================================================

    #[test]
    fn test_cleanup_lock_files_nonexistent_dir() {
        // Should not panic when directory doesn't exist
        let nonexistent = std::path::Path::new("/nonexistent/path/that/does/not/exist");
        cleanup_lock_files(nonexistent);
    }

    #[test]
    fn test_cleanup_lock_files_empty_dir() {
        // Create a temp directory with no lock files
        let temp_dir =
            std::env::temp_dir().join(format!("test_cleanup_lock_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        cleanup_lock_files(&temp_dir);

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cleanup_lock_files_removes_lock_files() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_cleanup_lock_remove_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        // Create some lock files
        let lock_file1 = temp_dir.join("test1.lock");
        let lock_file2 = temp_dir.join("test2.lock");
        let normal_file = temp_dir.join("normal.txt");

        std::fs::write(&lock_file1, "lock1").expect("Failed to write lock file 1");
        std::fs::write(&lock_file2, "lock2").expect("Failed to write lock file 2");
        std::fs::write(&normal_file, "normal").expect("Failed to write normal file");

        // Verify files exist before cleanup
        assert!(lock_file1.exists());
        assert!(lock_file2.exists());
        assert!(normal_file.exists());

        // Run cleanup
        cleanup_lock_files(&temp_dir);

        // Lock files should be removed
        assert!(!lock_file1.exists());
        assert!(!lock_file2.exists());
        // Normal file should remain
        assert!(normal_file.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cleanup_temp_files_nonexistent_dir() {
        // Should not panic when directory doesn't exist
        let nonexistent = std::path::Path::new("/nonexistent/temp/path");
        cleanup_temp_files(nonexistent);
    }

    #[test]
    fn test_cleanup_temp_files_removes_cortex_prefixed() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_cleanup_temp_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        // Create some test files
        let cortex_file = temp_dir.join("cortex-test-file.tmp");
        let dot_cortex_file = temp_dir.join(".cortex-temp");
        let other_file = temp_dir.join("other-file.txt");

        std::fs::write(&cortex_file, "cortex").expect("Failed to write cortex file");
        std::fs::write(&dot_cortex_file, ".cortex").expect("Failed to write .cortex file");
        std::fs::write(&other_file, "other").expect("Failed to write other file");

        // Verify files exist before cleanup
        assert!(cortex_file.exists());
        assert!(dot_cortex_file.exists());
        assert!(other_file.exists());

        // Run cleanup
        cleanup_temp_files(&temp_dir);

        // Cortex files should be removed
        assert!(!cortex_file.exists());
        assert!(!dot_cortex_file.exists());
        // Other file should remain
        assert!(other_file.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_cleanup_temp_files_removes_cortex_directories() {
        let temp_dir =
            std::env::temp_dir().join(format!("test_cleanup_temp_dir_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        // Create a cortex-prefixed directory with contents
        let cortex_dir = temp_dir.join("cortex-session-abc123");
        std::fs::create_dir_all(&cortex_dir).expect("Failed to create cortex dir");
        std::fs::write(cortex_dir.join("file.txt"), "content")
            .expect("Failed to write file in cortex dir");

        // Create a non-cortex directory
        let other_dir = temp_dir.join("other-dir");
        std::fs::create_dir_all(&other_dir).expect("Failed to create other dir");

        // Verify directories exist before cleanup
        assert!(cortex_dir.exists());
        assert!(other_dir.exists());

        // Run cleanup
        cleanup_temp_files(&temp_dir);

        // Cortex directory should be removed (including contents)
        assert!(!cortex_dir.exists());
        // Other directory should remain
        assert!(other_dir.exists());

        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    // =========================================================================
    // Sandbox command struct tests
    // =========================================================================

    #[test]
    fn test_seatbelt_command_default_flags() {
        let cmd = SeatbeltCommand {
            full_auto: false,
            log_denials: false,
            config_overrides: CliConfigOverrides::default(),
            command: vec!["ls".to_string(), "-la".to_string()],
        };
        assert!(!cmd.full_auto);
        assert!(!cmd.log_denials);
        assert_eq!(cmd.command, vec!["ls", "-la"]);
    }

    #[test]
    fn test_seatbelt_command_with_flags() {
        let cmd = SeatbeltCommand {
            full_auto: true,
            log_denials: true,
            config_overrides: CliConfigOverrides::default(),
            command: vec!["echo".to_string(), "hello".to_string()],
        };
        assert!(cmd.full_auto);
        assert!(cmd.log_denials);
        assert_eq!(cmd.command.len(), 2);
    }

    #[test]
    fn test_landlock_command_default_flags() {
        let cmd = LandlockCommand {
            full_auto: false,
            config_overrides: CliConfigOverrides::default(),
            command: vec!["cat".to_string(), "/etc/hosts".to_string()],
        };
        assert!(!cmd.full_auto);
        assert_eq!(cmd.command, vec!["cat", "/etc/hosts"]);
    }

    #[test]
    fn test_landlock_command_with_full_auto() {
        let cmd = LandlockCommand {
            full_auto: true,
            config_overrides: CliConfigOverrides::default(),
            command: vec!["python".to_string(), "script.py".to_string()],
        };
        assert!(cmd.full_auto);
    }

    #[test]
    fn test_windows_command_default_flags() {
        let cmd = WindowsCommand {
            full_auto: false,
            config_overrides: CliConfigOverrides::default(),
            command: vec!["dir".to_string()],
        };
        assert!(!cmd.full_auto);
        assert_eq!(cmd.command, vec!["dir"]);
    }

    #[test]
    fn test_windows_command_with_full_auto() {
        let cmd = WindowsCommand {
            full_auto: true,
            config_overrides: CliConfigOverrides::default(),
            command: vec![
                "powershell".to_string(),
                "-Command".to_string(),
                "Get-Process".to_string(),
            ],
        };
        assert!(cmd.full_auto);
        assert_eq!(cmd.command.len(), 3);
    }

    #[test]
    fn test_sandbox_commands_empty_command() {
        let seatbelt = SeatbeltCommand {
            full_auto: false,
            log_denials: false,
            config_overrides: CliConfigOverrides::default(),
            command: vec![],
        };
        assert!(seatbelt.command.is_empty());

        let landlock = LandlockCommand {
            full_auto: false,
            config_overrides: CliConfigOverrides::default(),
            command: vec![],
        };
        assert!(landlock.command.is_empty());

        let windows = WindowsCommand {
            full_auto: false,
            config_overrides: CliConfigOverrides::default(),
            command: vec![],
        };
        assert!(windows.command.is_empty());
    }

    // =========================================================================
    // Resource limit error pattern tests
    // =========================================================================

    #[test]
    fn test_resource_limit_error_patterns() {
        // Test the patterns that would be detected as resource limit errors
        let resource_limit_messages = [
            "Resource temporarily unavailable",
            "EAGAIN",
            "Cannot allocate memory",
            "ENOMEM",
            "Too many open files",
            "EMFILE",
            "No space left on device",
            "ENOSPC",
            "cannot spawn",
        ];

        for msg in resource_limit_messages {
            // Verify these are the patterns we check for
            let is_resource = msg.contains("Resource temporarily unavailable")
                || msg.contains("EAGAIN")
                || msg.contains("Cannot allocate memory")
                || msg.contains("ENOMEM")
                || msg.contains("Too many open files")
                || msg.contains("EMFILE")
                || msg.contains("No space left on device")
                || msg.contains("ENOSPC")
                || msg.contains("cannot spawn");
            assert!(
                is_resource,
                "Should detect '{}' as resource limit error",
                msg
            );
        }

        // Test non-resource-limit messages
        let normal_messages = [
            "Connection refused",
            "File not found",
            "Permission denied",
            "Invalid argument",
        ];

        for msg in normal_messages {
            let is_resource = msg.contains("Resource temporarily unavailable")
                || msg.contains("EAGAIN")
                || msg.contains("Cannot allocate memory")
                || msg.contains("ENOMEM")
                || msg.contains("Too many open files")
                || msg.contains("EMFILE")
                || msg.contains("No space left on device")
                || msg.contains("ENOSPC")
                || msg.contains("cannot spawn");
            assert!(
                !is_resource,
                "Should not detect '{}' as resource limit error",
                msg
            );
        }
    }
}
