//! Shell/Interactive/REPL command for Cortex CLI.
//!
//! Provides an explicit interactive mode entry point:
//! - Start interactive TUI mode
//! - Pass through to main TUI functionality
//! - Serve as an alias for running without a subcommand

use anyhow::{Result, bail};
use clap::Parser;
use std::path::PathBuf;

/// Shell/Interactive CLI command.
///
/// This command provides an explicit way to enter interactive mode.
/// It is equivalent to running `cortex` without any subcommand.
#[derive(Debug, Parser)]
pub struct ShellCli {
    /// Model to use (e.g., claude-sonnet-4-20250514, gpt-4o)
    #[arg(short, long)]
    pub model: Option<String>,

    /// Working directory override
    #[arg(long = "cwd", short = 'C')]
    pub cwd: Option<PathBuf>,

    /// Configuration profile from config.toml
    #[arg(long = "profile", short = 'p')]
    pub config_profile: Option<String>,

    /// Enable web search capability
    #[arg(long = "search")]
    pub web_search: bool,

    /// Initial prompt to start the session with
    #[arg(trailing_var_arg = true)]
    pub prompt: Vec<String>,
}

impl ShellCli {
    /// Run the shell command.
    pub async fn run(self) -> Result<()> {
        use std::io::IsTerminal;

        // Check if stdin/stdout are TTYs
        if !std::io::stdin().is_terminal() {
            bail!(
                "Interactive shell requires a terminal (stdin is not a TTY).\n\
                 For piped input, use 'cortex run' or 'cortex exec' instead."
            );
        }

        if !std::io::stdout().is_terminal() {
            bail!(
                "Interactive shell requires a terminal (stdout is not a TTY).\n\
                 For non-interactive output, use 'cortex run' or 'cortex exec' instead."
            );
        }

        // Build config
        let mut config = cortex_engine::Config::default();

        // Apply model override
        if let Some(ref model) = self.model {
            use cortex_common::resolve_model_alias;
            if model.trim().is_empty() {
                bail!("Model name cannot be empty.");
            }
            config.model = resolve_model_alias(model).to_string();
        }

        // Apply working directory override
        if let Some(ref cwd) = self.cwd {
            let cwd_path = if cwd.is_absolute() {
                cwd.clone()
            } else {
                std::env::current_dir()?.join(cwd)
            };
            std::env::set_current_dir(&cwd_path)?;
            config.cwd = cwd_path;
        }

        // Get initial prompt if provided
        let initial_prompt = if !self.prompt.is_empty() {
            Some(self.prompt.join(" "))
        } else {
            None
        };

        // Initialize custom command registry
        let project_root = std::env::current_dir().ok();
        let _custom_registry = cortex_engine::init_custom_command_registry(
            &config.cortex_home,
            project_root.as_deref(),
        );

        // Scan custom commands
        if let Err(e) = _custom_registry.scan().await {
            tracing::warn!("Failed to scan custom commands: {}", e);
        }

        // Run TUI
        #[cfg(feature = "cortex-tui")]
        {
            cortex_tui::run(config, initial_prompt).await?;
        }

        #[cfg(not(feature = "cortex-tui"))]
        {
            bail!("Interactive mode requires the cortex-tui feature to be enabled.");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    // =========================================================================
    // ShellCli default values tests
    // =========================================================================

    #[test]
    fn test_shell_cli_defaults() {
        let cli = ShellCli::parse_from(["shell"]);

        assert!(cli.model.is_none(), "Model should be None by default");
        assert!(cli.cwd.is_none(), "CWD should be None by default");
        assert!(
            cli.config_profile.is_none(),
            "Config profile should be None by default"
        );
        assert!(!cli.web_search, "Web search should be false by default");
        assert!(cli.prompt.is_empty(), "Prompt should be empty by default");
    }

    // =========================================================================
    // Model argument tests
    // =========================================================================

    #[test]
    fn test_shell_cli_model_short_flag() {
        let cli = ShellCli::parse_from(["shell", "-m", "gpt-4o"]);

        assert_eq!(
            cli.model,
            Some("gpt-4o".to_string()),
            "Model should be set via -m flag"
        );
    }

    #[test]
    fn test_shell_cli_model_long_flag() {
        let cli = ShellCli::parse_from(["shell", "--model", "claude-sonnet-4-20250514"]);

        assert_eq!(
            cli.model,
            Some("claude-sonnet-4-20250514".to_string()),
            "Model should be set via --model flag"
        );
    }

    // =========================================================================
    // Working directory (cwd) argument tests
    // =========================================================================

    #[test]
    fn test_shell_cli_cwd_short_flag() {
        let cli = ShellCli::parse_from(["shell", "-C", "/tmp/project"]);

        assert_eq!(
            cli.cwd,
            Some(PathBuf::from("/tmp/project")),
            "CWD should be set via -C flag"
        );
    }

    #[test]
    fn test_shell_cli_cwd_long_flag() {
        let cli = ShellCli::parse_from(["shell", "--cwd", "/home/user/workspace"]);

        assert_eq!(
            cli.cwd,
            Some(PathBuf::from("/home/user/workspace")),
            "CWD should be set via --cwd flag"
        );
    }

    #[test]
    fn test_shell_cli_cwd_relative_path() {
        let cli = ShellCli::parse_from(["shell", "--cwd", "relative/path"]);

        assert_eq!(
            cli.cwd,
            Some(PathBuf::from("relative/path")),
            "CWD should accept relative paths"
        );
    }

    // =========================================================================
    // Config profile argument tests
    // =========================================================================

    #[test]
    fn test_shell_cli_profile_short_flag() {
        let cli = ShellCli::parse_from(["shell", "-p", "development"]);

        assert_eq!(
            cli.config_profile,
            Some("development".to_string()),
            "Config profile should be set via -p flag"
        );
    }

    #[test]
    fn test_shell_cli_profile_long_flag() {
        let cli = ShellCli::parse_from(["shell", "--profile", "production"]);

        assert_eq!(
            cli.config_profile,
            Some("production".to_string()),
            "Config profile should be set via --profile flag"
        );
    }

    // =========================================================================
    // Web search argument tests
    // =========================================================================

    #[test]
    fn test_shell_cli_web_search_flag() {
        let cli = ShellCli::parse_from(["shell", "--search"]);

        assert!(
            cli.web_search,
            "Web search should be true when --search is passed"
        );
    }

    #[test]
    fn test_shell_cli_web_search_not_present() {
        let cli = ShellCli::parse_from(["shell"]);

        assert!(
            !cli.web_search,
            "Web search should be false when --search is not passed"
        );
    }

    // =========================================================================
    // Trailing prompt argument tests
    // =========================================================================

    #[test]
    fn test_shell_cli_single_word_prompt() {
        let cli = ShellCli::parse_from(["shell", "hello"]);

        assert_eq!(
            cli.prompt,
            vec!["hello".to_string()],
            "Single word prompt should be captured"
        );
    }

    #[test]
    fn test_shell_cli_multi_word_prompt() {
        let cli = ShellCli::parse_from(["shell", "hello", "world", "how", "are", "you"]);

        assert_eq!(
            cli.prompt,
            vec![
                "hello".to_string(),
                "world".to_string(),
                "how".to_string(),
                "are".to_string(),
                "you".to_string()
            ],
            "Multi-word prompt should be captured as separate strings"
        );
    }

    #[test]
    fn test_shell_cli_prompt_with_special_characters() {
        let cli = ShellCli::parse_from(["shell", "What", "is", "2+2?"]);

        assert_eq!(
            cli.prompt,
            vec!["What".to_string(), "is".to_string(), "2+2?".to_string()],
            "Prompt with special characters should be captured"
        );
    }

    // =========================================================================
    // Combined arguments tests
    // =========================================================================

    #[test]
    fn test_shell_cli_combined_flags() {
        let cli = ShellCli::parse_from([
            "shell",
            "-m",
            "gpt-4o",
            "-C",
            "/tmp/project",
            "-p",
            "dev",
            "--search",
        ]);

        assert_eq!(cli.model, Some("gpt-4o".to_string()));
        assert_eq!(cli.cwd, Some(PathBuf::from("/tmp/project")));
        assert_eq!(cli.config_profile, Some("dev".to_string()));
        assert!(cli.web_search);
        assert!(cli.prompt.is_empty());
    }

    #[test]
    fn test_shell_cli_combined_flags_with_prompt() {
        let cli = ShellCli::parse_from([
            "shell",
            "--model",
            "claude-sonnet-4-20250514",
            "--search",
            "explain",
            "this",
            "code",
        ]);

        assert_eq!(cli.model, Some("claude-sonnet-4-20250514".to_string()));
        assert!(cli.web_search);
        assert_eq!(
            cli.prompt,
            vec![
                "explain".to_string(),
                "this".to_string(),
                "code".to_string()
            ]
        );
    }

    #[test]
    fn test_shell_cli_all_options() {
        let cli = ShellCli::parse_from([
            "shell",
            "-m",
            "gpt-4o",
            "--cwd",
            "/workspace",
            "--profile",
            "test",
            "--search",
            "generate",
            "unit",
            "tests",
        ]);

        assert_eq!(cli.model, Some("gpt-4o".to_string()));
        assert_eq!(cli.cwd, Some(PathBuf::from("/workspace")));
        assert_eq!(cli.config_profile, Some("test".to_string()));
        assert!(cli.web_search);
        assert_eq!(
            cli.prompt,
            vec![
                "generate".to_string(),
                "unit".to_string(),
                "tests".to_string()
            ]
        );
    }

    // =========================================================================
    // Debug trait tests
    // =========================================================================

    #[test]
    fn test_shell_cli_debug_impl() {
        let cli = ShellCli::parse_from(["shell", "-m", "gpt-4o"]);

        // Verify Debug trait is implemented and produces output
        let debug_output = format!("{:?}", cli);
        assert!(
            debug_output.contains("ShellCli"),
            "Debug output should contain struct name"
        );
        assert!(
            debug_output.contains("gpt-4o"),
            "Debug output should contain model value"
        );
    }

    // =========================================================================
    // Edge case tests
    // =========================================================================

    #[test]
    fn test_shell_cli_empty_model_string() {
        // clap will accept an empty string - validation happens in run()
        let cli = ShellCli::parse_from(["shell", "-m", ""]);

        assert_eq!(
            cli.model,
            Some(String::new()),
            "Empty model string should be accepted by parser"
        );
    }

    #[test]
    fn test_shell_cli_model_with_dashes() {
        let cli = ShellCli::parse_from(["shell", "-m", "claude-3-5-sonnet-20241022"]);

        assert_eq!(
            cli.model,
            Some("claude-3-5-sonnet-20241022".to_string()),
            "Model with dashes should be parsed correctly"
        );
    }

    #[test]
    fn test_shell_cli_cwd_with_spaces_quoted() {
        let cli = ShellCli::parse_from(["shell", "--cwd", "/path/with spaces/in it"]);

        assert_eq!(
            cli.cwd,
            Some(PathBuf::from("/path/with spaces/in it")),
            "CWD with spaces should be handled"
        );
    }
}
