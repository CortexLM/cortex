//! Ripgrep command handler.

use anyhow::{Result, bail};
use std::path::PathBuf;

use crate::debug_cmd::commands::RipgrepArgs;
use crate::debug_cmd::types::{RipgrepDebugOutput, RipgrepTestResult};
use crate::debug_cmd::utils::{check_command_installed, get_path_directories};

/// Run the ripgrep debug command.
pub async fn run_ripgrep(args: RipgrepArgs) -> Result<()> {
    let (available, path, version) = check_command_installed("rg").await;

    // When ripgrep is not found, include the searched paths for debugging
    let searched_paths = if !available {
        Some(get_path_directories())
    } else {
        None
    };

    let test_result = if let Some(pattern) = args.test {
        let dir = args
            .dir
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

        if available {
            let start = std::time::Instant::now();
            let output = tokio::process::Command::new("rg")
                .args(["--count", "--no-heading", &pattern])
                .current_dir(&dir)
                .output()
                .await;

            let duration_ms = start.elapsed().as_millis() as u64;

            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let matches: usize = stdout
                        .lines()
                        .filter_map(|line| {
                            line.rsplit(':')
                                .next()
                                .and_then(|n| n.parse::<usize>().ok())
                        })
                        .sum();

                    Some(RipgrepTestResult {
                        pattern,
                        directory: dir,
                        matches_found: matches,
                        duration_ms,
                        error: None,
                    })
                }
                Err(e) => Some(RipgrepTestResult {
                    pattern,
                    directory: dir,
                    matches_found: 0,
                    duration_ms,
                    error: Some(e.to_string()),
                }),
            }
        } else {
            Some(RipgrepTestResult {
                pattern,
                directory: dir,
                matches_found: 0,
                duration_ms: 0,
                error: Some("ripgrep not installed".to_string()),
            })
        }
    } else {
        None
    };

    let output = RipgrepDebugOutput {
        available,
        version,
        path,
        searched_paths,
        test_result,
    };

    if args.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        println!("Ripgrep Status");
        println!("{}", "=".repeat(50));
        println!(
            "  Available: {}",
            if output.available { "yes" } else { "no" }
        );
        if let Some(ref path) = output.path {
            println!("  Path:      {}", path.display());
        }
        if let Some(ref version) = output.version {
            println!("  Version:   {}", version);
        }

        if let Some(ref test) = output.test_result {
            println!();
            println!("Search Test");
            println!("{}", "-".repeat(40));
            println!("  Pattern:   {}", test.pattern);
            println!("  Directory: {}", test.directory.display());
            println!("  Matches:   {}", test.matches_found);
            println!("  Duration:  {}ms", test.duration_ms);
            if let Some(ref error) = test.error {
                println!("  Error:     {}", error);
            }
        }

        if !output.available {
            println!();
            if let Some(ref paths) = output.searched_paths {
                println!("Searched Paths");
                println!("{}", "-".repeat(40));
                if paths.is_empty() {
                    println!("  (PATH environment variable is empty or not set)");
                } else {
                    for p in paths {
                        println!("  {}", p.display());
                    }
                }
            }
            println!();
            println!("To install ripgrep:");
            println!("  macOS:   brew install ripgrep");
            println!("  Linux:   apt install ripgrep / dnf install ripgrep");
            println!("  Windows: winget install BurntSushi.ripgrep.MSVC");

            // Offer to install if --install flag was provided
            if args.install {
                println!();
                println!("Attempting to install ripgrep...");
                if let Err(e) = install_ripgrep().await {
                    eprintln!("Failed to install ripgrep: {e}");
                    eprintln!("Please install manually using the commands above.");
                }
            }
        }
    }

    Ok(())
}

/// Attempt to install ripgrep using the system package manager.
async fn install_ripgrep() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Try Homebrew first
        let output = tokio::process::Command::new("brew")
            .args(["install", "ripgrep"])
            .status()
            .await;

        match output {
            Ok(status) if status.success() => {
                println!("ripgrep installed successfully via Homebrew!");
                return Ok(());
            }
            _ => {
                bail!("Failed to install via Homebrew. Is Homebrew installed?");
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try apt first (Debian/Ubuntu)
        if tokio::process::Command::new("which")
            .arg("apt")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            let output = tokio::process::Command::new("sudo")
                .args(["apt", "install", "-y", "ripgrep"])
                .status()
                .await;

            match output {
                Ok(status) if status.success() => {
                    println!("ripgrep installed successfully via apt!");
                    return Ok(());
                }
                _ => {}
            }
        }

        // Try dnf (Fedora/RHEL)
        if tokio::process::Command::new("which")
            .arg("dnf")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            let output = tokio::process::Command::new("sudo")
                .args(["dnf", "install", "-y", "ripgrep"])
                .status()
                .await;

            match output {
                Ok(status) if status.success() => {
                    println!("ripgrep installed successfully via dnf!");
                    return Ok(());
                }
                _ => {}
            }
        }

        // Try pacman (Arch)
        if tokio::process::Command::new("which")
            .arg("pacman")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            let output = tokio::process::Command::new("sudo")
                .args(["pacman", "-S", "--noconfirm", "ripgrep"])
                .status()
                .await;

            match output {
                Ok(status) if status.success() => {
                    println!("ripgrep installed successfully via pacman!");
                    return Ok(());
                }
                _ => {}
            }
        }

        bail!("Could not detect a supported package manager (apt, dnf, pacman)");
    }

    #[cfg(target_os = "windows")]
    {
        // Try winget
        let output = tokio::process::Command::new("winget")
            .args(["install", "-e", "--id", "BurntSushi.ripgrep.MSVC"])
            .status()
            .await;

        match output {
            Ok(status) if status.success() => {
                println!("ripgrep installed successfully via winget!");
                return Ok(());
            }
            _ => {
                bail!("Failed to install via winget. Is winget available?");
            }
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("Automatic installation not supported on this platform");
    }
}
