//! Git utilities for Cortex.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Default timeout for git operations in seconds
const DEFAULT_GIT_TIMEOUT_SECS: u64 = 30;

/// Get the configured git timeout duration
fn get_git_timeout() -> Duration {
    std::env::var("CORTEX_GIT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(Duration::from_secs(DEFAULT_GIT_TIMEOUT_SECS))
}

/// Execute a git command with timeout (synchronous version)
fn run_git_command_with_timeout(
    mut command: Command,
    timeout: Duration,
) -> Option<std::process::Output> {
    let mut child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    let start = Instant::now();

    loop {
        match child.try_wait() {
            Ok(Some(_status)) => {
                return child.wait_with_output().ok();
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                return None;
            }
        }
    }
}

/// Execute a git command with timeout
fn git_command_with_timeout(args: &[&str], cwd: &Path) -> Option<std::process::Output> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(cwd);
    run_git_command_with_timeout(cmd, get_git_timeout())
}

/// Get the current git branch.
pub fn get_current_branch(cwd: &Path) -> Option<String> {
    let output = git_command_with_timeout(&["rev-parse", "--abbrev-ref", "HEAD"], cwd)?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Get the current commit hash.
pub fn get_commit_hash(cwd: &Path) -> Option<String> {
    let output = git_command_with_timeout(&["rev-parse", "HEAD"], cwd)?;

    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        None
    }
}

/// Check if a path is inside a git repository.
pub fn is_git_repo(path: &Path) -> bool {
    let output = git_command_with_timeout(&["rev-parse", "--git-dir"], path);

    matches!(output, Some(o) if o.status.success())
}

/// Get the git repository root.
pub fn get_repo_root(cwd: &Path) -> Option<std::path::PathBuf> {
    let output = git_command_with_timeout(&["rev-parse", "--show-toplevel"], cwd)?;

    if output.status.success() {
        Some(std::path::PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    } else {
        None
    }
}
