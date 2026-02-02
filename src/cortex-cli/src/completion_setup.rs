//! First-run shell completion setup module.
//!
//! This module handles prompting users to install shell completions on first run.
//! It detects the user's shell, offers to install completions, and tracks whether
//! the prompt has been shown to avoid repeated prompts.

use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use clap_complete::Shell;

/// Marker file name to track if completion setup has been offered.
const COMPLETION_OFFERED_MARKER: &str = ".completion_offered";

/// Check if this is an interactive terminal.
fn is_interactive_terminal() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

/// Get the cortex home directory.
fn get_cortex_home() -> Option<PathBuf> {
    // Check CORTEX_HOME or CORTEX_CONFIG_DIR environment variables
    if let Ok(val) = std::env::var("CORTEX_CONFIG_DIR")
        && !val.is_empty()
    {
        return Some(PathBuf::from(val));
    }
    if let Ok(val) = std::env::var("CORTEX_HOME")
        && !val.is_empty()
    {
        return Some(PathBuf::from(val));
    }

    // Default to ~/.cortex
    dirs::home_dir().map(|h| h.join(".cortex"))
}

/// Check if completion setup has already been offered.
fn completion_already_offered(cortex_home: &Path) -> bool {
    cortex_home.join(COMPLETION_OFFERED_MARKER).exists()
}

/// Mark that completion setup has been offered.
fn mark_completion_offered(cortex_home: &Path) -> io::Result<()> {
    // Ensure the directory exists
    fs::create_dir_all(cortex_home)?;

    // Create the marker file
    let marker_path = cortex_home.join(COMPLETION_OFFERED_MARKER);
    fs::write(&marker_path, "completion setup offered\n")?;
    Ok(())
}

/// Detect the user's shell from the SHELL environment variable.
fn detect_shell() -> Option<Shell> {
    let shell_path = std::env::var("SHELL").ok()?;
    let shell_name = std::path::Path::new(&shell_path)
        .file_name()?
        .to_str()?
        .to_lowercase();

    match shell_name.as_str() {
        "bash" => Some(Shell::Bash),
        "zsh" => Some(Shell::Zsh),
        "fish" => Some(Shell::Fish),
        "powershell" | "pwsh" => Some(Shell::PowerShell),
        "elvish" => Some(Shell::Elvish),
        _ => None,
    }
}

/// Get the completion installation path for a given shell.
fn get_completion_install_path(shell: Shell) -> Option<PathBuf> {
    let home = dirs::home_dir()?;

    match shell {
        Shell::Bash => {
            // Try bash-completion directory first, then fall back to .bashrc
            let bash_completion = home.join(".local/share/bash-completion/completions");
            if bash_completion.parent().is_some_and(|p| p.exists()) {
                Some(bash_completion.join("cortex"))
            } else {
                Some(home.join(".bashrc"))
            }
        }
        Shell::Zsh => {
            // Check for common zsh completion directories
            let zsh_completions = home.join(".zsh/completions");
            if zsh_completions.exists() {
                Some(zsh_completions.join("_cortex"))
            } else {
                Some(home.join(".zshrc"))
            }
        }
        Shell::Fish => Some(home.join(".config/fish/completions/cortex.fish")),
        Shell::PowerShell => {
            // PowerShell profile location varies by platform
            #[cfg(windows)]
            {
                Some(home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1"))
            }
            #[cfg(not(windows))]
            {
                Some(home.join(".config/powershell/Microsoft.PowerShell_profile.ps1"))
            }
        }
        Shell::Elvish => Some(home.join(".elvish/lib/cortex.elv")),
        _ => None,
    }
}

/// Get the shell name in lowercase for command output.
fn shell_name(shell: Shell) -> &'static str {
    match shell {
        Shell::Bash => "bash",
        Shell::Zsh => "zsh",
        Shell::Fish => "fish",
        Shell::PowerShell => "powershell",
        Shell::Elvish => "elvish",
        _ => "bash",
    }
}

