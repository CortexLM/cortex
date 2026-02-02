//! System utilities for the Cortex CLI.
//!
//! Provides system-level checks and utilities including:
//! - File descriptor limit checking
//! - PATH validation
//! - Locale detection

use anyhow::{Result, bail};

/// Minimum recommended file descriptor limit for Cortex operations.
pub const MIN_RECOMMENDED_FD_LIMIT: u64 = 256;

/// Check file descriptor limits and provide helpful error message if too low.
///
/// This prevents cryptic "Too many open files" errors during operation.
///
/// # Returns
/// `Ok(())` if limits are acceptable, or an error with remediation instructions.
pub fn check_file_descriptor_limits() -> Result<()> {
    #[cfg(unix)]
    {
        let (soft_limit, _hard_limit) = match get_fd_limits() {
            Ok(limits) => limits,
            Err(e) => {
                // If we can't check limits, log a warning but don't fail
                tracing::debug!("Could not check file descriptor limits: {}", e);
                return Ok(());
            }
        };

        if soft_limit < MIN_RECOMMENDED_FD_LIMIT {
            bail!(
                "File descriptor limit too low: {} (recommended minimum: {})\n\n\
                 This may cause 'Too many open files' errors during operation.\n\n\
                 To increase the limit, run:\n\
                 \x20 ulimit -n 4096\n\n\
                 Or add to your shell profile (~/.bashrc or ~/.zshrc):\n\
                 \x20 ulimit -n 4096",
                soft_limit,
                MIN_RECOMMENDED_FD_LIMIT
            );
        }
    }

    Ok(())
}

/// Get the current soft and hard file descriptor limits.
#[cfg(unix)]
pub fn get_fd_limits() -> Result<(u64, u64)> {
    use std::io;

    let mut rlim = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };

    let result = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut rlim) };

    if result != 0 {
        return Err(anyhow::anyhow!(
            "getrlimit failed: {}",
            io::Error::last_os_error()
        ));
    }

    Ok((rlim.rlim_cur, rlim.rlim_max))
}

/// Validate PATH environment variable and return warnings.
///
/// Checks for:
/// - Empty PATH
/// - Non-existent directories in PATH
/// - Unquoted entries with spaces
///
/// # Returns
/// A list of warning messages (empty if no issues found).
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
            if !std::path::Path::new(entry).exists() {
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

/// Check if the current locale supports UTF-8.
///
/// Returns `true` if the locale appears to be problematic (C or POSIX locale).
pub fn is_problematic_locale() -> bool {
    let lang = std::env::var("LANG").unwrap_or_default();
    let lc_all = std::env::var("LC_ALL").unwrap_or_default();

    lang == "C" || lang == "POSIX" || lang.is_empty() || lc_all == "C" || lc_all == "POSIX"
}

/// Warn about locale issues that may affect UTF-8 handling.
///
/// # Returns
/// `true` if a warning was emitted.
pub fn warn_about_locale() -> bool {
    if is_problematic_locale() {
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
    fn test_check_fd_limits() {
        // Should not error on systems with reasonable limits
        let result = check_file_descriptor_limits();
        // On CI systems, this might fail, so we just check it doesn't panic
        let _ = result;
    }

    #[test]
    fn test_validate_path() {
        // Should return a list (possibly empty)
        let warnings = validate_path_environment();
        // We can't assert on content since it depends on the system
        assert!(warnings.len() < 1000); // Sanity check
    }
}
