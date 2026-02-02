//! Debug sandbox commands for testing sandbox functionality.

use std::path::PathBuf;

use anyhow::Result;
use cortex_protocol::SandboxMode;

use crate::{LandlockCommand, SeatbeltCommand, WindowsCommand};

/// Run a command under Seatbelt sandbox (macOS only).
#[cfg(target_os = "macos")]
pub async fn run_command_under_seatbelt(
    command: SeatbeltCommand,
    _sandbox_exe: Option<PathBuf>,
) -> Result<()> {
    let SeatbeltCommand {
        full_auto,
        log_denials: _,
        config_overrides: _,
        command,
    } = command;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let sandbox_mode = create_sandbox_mode(full_auto);

    // For now, just run the command without sandbox on macOS
    // Full seatbelt implementation would use cortex_sandbox::seatbelt
    eprintln!("Running command with sandbox mode: {:?}", sandbox_mode);

    let status = tokio::process::Command::new(&command[0])
        .args(&command[1..])
        .status()
        .await?;

    // Preserve the original exit code from the command (#2835)
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(not(target_os = "macos"))]
pub async fn run_command_under_seatbelt(
    _command: SeatbeltCommand,
    _sandbox_exe: Option<PathBuf>,
) -> Result<()> {
    anyhow::bail!("Seatbelt sandbox is only available on macOS");
}

/// Run a command under Landlock sandbox (Linux only).
pub async fn run_command_under_landlock(
    command: LandlockCommand,
    _sandbox_exe: Option<PathBuf>,
) -> Result<()> {
    let LandlockCommand {
        full_auto,
        config_overrides: _,
        command,
    } = command;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let sandbox_mode = create_sandbox_mode(full_auto);

    #[cfg(target_os = "linux")]
    {
        eprintln!("Running command with sandbox mode: {sandbox_mode:?}");

        // Use cortex_sandbox for landlock
        let status = tokio::process::Command::new(&command[0])
            .args(&command[1..])
            .status()
            .await?;

        // Preserve the original exit code from the command (#2835)
        std::process::exit(status.code().unwrap_or(1));
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = sandbox_mode;
        anyhow::bail!("Landlock sandbox is only available on Linux");
    }
}

/// Run a command under Windows sandbox.
pub async fn run_command_under_windows(
    command: WindowsCommand,
    _sandbox_exe: Option<PathBuf>,
) -> Result<()> {
    let WindowsCommand {
        full_auto,
        config_overrides: _,
        command,
    } = command;

    if command.is_empty() {
        anyhow::bail!("No command provided");
    }

    let sandbox_mode = create_sandbox_mode(full_auto);

    #[cfg(windows)]
    {
        eprintln!("Running command with sandbox mode: {:?}", sandbox_mode);

        let status = tokio::process::Command::new(&command[0])
            .args(&command[1..])
            .status()
            .await?;

        // Preserve the original exit code from the command (#2835)
        std::process::exit(status.code().unwrap_or(1));
    }

    #[cfg(not(windows))]
    {
        let _ = sandbox_mode;
        anyhow::bail!("Windows sandbox is only available on Windows");
    }
}

/// Create sandbox mode based on full_auto flag.
pub fn create_sandbox_mode(full_auto: bool) -> SandboxMode {
    if full_auto {
        SandboxMode::WorkspaceWrite
    } else {
        SandboxMode::ReadOnly
    }
}
