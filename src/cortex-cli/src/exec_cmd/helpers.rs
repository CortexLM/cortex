//! Helper functions for exec mode flags.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// Read content from the system clipboard.
pub fn read_clipboard() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        let output = Command::new("pbpaste").output()?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            bail!("Failed to read clipboard")
        }
    }

    #[cfg(target_os = "linux")]
    {
        use std::process::Command;
        // Try xclip first, then xsel
        let output = Command::new("xclip")
            .args(["-selection", "clipboard", "-o"])
            .output()
            .or_else(|_| {
                Command::new("xsel")
                    .args(["--clipboard", "--output"])
                    .output()
            })?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            bail!("Failed to read clipboard (tried xclip and xsel)")
        }
    }

    #[cfg(target_os = "windows")]
    {
        use std::process::Command;
        let output = Command::new("powershell")
            .args(["-Command", "Get-Clipboard"])
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            bail!("Failed to read clipboard")
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("Clipboard not supported on this platform")
    }
}

/// Fetch content from a URL.
pub async fn fetch_url_content(url: &str) -> Result<String> {
    // Use curl for fetching
    {
        use std::process::Command;
        let output = Command::new("curl")
            .args(["-sL", "--max-time", "30", url])
            .output()
            .with_context(|| format!("Failed to fetch URL: {}", url))?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("Failed to fetch URL {}: {}", url, stderr)
        }
    }
}

/// Get git diff from the working directory.
pub fn get_git_diff(cwd: &Path) -> Result<String> {
    use std::process::Command;

    let output = Command::new("git")
        .args(["diff", "HEAD"])
        .current_dir(cwd)
        .output()
        .with_context(|| "Failed to run git diff")?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        // Try without HEAD for unstaged changes
        let output = Command::new("git")
            .args(["diff"])
            .current_dir(cwd)
            .output()?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}

/// Collect files matching include/exclude patterns.
pub fn collect_files_by_pattern(
    cwd: &Path,
    include_patterns: &[String],
    exclude_patterns: &[String],
) -> Result<String> {
    use std::process::Command;

    let mut all_files = Vec::new();

    // Use ripgrep's --files mode if available, otherwise find
    for pattern in include_patterns {
        let output = Command::new("find")
            .args([cwd.to_str().unwrap_or("."), "-name", pattern, "-type", "f"])
            .output();

        if let Ok(output) = output
            && output.status.success()
        {
            let files = String::from_utf8_lossy(&output.stdout);
            for file in files.lines() {
                let file_path = PathBuf::from(file);
                let should_exclude = exclude_patterns
                    .iter()
                    .any(|excl| file.contains(excl) || glob_match(excl, file));

                if !should_exclude {
                    all_files.push(file_path);
                }
            }
        }
    }

    // If no include patterns specified, use exclude patterns to filter current directory
    if include_patterns.is_empty() && !exclude_patterns.is_empty() {
        let output = Command::new("find")
            .args([cwd.to_str().unwrap_or("."), "-type", "f"])
            .output();

        if let Ok(output) = output
            && output.status.success()
        {
            let files = String::from_utf8_lossy(&output.stdout);
            for file in files.lines() {
                let should_exclude = exclude_patterns
                    .iter()
                    .any(|excl| file.contains(excl) || glob_match(excl, file));

                if !should_exclude {
                    all_files.push(PathBuf::from(file));
                }
            }
        }
    }

    // Read and combine file contents (limited to prevent huge prompts)
    let mut result = String::new();
    let max_files = 50; // Limit to prevent overly large prompts
    let max_total_size = 100_000; // 100KB limit
    let mut total_size = 0;

    for (i, file_path) in all_files.iter().take(max_files).enumerate() {
        if let Ok(content) = std::fs::read_to_string(file_path) {
            if total_size + content.len() > max_total_size {
                result.push_str(&format!(
                    "\n[Truncated: {} more files not included due to size limit]\n",
                    all_files.len() - i
                ));
                break;
            }

            result.push_str(&format!(
                "--- {} ---\n{}\n--- End {} ---\n\n",
                file_path.display(),
                content,
                file_path.display()
            ));
            total_size += content.len();
        }
    }

    Ok(result)
}