/// Install completions for the given shell.
///
/// For most shells, we add an eval command to the shell configuration file
/// that dynamically loads completions. This approach is more robust as it
/// doesn't require updating the completion script when the CLI changes.
fn install_completions(shell: Shell) -> io::Result<()> {
    let install_path = get_completion_install_path(shell).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "Could not determine completion installation path",
        )
    })?;

    // Create parent directory if needed
    if let Some(parent) = install_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let shell_cmd = shell_name(shell);

    match shell {
        Shell::Bash => {
            // Append eval command to .bashrc
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&install_path)?;

            writeln!(file)?;
            writeln!(file, "# Cortex CLI completions")?;
            writeln!(file, "eval \"$(cortex completion {})\"\n", shell_cmd)?;
        }
        Shell::Zsh => {
            // Append eval command to .zshrc
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&install_path)?;

            writeln!(file)?;
            writeln!(file, "# Cortex CLI completions")?;
            writeln!(file, "eval \"$(cortex completion {})\"\n", shell_cmd)?;
        }
        Shell::Fish => {
            // Fish needs the script written to the completions directory
            // Generate it by running the command
            let output = std::process::Command::new("cortex")
                .args(["completion", shell_cmd])
                .output()?;

            if output.status.success() {
                fs::write(&install_path, output.stdout)?;
            } else {
                return Err(io::Error::other("Failed to generate fish completions"));
            }
        }
        Shell::PowerShell => {
            // Append to PowerShell profile
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&install_path)?;

            writeln!(file)?;
            writeln!(file, "# Cortex CLI completions")?;
            writeln!(
                file,
                "cortex completion powershell | Out-String | Invoke-Expression\n"
            )?;
        }
        Shell::Elvish => {
            // Generate script by running the command
            let output = std::process::Command::new("cortex")
                .args(["completion", shell_cmd])
                .output()?;

            if output.status.success() {
                fs::write(&install_path, output.stdout)?;
            } else {
                return Err(io::Error::other("Failed to generate elvish completions"));
            }
        }
        _ => {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "Unsupported shell for automatic installation",
            ));
        }
    }

    Ok(())
}

/// Maximum number of completions to return for file/directory listings.
/// This prevents shell hangs when completing in directories with many files.
pub const MAX_COMPLETION_RESULTS: usize = 1000;

