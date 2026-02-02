//! System utilities: file descriptor limits and other OS-level checks.

use anyhow::{Result, bail};

/// Minimum recommended file descriptor limit for Cortex operations.
const MIN_RECOMMENDED_FD_LIMIT: u64 = 256;

/// Check file descriptor limits and provide helpful error message if too low.
/// This prevents cryptic "Too many open files" errors during operation.
pub fn check_file_descriptor_limits() -> Result<()> {
    #[cfg(unix)]
    {
        // Try to get current file descriptor limit
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
fn get_fd_limits() -> Result<(u64, u64)> {
    use std::io;

    // Use libc to get resource limits
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

/// Parse model string in provider/model format.
#[derive(Debug, Clone)]
pub struct ModelSpec {
    pub provider: Option<String>,
    pub model: String,
}

impl ModelSpec {
    /// Parse a model string like "anthropic/claude-3-5-sonnet".
    pub fn parse(s: &str) -> Self {
        if let Some((provider, model)) = s.split_once('/') {
            ModelSpec {
                provider: Some(provider.to_string()),
                model: model.to_string(),
            }
        } else {
            ModelSpec {
                provider: None,
                model: s.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_spec_parse() {
        let spec = ModelSpec::parse("anthropic/claude-3-5-sonnet");
        assert_eq!(spec.provider, Some("anthropic".to_string()));
        assert_eq!(spec.model, "claude-3-5-sonnet");

        let spec = ModelSpec::parse("gpt-4");
        assert_eq!(spec.provider, None);
        assert_eq!(spec.model, "gpt-4");
    }
}