/// Simple glob pattern matching for exclude patterns.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    // Simple implementation - handles * and ** patterns
    if pattern.contains("**") {
        let parts: Vec<&str> = pattern.split("**").collect();
        if parts.len() == 2 {
            let prefix = parts[0].trim_end_matches('/');
            let suffix = parts[1].trim_start_matches('/');
            let prefix_ok = prefix.is_empty() || text.starts_with(prefix);
            let suffix_ok = if suffix.is_empty() {
                true
            } else if let Some(ext) = suffix.strip_prefix('*') {
                // Handle patterns like **/*.rs - suffix is "*.rs"
                text.ends_with(ext)
            } else {
                text.ends_with(suffix)
            };
            return prefix_ok && suffix_ok;
        }
    }

    if pattern.starts_with('*') && pattern.ends_with('*') {
        let middle = &pattern[1..pattern.len() - 1];
        return text.contains(middle);
    }

    if let Some(suffix) = pattern.strip_prefix('*') {
        return text.ends_with(suffix);
    }

    if let Some(prefix) = pattern.strip_suffix('*') {
        return text.starts_with(prefix);
    }

    text == pattern || text.ends_with(&format!("/{}", pattern))
}

/// Validate PATH environment variable and warn about issues.
pub fn validate_path_environment() -> Vec<String> {
    let mut warnings = Vec::new();

    if let Ok(path_var) = std::env::var("PATH") {
        if path_var.is_empty() {
            warnings.push(
                "Warning: PATH environment variable is empty. Commands may not be found."
                    .to_string(),
            );
            return warnings;
        }

        let separator = if cfg!(windows) { ';' } else { ':' };
        for entry in path_var.split(separator) {
            if entry.is_empty() {
                continue;
            }

            // Check for non-existent directories
            if !PathBuf::from(entry).exists() {
                warnings.push(format!(
                    "Warning: PATH contains non-existent directory: {}",
                    entry
                ));
            }

            // Check for entries with spaces (potential issues on some systems)
            if entry.contains(' ') && !entry.starts_with('"') {
                warnings.push(format!(
                    "Warning: PATH contains unquoted entry with spaces: {}",
                    entry
                ));
            }
        }
    } else {
        warnings.push(
            "Warning: PATH environment variable is not set. Commands may not be found.".to_string(),
        );
    }

    warnings
}

/// Ensure UTF-8 locale is set for proper text handling.
/// Returns true if locale was problematic and a warning was emitted.
pub fn ensure_utf8_locale() -> bool {
    // Check if locale is set to C or POSIX
    let lang = std::env::var("LANG").unwrap_or_default();
    let lc_all = std::env::var("LC_ALL").unwrap_or_default();

    let is_c_locale =
        lang == "C" || lang == "POSIX" || lang.is_empty() || lc_all == "C" || lc_all == "POSIX";

    if is_c_locale {
        // Instead of setting env vars (which is unsafe in Rust 2024), just warn the user
        // Setting environment variables at runtime doesn't reliably affect already-running processes anyway
        eprintln!("Warning: Detected C/POSIX locale which may cause UTF-8 encoding issues.");
        eprintln!(
            "Recommendation: Set LANG=en_US.UTF-8 before running cortex for proper UTF-8 support."
        );
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_glob_match() {
        // Test ** patterns
        assert!(glob_match("**/*.rs", "src/main.rs"));
        assert!(glob_match(
            "node_modules/**",
            "node_modules/package/index.js"
        ));

        // Test * prefix patterns
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "path/to/main.rs"));

        // Test * suffix patterns
        assert!(glob_match("test_*", "test_main"));

        // Test exact match
        assert!(glob_match("main.rs", "main.rs"));
        assert!(glob_match("main.rs", "src/main.rs"));
    }

    #[test]
    fn test_validate_path_environment() {
        // This test just checks that the function doesn't panic
        let warnings = validate_path_environment();
        // Just verify we got a result (warnings may or may not be present)
        let _ = warnings;
    }
}