/// Prompt the user to install shell completions on first run.
///
/// This function checks if:
/// 1. We're running in an interactive terminal
/// 2. Completion setup hasn't been offered before
/// 3. We can detect the user's shell
///
/// If all conditions are met, it prompts the user and optionally installs completions.
///
/// Note: For large directories (>1000 files), completion may be slow.
/// Consider using more specific paths or limiting directory size.
pub fn maybe_prompt_completion_setup() {
    // Only prompt in interactive terminals
    if !is_interactive_terminal() {
        return;
    }

    // Get cortex home directory
    let cortex_home = match get_cortex_home() {
        Some(home) => home,
        None => return,
    };

    // Check if we've already offered completion setup
    if completion_already_offered(&cortex_home) {
        return;
    }

    // Detect the user's shell
    let shell = match detect_shell() {
        Some(s) => s,
        None => {
            // Mark as offered so we don't keep trying with unknown shells
            let _ = mark_completion_offered(&cortex_home);
            return;
        }
    };

    let shell_str = shell_name(shell);

    // Show the prompt
    println!();
    println!("Welcome to Cortex CLI!");
    println!();
    println!("Would you like to enable tab completion for your shell ({shell_str})?",);
    println!("This will allow you to press TAB to complete commands and options.");
    println!();
    print!("Enable tab completion? [y/N] ");
    let _ = io::stdout().flush();

    // Read user input
    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_err() {
        let _ = mark_completion_offered(&cortex_home);
        return;
    }

    let response = input.trim().to_lowercase();
    if response == "y" || response == "yes" {
        match install_completions(shell) {
            Ok(()) => {
                println!();
                println!("Tab completion has been enabled!");
                println!(
                    "Please restart your shell or run 'source ~/.{}rc' to activate.",
                    shell_str
                );
                println!();
            }
            Err(e) => {
                println!();
                println!("Could not automatically install completions: {e}");
                println!();
                println!("You can manually enable completions by running:");
                println!("  cortex completion {shell_str}");
                println!();
                println!("Then add the output to your shell configuration file.");
                println!();
            }
        }
    } else {
        println!();
        println!("No problem! You can enable tab completion later by running:");
        println!("  cortex completion {shell_str}");
        println!();
    }

    // Mark as offered regardless of the user's choice
    let _ = mark_completion_offered(&cortex_home);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn test_shell_name_bash() {
        assert_eq!(shell_name(Shell::Bash), "bash");
    }

    #[test]
    fn test_shell_name_zsh() {
        assert_eq!(shell_name(Shell::Zsh), "zsh");
    }

    #[test]
    fn test_shell_name_fish() {
        assert_eq!(shell_name(Shell::Fish), "fish");
    }

    #[test]
    fn test_shell_name_powershell() {
        assert_eq!(shell_name(Shell::PowerShell), "powershell");
    }

    #[test]
    fn test_shell_name_elvish() {
        assert_eq!(shell_name(Shell::Elvish), "elvish");
    }

    #[test]
    #[serial]
    fn test_detect_shell_bash() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("SHELL", "/bin/bash") };
        let shell = detect_shell();
        assert_eq!(shell, Some(Shell::Bash));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_zsh() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/zsh") };
        let shell = detect_shell();
        assert_eq!(shell, Some(Shell::Zsh));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_fish() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("SHELL", "/usr/bin/fish") };
        let shell = detect_shell();
        assert_eq!(shell, Some(Shell::Fish));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_powershell() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("SHELL", "/usr/bin/pwsh") };
        let shell = detect_shell();
        assert_eq!(shell, Some(Shell::PowerShell));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_case_insensitive() {
        // SAFETY: Tests run serially and we restore env vars immediately
        // Test that shell detection converts to lowercase (BASH -> bash -> Shell::Bash)
        unsafe { std::env::set_var("SHELL", "/bin/BASH") };
        let shell = detect_shell();
        // The function lowercases the shell name, so BASH becomes bash
        assert_eq!(shell, Some(Shell::Bash));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_elvish() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("SHELL", "/usr/local/bin/elvish") };
        let shell = detect_shell();
        assert_eq!(shell, Some(Shell::Elvish));
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_unknown() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::set_var("SHELL", "/bin/unknown_shell") };
        let shell = detect_shell();
        assert_eq!(shell, None);
        unsafe { std::env::remove_var("SHELL") };
    }

    #[test]
    #[serial]
    fn test_detect_shell_no_shell_env() {
        // SAFETY: Tests run serially and we restore env vars immediately
        unsafe { std::env::remove_var("SHELL") };
        let shell = detect_shell();
        assert_eq!(shell, None);
    }

    #[test]
    fn test_get_completion_install_path_bash() {
        let path = get_completion_install_path(Shell::Bash);
        assert!(path.is_some());
        let path = path.expect("bash completion path should be available");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with("bash-completion/completions/cortex")
                || path_str.ends_with(".bashrc"),
            "bash path should end with bash-completion/completions/cortex or .bashrc"
        );
    }

    #[test]
    fn test_get_completion_install_path_zsh() {
        let path = get_completion_install_path(Shell::Zsh);
        assert!(path.is_some());
        let path = path.expect("zsh completion path should be available");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.ends_with("_cortex") || path_str.ends_with(".zshrc"),
            "zsh path should end with _cortex or .zshrc"
        );
    }

    #[test]
    fn test_get_completion_install_path_fish() {
        let path = get_completion_install_path(Shell::Fish);
        assert!(path.is_some());
        let path = path.expect("fish completion path should be available");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("fish/completions/cortex.fish"),
            "fish path should contain fish/completions/cortex.fish"
        );
    }

    #[test]
    fn test_get_completion_install_path_powershell() {
        let path = get_completion_install_path(Shell::PowerShell);
        assert!(path.is_some());
        let path = path.expect("powershell completion path should be available");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("PowerShell") || path_str.contains("powershell"),
            "powershell path should contain PowerShell or powershell"
        );
    }

    #[test]
    fn test_get_completion_install_path_elvish() {
        let path = get_completion_install_path(Shell::Elvish);
        assert!(path.is_some());
        let path = path.expect("elvish completion path should be available");
        let path_str = path.to_string_lossy();
        assert!(
            path_str.contains("elvish") && path_str.ends_with("cortex.elv"),
            "elvish path should contain elvish and end with cortex.elv"
        );
    }
}
