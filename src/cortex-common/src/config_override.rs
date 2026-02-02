//! Configuration override helpers for CLI.

use std::path::PathBuf;

use cortex_protocol::AskForApproval;

/// CLI configuration overrides using raw key=value strings.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "cli", derive(clap::Args))]
pub struct CliConfigOverrides {
    /// Configuration overrides in the form key=value.
    #[cfg_attr(
        feature = "cli",
        arg(short = 'c', long = "config", value_name = "KEY=VALUE")
    )]
    pub raw_overrides: Vec<String>,
}

impl CliConfigOverrides {
    /// Parse the raw overrides into key-value pairs.
    pub fn parse_overrides(&self) -> Result<Vec<(String, toml::Value)>, String> {
        let mut result = Vec::new();
        for raw in &self.raw_overrides {
            let parts: Vec<&str> = raw.splitn(2, '=').collect();
            if parts.len() != 2 {
                return Err(format!(
                    "Invalid override format: {raw}. Expected KEY=VALUE"
                ));
            }
            let key = parts[0].trim().to_string();
            let value_str = parts[1].trim();

            // Try to parse as various TOML types
            let value = if value_str.eq_ignore_ascii_case("true") {
                toml::Value::Boolean(true)
            } else if value_str.eq_ignore_ascii_case("false") {
                toml::Value::Boolean(false)
            } else if let Ok(num) = value_str.parse::<i64>() {
                toml::Value::Integer(num)
            } else if let Ok(num) = value_str.parse::<f64>() {
                toml::Value::Float(num)
            } else {
                toml::Value::String(value_str.to_string())
            };

            result.push((key, value));
        }
        Ok(result)
    }
}

/// CLI configuration overrides.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "cli", derive(clap::Args))]
pub struct ConfigOverride {
    /// Model to use (overrides config file)
    #[cfg_attr(feature = "cli", arg(short, long))]
    pub model: Option<String>,

    /// Model provider (openai, anthropic, etc.)
    #[cfg_attr(feature = "cli", arg(long))]
    pub provider: Option<String>,

    /// Working directory
    #[cfg_attr(feature = "cli", arg(short = 'C', long))]
    pub cwd: Option<PathBuf>,

    /// Full-auto mode (no approval prompts)
    #[cfg_attr(feature = "cli", arg(long))]
    pub full_auto: bool,

    /// Additional writable roots for sandbox
    #[cfg_attr(feature = "cli", arg(long = "writable-root"))]
    pub writable_roots: Vec<PathBuf>,

    /// Disable sandbox entirely (dangerous!)
    #[cfg_attr(feature = "cli", arg(long))]
    pub no_sandbox: bool,
}

impl ConfigOverride {
    /// Get approval policy based on flags.
    pub fn approval_policy(&self) -> Option<AskForApproval> {
        if self.full_auto {
            Some(AskForApproval::Never)
        } else {
            None
        }
    }
}
