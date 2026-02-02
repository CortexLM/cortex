//! Cortex CLI - Main entry point.
//!
//! This is the main entry point for the Cortex CLI, providing:
//! - Interactive TUI mode (default)
//! - Non-interactive exec mode
//! - Session management (resume, list)
//! - Login/logout authentication
//! - MCP server management
//! - Debug sandbox commands
//! - Shell completions
//!
//! # Architecture
//!
//! The CLI is structured as follows:
//! - `cli/` - Command-line argument parsing and dispatch
//! - `utils/` - Shared utilities for all commands
//! - `*_cmd.rs` - Individual command implementations

use anyhow::Result;
use clap::Parser;

use cortex_cli::cli::{Cli, ColorMode, Commands, LogLevel, dispatch_command};

/// Apply process hardening measures early in startup.
#[cfg(not(debug_assertions))]
#[ctor::ctor]
fn pre_main_hardening() {
    cortex_process_hardening::pre_main_hardening();
}

/// Guard that ensures debug log file is properly flushed when dropped.
struct DebugLogGuard {
    _guard: tracing_appender::non_blocking::WorkerGuard,
}

/// Set up debug file logging that writes ALL trace-level logs to ./debug.txt.
fn setup_debug_file_logging() -> Result<DebugLogGuard> {
    use std::fs::File;
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let debug_file_path = std::env::current_dir()?.join("debug.txt");

    let file = File::create(&debug_file_path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to create debug.txt: {}. Check write permissions.",
            e
        )
    })?;

    let (non_blocking, guard) = tracing_appender::non_blocking(file);

    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_file(true)
        .with_line_number(true);

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new("trace"))
        .with(file_layer)
        .init();

    eprintln!(
        "Debug mode enabled: logging to {}",
        debug_file_path.display()
    );

    Ok(DebugLogGuard { _guard: guard })
}

/// Check if CORTEX_HOME is writable.
fn check_cortex_home_writable() -> Result<()> {
    use anyhow::bail;

    if let Ok(cortex_home_env) = std::env::var("CORTEX_HOME") {
        let cortex_home_path = std::path::Path::new(&cortex_home_env);
        if cortex_home_path.exists() {
            let test_file = cortex_home_path.join(".write_test");
            match std::fs::File::create(&test_file) {
                Ok(_) => {
                    let _ = std::fs::remove_file(&test_file);
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    bail!(
                        "Cannot write to CORTEX_HOME: Permission denied\n\n\
                        CORTEX_HOME is set to: {}\n\
                        This directory exists but is not writable.\n\n\
                        To fix this, either:\n\
                        - Change permissions: chmod u+w {}\n\
                        - Use a different directory: export CORTEX_HOME=/path/to/writable/dir\n\
                        - Unset the variable to use default: unset CORTEX_HOME",
                        cortex_home_env,
                        cortex_home_env
                    );
                }
                Err(_) => {
                    // Other errors (e.g., disk full) - continue and let it fail later
                }
            }
        }
    }
    Ok(())
}

/// Check for updates in the background.
async fn check_for_updates_background() {
    // Use cortex_update crate for update checking
    // This runs asynchronously and doesn't block the main command
    if let Ok(manager) = cortex_update::UpdateManager::new()
        && let Ok(Some(update_info)) = manager.check_update().await
    {
        eprintln!(
            "\n\x1b[1;33mUpdate available:\x1b[0m {} -> {}\n\
                 Run 'cortex upgrade' to update.\n",
            update_info.current_version, update_info.latest_version
        );
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Install Ctrl+C handler to restore terminal state before exiting
    cortex_cli::install_cleanup_handler();

    // Install panic hook that suggests RUST_BACKTRACE for debugging
    cortex_cli::install_panic_hook();

    let cli = Cli::parse();

    // Handle color mode
    // SAFETY: Environment variable mutations happen early before threads spawn
    match cli.color {
        ColorMode::Never => unsafe { std::env::set_var("NO_COLOR", "1") },
        ColorMode::Always => unsafe { std::env::remove_var("NO_COLOR") },
        ColorMode::Auto => {}
    }

    // Early check for CORTEX_HOME writability
    let is_debug_cmd = matches!(&cli.command, Some(Commands::Debug(_)));
    if !is_debug_cmd {
        check_cortex_home_writable()?;
    }

    // Initialize debug file logging if --debug flag is passed
    let _debug_guard = if cli.interactive.debug {
        Some(setup_debug_file_logging()?)
    } else {
        None
    };

    // Initialize logging for non-TUI commands (when not in debug mode)
    if cli.command.is_some() && !cli.interactive.debug {
        let log_level = if cli.trace {
            LogLevel::Trace
        } else if cli.verbose {
            LogLevel::Debug
        } else if let Ok(env_level) = std::env::var("CORTEX_LOG_LEVEL") {
            LogLevel::from_str_loose(&env_level).unwrap_or(cli.interactive.log_level)
        } else {
            cli.interactive.log_level
        };

        let filter_str = if std::env::var("RUST_LOG").is_ok() {
            format!(
                "error,cortex={},cortex_cli={},cortex_engine={},cortex_common={}",
                log_level.as_filter_str(),
                log_level.as_filter_str(),
                log_level.as_filter_str(),
                log_level.as_filter_str()
            )
        } else {
            log_level.as_filter_str().to_string()
        };

        tracing_subscriber::fmt()
            .with_env_filter(&filter_str)
            .init();
    }

    // Background update check (non-blocking)
    let is_upgrade_cmd = matches!(&cli.command, Some(Commands::Upgrade(_)));
    let is_tui_mode = cli.command.is_none();
    if !is_upgrade_cmd && !is_tui_mode {
        tokio::spawn(async {
            check_for_updates_background().await;
        });
    }

    // Dispatch the command
    dispatch_command(cli).await
}
