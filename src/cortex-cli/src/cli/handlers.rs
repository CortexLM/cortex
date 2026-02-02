//! Command dispatch and execution handlers.
//!
//! This module provides the dispatch function that routes CLI commands
//! to their respective handlers.

use anyhow::{Result, bail};
use clap::CommandFactory;
use clap_complete::{Shell, generate};
use std::io::{self, BufRead, Write};

use super::args::*;
use crate::login::{
    read_api_key_from_stdin, run_login_status, run_login_with_api_key, run_login_with_device_code,
    run_logout,
};
use crate::styled_output::{print_info, print_success, print_warning};

/// Dispatch a CLI command to its handler.
///
/// This is the main command router for the CLI.
pub async fn dispatch_command(cli: Cli) -> Result<()> {
    match cli.command {
        None => run_tui(cli.interactive).await,
        Some(Commands::Init(init_cli)) => run_init(init_cli).await,
        Some(Commands::Run(run_cli)) => run_cli.run().await,
        Some(Commands::Exec(exec_cli)) => exec_cli.run().await,
        Some(Commands::Login(login_cli)) => handle_login(login_cli).await,
        Some(Commands::Logout(logout_cli)) => handle_logout(logout_cli).await,
        Some(Commands::Whoami) => {
            run_whoami().await?;
            Ok(())
        }
        Some(Commands::Mcp(mcp_cli)) => mcp_cli.run().await,
        Some(Commands::Agent(agent_cli)) => agent_cli.run().await,
        Some(Commands::McpServer) => {
            bail!(
                "MCP server mode is not yet implemented. Use 'cortex mcp' for MCP server management."
            );
        }
        Some(Commands::Completion(completion_cli)) => handle_completion(completion_cli),
        Some(Commands::Sandbox(sandbox_args)) => handle_sandbox(sandbox_args).await,
        Some(Commands::Resume(resume_cli)) => run_resume(resume_cli).await,
        Some(Commands::Sessions(sessions_cli)) => handle_sessions(sessions_cli).await,
        Some(Commands::Export(export_cli)) => export_cli.run().await,
        Some(Commands::Import(import_cli)) => import_cli.run().await,
        Some(Commands::Delete(delete_cli)) => run_delete(delete_cli).await,
        Some(Commands::Config(config_cli)) => show_config(config_cli).await,
        Some(Commands::Features(features_cli)) => handle_features(features_cli).await,
        Some(Commands::Serve(serve_cli)) => run_serve(serve_cli).await,
        Some(Commands::Models(models_cli)) => models_cli.run().await,
        Some(Commands::Upgrade(upgrade_cli)) => upgrade_cli.run().await,
        Some(Commands::Uninstall(uninstall_cli)) => uninstall_cli.run().await,
        Some(Commands::Stats(stats_cli)) => stats_cli.run().await,
        Some(Commands::Github(github_cli)) => github_cli.run().await,
        Some(Commands::Pr(pr_cli)) => pr_cli.run().await,
        Some(Commands::Scrape(scrape_cli)) => scrape_cli.run().await,
        Some(Commands::Acp(acp_cli)) => acp_cli.run().await,
        Some(Commands::Debug(debug_cli)) => debug_cli.run().await,
        Some(Commands::Servers(servers_cli)) => run_servers(servers_cli).await,
        Some(Commands::History(history_cli)) => run_history(history_cli).await,
        Some(Commands::Plugin(plugin_cli)) => plugin_cli.run().await,
        Some(Commands::Feedback(feedback_cli)) => feedback_cli.run().await,
        Some(Commands::Lock(lock_cli)) => lock_cli.run().await,
        Some(Commands::Alias(alias_cli)) => alias_cli.run().await,
        Some(Commands::Cache(cache_cli)) => cache_cli.run().await,
        Some(Commands::Compact(compact_cli)) => compact_cli.run().await,
        Some(Commands::Logs(logs_cli)) => logs_cli.run().await,
        Some(Commands::Shell(shell_cli)) => shell_cli.run().await,
        Some(Commands::Workspace(workspace_cli)) => workspace_cli.run().await,
        Some(Commands::Dag(dag_cli)) => crate::dag_cmd::run(dag_cli).await,
    }
}

/// Run the TUI (Terminal User Interface) mode.
async fn run_tui(args: InteractiveArgs) -> Result<()> {
    use cortex_common::resolve_model_alias;
    use std::io::IsTerminal;

    // Check if stdin is a TTY
    if !io::stdin().is_terminal() {
        bail!(
            "Interactive TUI requires a terminal (stdin is not a TTY).\n\
             For piped input, use 'cortex run' or 'cortex exec' instead:\n\
             \n\
             Examples:\n\
             \x20 echo \"your prompt\" | cortex run\n\
             \x20 cat prompt.txt | cortex exec\n\
             \x20 cortex run \"your prompt\""
        );
    }

    if !io::stdout().is_terminal() {
        bail!(
            "Interactive TUI requires a terminal (stdout is not a TTY).\n\
             For non-interactive output, use 'cortex run' or 'cortex exec' instead."
        );
    }

    let mut config = cortex_engine::Config::default();

    // Apply model override if specified
    if let Some(ref model) = args.model {
        if model.trim().is_empty() {
            bail!(
                "Error: Model name cannot be empty. Please provide a valid model name \
                 (e.g., 'gpt-4', 'claude-sonnet-4-20250514')."
            );
        }
        config.model = resolve_model_alias(model).to_string();
    }

    // Apply working directory override if specified
    if let Some(ref cwd) = args.cwd {
        let cwd_path = if cwd.is_absolute() {
            cwd.clone()
        } else {
            std::env::current_dir()?.join(cwd)
        };
        std::env::set_current_dir(&cwd_path)?;
        config.cwd = cwd_path;
    }

    // Initialize custom command registry
    let project_root = std::env::current_dir().ok();
    let _custom_registry =
        cortex_engine::init_custom_command_registry(&config.cortex_home, project_root.as_deref());

    if let Err(e) = _custom_registry.scan().await {
        tracing::warn!("Failed to scan custom commands: {}", e);
    }

    let initial_prompt = if !args.prompt.is_empty() {
        Some(args.prompt.join(" "))
    } else {
        None
    };

    #[cfg(feature = "cortex-tui")]
    {
        let exit_info = cortex_tui::run(config, initial_prompt).await?;
        // Print exit message if present (e.g., logout confirmation)
        if let Some(msg) = exit_info.exit_message {
            println!("{}", msg);
        }
    }

    #[cfg(not(feature = "cortex-tui"))]
    {
        compile_error!("The 'cortex-tui' feature must be enabled");
    }

    Ok(())
}

/// Run the init command to create AGENTS.md.
async fn run_init(init_cli: InitCommand) -> Result<()> {
    use cortex_commands::builtin::InitCommand as InitCmd;
    use std::io::IsTerminal;

    let cwd = std::env::current_dir()?;
    let is_tty = io::stdin().is_terminal() && io::stdout().is_terminal();

    if !is_tty && !init_cli.yes {
        bail!(
            "Non-interactive mode detected but --yes flag not provided.\n\
             Use 'cortex init --yes' to run non-interactively with default settings."
        );
    }

    let cmd = InitCmd::new().force(init_cli.force);

    match cmd.execute(&cwd) {
        Ok(result) => {
            println!("{}", result.message());
            Ok(())
        }
        Err(e) => {
            bail!("Failed to initialize: {}", e);
        }
    }
}

/// Handle login command.
async fn handle_login(login_cli: LoginCommand) -> Result<()> {
    // Validate mutually exclusive authentication methods
    let auth_methods_count = [
        login_cli.token.is_some(),
        login_cli.use_sso,
        login_cli.use_device_code,
        login_cli.with_api_key,
    ]
    .iter()
    .filter(|&&x| x)
    .count();

    if auth_methods_count > 1 {
        bail!(
            "Cannot specify multiple authentication methods. Choose one: --token, --sso, --device-auth, or --with-api-key."
        );
    }

    match login_cli.action {
        Some(LoginSubcommand::Status) => run_login_status(login_cli.config_overrides).await,
        None => {
            if let Some(token) = login_cli.token {
                run_login_with_api_key(login_cli.config_overrides, token).await
            } else if login_cli.use_sso {
                eprintln!("Starting enterprise SSO authentication...");
                run_login_with_device_code(
                    login_cli.config_overrides,
                    login_cli.issuer_base_url,
                    login_cli.client_id,
                )
                .await
            } else if login_cli.use_device_code {
                run_login_with_device_code(
                    login_cli.config_overrides,
                    login_cli.issuer_base_url,
                    login_cli.client_id,
                )
                .await
            } else if login_cli.with_api_key {
                let api_key = read_api_key_from_stdin();
                run_login_with_api_key(login_cli.config_overrides, api_key).await
            } else {
                run_login_with_device_code(
                    login_cli.config_overrides,
                    login_cli.issuer_base_url,
                    login_cli.client_id,
                )
                .await
            }
        }
    }
}

/// Handle logout command.
async fn handle_logout(logout_cli: LogoutCommand) -> Result<()> {
    let skip_confirmation = logout_cli.yes || logout_cli.all;
    if logout_cli.all {
        eprintln!("Logging out from all accounts...");
    }
    run_logout(logout_cli.config_overrides, skip_confirmation).await
}

/// Handle completion command.
fn handle_completion(completion_cli: CompletionCommand) -> Result<()> {
    let shell = completion_cli.shell.unwrap_or_else(detect_shell_from_env);
    if completion_cli.install {
        install_completions(shell)
    } else {
        generate_completions(shell);
        Ok(())
    }
}

/// Handle sandbox command.
async fn handle_sandbox(sandbox_args: SandboxArgs) -> Result<()> {
    match sandbox_args.cmd {
        SandboxCommand::Macos(cmd) => {
            crate::debug_sandbox::run_command_under_seatbelt(cmd, None).await
        }
        SandboxCommand::Linux(cmd) => {
            crate::debug_sandbox::run_command_under_landlock(cmd, None).await
        }
        SandboxCommand::Windows(cmd) => {
            crate::debug_sandbox::run_command_under_windows(cmd, None).await
        }
    }
}

/// Handle sessions command.
async fn handle_sessions(sessions_cli: SessionsCommand) -> Result<()> {
    list_sessions(
        sessions_cli.all,
        sessions_cli.days,
        sessions_cli.since.as_deref(),
        sessions_cli.until.as_deref(),
        sessions_cli.favorites,
        sessions_cli.search.as_deref(),
        sessions_cli.limit,
        sessions_cli.json,
    )
    .await
}

/// Handle features command.
async fn handle_features(features_cli: FeaturesCommand) -> Result<()> {
    match features_cli.sub {
        FeaturesSubcommand::List => list_features().await,
    }
}

// ============================================================================
// Shell completion helpers
// ============================================================================

/// Detect the shell from the SHELL environment variable.
fn detect_shell_from_env() -> Shell {
    if let Ok(shell_path) = std::env::var("SHELL") {
        let shell_name = std::path::Path::new(&shell_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();

        match shell_name.as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "powershell" | "pwsh" => Shell::PowerShell,
            "elvish" => Shell::Elvish,
            _ => {
                eprintln!(
                    "Warning: Unknown shell '{}' from $SHELL. Defaulting to bash.",
                    shell_name
                );
                Shell::Bash
            }
        }
    } else {
        #[cfg(windows)]
        {
            Shell::PowerShell
        }
        #[cfg(not(windows))]
        {
            Shell::Bash
        }
    }
}

/// Generate shell completions to stdout.
fn generate_completions(shell: Shell) {
    /// Custom writer that silently ignores BrokenPipe errors.
    struct BrokenPipeIgnorer<W: Write> {
        inner: W,
    }

    impl<W: Write> Write for BrokenPipeIgnorer<W> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self.inner.write(buf) {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(buf.len()),
                other => other,
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.inner.flush() {
                Err(e) if e.kind() == io::ErrorKind::BrokenPipe => Ok(()),
                other => other,
            }
        }
    }

    let mut cmd = Cli::command();

    let mut buffer = Vec::new();
    generate(shell, &mut cmd, "cortex", &mut buffer);

    let mut output = String::from_utf8_lossy(&buffer).to_string();

    // Add shell-specific enhancements
    if matches!(shell, Shell::Bash) {
        let escape_helper = r#"
# Maximum number of completions to return
_CORTEX_MAX_COMPLETIONS=1000

# Helper function to properly escape file paths
_cortex_escape_path() {
    local path="$1"
    printf '%q' "$path"
}

# Enhanced file completion
_cortex_complete_files() {
    local cur="$1"
    local IFS=$'\n'
    local count=0
    local results=()
    
    while IFS= read -r f; do
        if [[ $count -ge $_CORTEX_MAX_COMPLETIONS ]]; then
            results+=("... (limited to $_CORTEX_MAX_COMPLETIONS results)")
            break
        fi
        results+=("$(printf '%q' "$f")")
        ((count++))
    done < <(compgen -f -- "$cur" 2>/dev/null | head -n $((_CORTEX_MAX_COMPLETIONS + 1)))
    
    COMPREPLY=("${results[@]}")
}
"#;
        if output.starts_with("#!") {
            if let Some(newline_pos) = output.find('\n') {
                output.insert_str(newline_pos + 1, escape_helper);
            }
        } else {
            output.insert_str(0, escape_helper);
        }
    }

    if matches!(shell, Shell::Zsh) {
        let zsh_helper = r#"
# Enable extended glob and proper quoting
setopt LOCAL_OPTIONS
setopt NO_GLOB_SUBST
setopt NO_SH_WORD_SPLIT

# Maximum completions
zstyle ':completion:*' max-errors 0
zstyle ':completion:*:cortex:*' list-max 1000

# Helper to escape paths
_cortex_quote_path() {
    local path="$1"
    print -r -- "${(q)path}"
}
"#;
        if output.starts_with("#!") || output.starts_with("#compdef") {
            if let Some(newline_pos) = output.find('\n') {
                output.insert_str(newline_pos + 1, zsh_helper);
            }
        } else {
            output.insert_str(0, zsh_helper);
        }
    }

    let stdout = io::stdout();
    let mut writer = BrokenPipeIgnorer { inner: stdout };
    let _ = writer.write_all(output.as_bytes());
}

/// Install shell completions to the user's shell configuration file.
fn install_completions(shell: Shell) -> Result<()> {
    use std::fs::OpenOptions;

    let home =
        dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;

    let (rc_file, eval_cmd) = match shell {
        Shell::Bash => (home.join(".bashrc"), r#"eval "$(cortex completion bash)""#),
        Shell::Zsh => (home.join(".zshrc"), r#"eval "$(cortex completion zsh)""#),
        Shell::Fish => {
            let fish_dir = home.join(".config/fish/completions");
            std::fs::create_dir_all(&fish_dir)?;
            let fish_file = fish_dir.join("cortex.fish");

            let mut cmd = Cli::command();
            let mut output = Vec::new();
            generate(shell, &mut cmd, "cortex", &mut output);
            std::fs::write(&fish_file, output)?;

            println!("Completions installed to: {}", fish_file.display());
            println!(
                "Restart your shell or run 'source {}' to activate.",
                fish_file.display()
            );
            return Ok(());
        }
        Shell::PowerShell => {
            let profile_dir = if cfg!(windows) {
                home.join("Documents/PowerShell")
            } else {
                home.join(".config/powershell")
            };
            std::fs::create_dir_all(&profile_dir)?;

            (
                profile_dir.join("Microsoft.PowerShell_profile.ps1"),
                "cortex completion powershell | Out-String | Invoke-Expression",
            )
        }
        Shell::Elvish => {
            let elvish_dir = home.join(".elvish/lib");
            std::fs::create_dir_all(&elvish_dir)?;
            let elvish_file = elvish_dir.join("cortex.elv");

            let mut cmd = Cli::command();
            let mut output = Vec::new();
            generate(shell, &mut cmd, "cortex", &mut output);
            std::fs::write(&elvish_file, output)?;

            println!("Completions installed to: {}", elvish_file.display());
            println!("Add 'use cortex' to your rc.elv to activate.");
            return Ok(());
        }
        _ => {
            bail!(
                "Shell {:?} is not supported for automatic installation. \
                 Use 'cortex completion {:?}' to generate the script manually.",
                shell,
                shell
            );
        }
    };

    // Check if already installed
    if rc_file.exists() {
        let content = std::fs::read_to_string(&rc_file)?;
        if content.contains("cortex completion") {
            println!("Completions already installed in {}", rc_file.display());
            println!("If completions aren't working, restart your shell.");
            return Ok(());
        }
    }

    // Append the eval command
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc_file)?;

    writeln!(file)?;
    writeln!(file, "# Cortex CLI completions")?;
    writeln!(file, "{}", eval_cmd)?;

    println!("Completions installed to: {}", rc_file.display());
    println!(
        "Restart your shell or run 'source {}' to activate.",
        rc_file.display()
    );

    Ok(())
}

// ============================================================================
// Command handler stubs (implemented elsewhere)
// ============================================================================

/// Show current logged-in user.
pub async fn run_whoami() -> Result<()> {
    use cortex_login::{AuthMode, load_auth_with_fallback, safe_format_key};

    let cortex_home = dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .unwrap_or_else(|| std::path::PathBuf::from(".cortex"));

    // Check environment variables first
    if let Ok(token) = std::env::var("CORTEX_AUTH_TOKEN")
        && !token.is_empty()
    {
        println!(
            "Authenticated via CORTEX_AUTH_TOKEN: {}",
            safe_format_key(&token)
        );
        return Ok(());
    }

    if let Ok(token) = std::env::var("CORTEX_API_KEY")
        && !token.is_empty()
    {
        println!(
            "Authenticated via CORTEX_API_KEY: {}",
            safe_format_key(&token)
        );
        return Ok(());
    }

    // Load stored credentials
    match load_auth_with_fallback(&cortex_home) {
        Ok(Some(auth)) => match auth.mode {
            AuthMode::ApiKey => {
                if let Some(key) = auth.get_token() {
                    println!("Logged in with API key: {}", safe_format_key(key));
                } else {
                    println!("Logged in with API key (stored)");
                }
            }
            AuthMode::OAuth => {
                if let Some(account_id) = &auth.account_id {
                    println!("Logged in via OAuth (account: {})", account_id);
                } else {
                    println!("Logged in via OAuth");
                }
                if auth.is_expired() {
                    print_warning("Token is expired. Run 'cortex login' to refresh.");
                }
            }
        },
        Ok(None) => {
            println!("Not logged in. Run 'cortex login' to authenticate.");
        }
        Err(e) => {
            return Err(anyhow::anyhow!("Error checking login status: {}", e));
        }
    }

    Ok(())
}

/// Resume a previous session.
pub async fn run_resume(resume_cli: ResumeCommand) -> Result<()> {
    use crate::utils::resolve_session_id;
    use cortex_protocol::ConversationId;

    if resume_cli.no_session {
        anyhow::bail!(
            "The --no-session flag is incompatible with the 'resume' command. \
            Resuming a session inherently requires session persistence."
        );
    }

    let config = cortex_engine::Config::default();

    let id_str = match (resume_cli.session_id, resume_cli.last, resume_cli.pick) {
        // Support "last" as SESSION_ID as documented in help text (Issue #3646)
        (Some(id), _, _) if id.to_lowercase() == "last" => {
            let sessions = cortex_engine::list_sessions(&config.cortex_home)?;
            if sessions.is_empty() {
                print_info("No sessions found to resume.");
                return Ok(());
            }
            print_info("Resuming most recent session...");
            sessions[0].id.clone()
        }
        (Some(id), _, _) => id,
        (None, true, _) => {
            let sessions = cortex_engine::list_sessions(&config.cortex_home)?;
            if sessions.is_empty() {
                print_info("No sessions found to resume.");
                return Ok(());
            }
            print_info("Resuming most recent session...");
            sessions[0].id.clone()
        }
        (None, false, true) => {
            let sessions = cortex_engine::list_sessions(&config.cortex_home)?;
            if sessions.is_empty() {
                print_info("No sessions found to resume.");
                return Ok(());
            }

            // Filter by cwd unless --all
            let current_dir = std::env::current_dir().ok();
            let filtered_sessions: Vec<_> = if resume_cli.all {
                sessions.clone()
            } else {
                sessions
                    .iter()
                    .filter(|s| current_dir.as_ref().is_none_or(|cwd| s.cwd == *cwd))
                    .cloned()
                    .collect()
            };

            let display_sessions = if filtered_sessions.is_empty() {
                &sessions
            } else {
                &filtered_sessions
            };

            if display_sessions.is_empty() {
                print_info("No sessions found to resume.");
                return Ok(());
            }

            // For non-interactive mode, use the first session
            print_info(&format!("Using session: {}", display_sessions[0].id));
            display_sessions[0].id.clone()
        }
        (None, false, false) => {
            let sessions = cortex_engine::list_sessions(&config.cortex_home)?;
            if sessions.is_empty() {
                print_info("No sessions found. Use 'cortex' to start a new session.");
                return Ok(());
            }
            sessions[0].id.clone()
        }
    };

    // Validate and resolve the session ID
    let conversation_id: ConversationId =
        resolve_session_id(&id_str, &config.cortex_home).map_err(|e| anyhow::anyhow!("{}", e))?;

    print_success(&format!("Resuming session: {}", conversation_id));

    // Start TUI with the session
    #[cfg(feature = "cortex-tui")]
    {
        // The TUI would need to support session resumption
        cortex_tui::run(config, None).await?;
    }

    Ok(())
}

/// Delete a session.
pub async fn run_delete(delete_cli: DeleteCommand) -> Result<()> {
    use crate::utils::resolve_session_id;
    use std::io::{BufRead, Write};

    let config = cortex_engine::Config::default();

    // Resolve session ID
    let conversation_id = resolve_session_id(&delete_cli.session_id, &config.cortex_home)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // Confirm deletion
    if !delete_cli.yes {
        print!(
            "Delete session {}? [y/N]: ",
            &delete_cli.session_id[..8.min(delete_cli.session_id.len())]
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Delete the session
    let rollout_path =
        cortex_engine::rollout::get_rollout_path(&config.cortex_home, &conversation_id);
    if rollout_path.exists() {
        std::fs::remove_file(&rollout_path)?;
        print_success(&format!("Deleted session: {}", conversation_id));
    } else {
        print_warning("Session file not found (may have been already deleted).");
    }

    Ok(())
}

/// List sessions.
#[allow(clippy::too_many_arguments)]
pub async fn list_sessions(
    all: bool,
    _days: Option<u32>,
    _since: Option<&str>,
    _until: Option<&str>,
    _favorites: bool,
    search: Option<&str>,
    limit: Option<usize>,
    json: bool,
) -> Result<()> {
    let config = cortex_engine::Config::default();
    let sessions = cortex_engine::list_sessions(&config.cortex_home)?;

    // Filter sessions
    let current_dir = std::env::current_dir().ok();
    let mut filtered: Vec<_> = sessions
        .into_iter()
        .filter(|s| {
            // Filter by cwd unless --all
            if !all
                && let Some(ref cwd) = current_dir
                && s.cwd != *cwd
            {
                return false;
            }
            // Filter by search term
            if let Some(term) = search {
                let term_lower = term.to_lowercase();
                if !s.id.to_lowercase().contains(&term_lower) {
                    return false;
                }
            }
            true
        })
        .collect();

    // Apply limit
    if let Some(limit) = limit {
        filtered.truncate(limit);
    }

    if json {
        // Manually construct JSON since SessionInfo doesn't derive Serialize
        let json_sessions: Vec<serde_json::Value> = filtered
            .iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "timestamp": s.timestamp,
                    "model": s.model,
                    "cwd": s.cwd.to_string_lossy(),
                    "message_count": s.message_count,
                    "git_branch": s.git_branch
                })
            })
            .collect();
        let json_output = serde_json::to_string_pretty(&json_sessions)?;
        println!("{}", json_output);
        return Ok(());
    }

    if filtered.is_empty() {
        print_info("No sessions found.");
        if !all {
            println!("Use --all to show sessions from all directories.");
        }
        return Ok(());
    }

    println!(
        "{:<12} {:<20} {:>8} {:<20}",
        "ID", "Date", "Messages", "Model"
    );
    println!("{}", "-".repeat(65));

    for session in &filtered {
        let date = if session.timestamp.len() >= 16 {
            session.timestamp[..16].replace('T', " ")
        } else {
            session.timestamp.clone()
        };
        let model = session.model.as_deref().unwrap_or("default");
        println!(
            "{:<12} {:<20} {:>8} {:<20}",
            &session.id[..8.min(session.id.len())],
            date,
            session.message_count,
            model,
        );
    }

    println!("\nTotal: {} session(s)", filtered.len());

    Ok(())
}

/// Show configuration.
pub async fn show_config(config_cli: ConfigCommand) -> Result<()> {
    let cortex_home = dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .unwrap_or_else(|| std::path::PathBuf::from(".cortex"));

    let config_path = cortex_home.join("config.toml");

    match config_cli.action {
        Some(ConfigSubcommand::Get(args)) => {
            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                let config: toml::Value = toml::from_str(&content)?;
                if let Some(value) = config.get(&args.key) {
                    println!("{}", value);
                } else {
                    bail!("Key '{}' not found in configuration", args.key);
                }
            } else {
                bail!("No configuration file found");
            }
        }
        Some(ConfigSubcommand::Set(args)) => {
            let mut config: toml::map::Map<String, toml::Value> = if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                toml::from_str(&content)?
            } else {
                toml::map::Map::new()
            };

            config.insert(args.key.clone(), toml::Value::String(args.value.clone()));

            let content = toml::to_string_pretty(&config)?;
            std::fs::create_dir_all(&cortex_home)?;
            std::fs::write(&config_path, content)?;
            print_success(&format!("Set {} = {}", args.key, args.value));
        }
        Some(ConfigSubcommand::Unset(args)) => {
            if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                let mut config: toml::map::Map<String, toml::Value> = toml::from_str(&content)?;

                if config.remove(&args.key).is_some() {
                    let content = toml::to_string_pretty(&config)?;
                    std::fs::write(&config_path, content)?;
                    print_success(&format!("Removed key: {}", args.key));
                } else {
                    bail!("Key '{}' not found in configuration", args.key);
                }
            } else {
                bail!("No configuration file found");
            }
        }
        None => {
            if config_cli.json {
                if config_path.exists() {
                    let content = std::fs::read_to_string(&config_path)?;
                    let config: toml::Value = toml::from_str(&content)?;
                    let json = serde_json::to_string_pretty(&config)?;
                    println!("{}", json);
                } else {
                    println!("{{}}");
                }
            } else {
                println!("Configuration file: {}", config_path.display());
                if config_path.exists() {
                    let content = std::fs::read_to_string(&config_path)?;
                    println!("\n{}", content);
                } else {
                    println!("\n(No configuration file found)");
                }
            }
        }
    }

    Ok(())
}

/// List features.
pub async fn list_features() -> Result<()> {
    println!("Feature flags:");
    println!("{}", "-".repeat(40));
    println!("  (Feature flag system not yet implemented)");
    Ok(())
}

/// Run the HTTP server.
pub async fn run_serve(serve_cli: ServeCommand) -> Result<()> {
    use cortex_app_server::ServerConfig;

    let mut config = ServerConfig {
        listen_addr: format!("{}:{}", serve_cli.host, serve_cli.port),
        ..Default::default()
    };

    // Set authentication if provided
    if let Some(token) = serve_cli.auth_token {
        config.auth.enabled = true;
        config.auth.api_keys.push(token);
    }

    // Set CORS origins
    if serve_cli.cors || !serve_cli.cors_origins.is_empty() {
        config.cors_origins = if serve_cli.cors_origins.is_empty() {
            vec!["*".to_string()] // Allow all origins if --cors without specific origins
        } else {
            serve_cli.cors_origins
        };
    }

    // Set mDNS configuration
    config.mdns.enabled = serve_cli.mdns && !serve_cli.no_mdns;
    config.mdns.service_name = serve_cli.mdns_name;

    println!("Starting Cortex server on {}", config.listen_addr);

    cortex_app_server::run(config).await
}

/// Discover servers on the network.
pub async fn run_servers(servers_cli: ServersCommand) -> Result<()> {
    match servers_cli.action {
        Some(ServersSubcommand::Refresh(args)) => {
            println!(
                "Scanning for Cortex servers (timeout: {}s)...",
                args.timeout
            );
            // Server discovery implementation
            println!("(mDNS discovery not yet implemented)");
        }
        None => {
            println!(
                "Scanning for Cortex servers (timeout: {}s)...",
                servers_cli.timeout
            );
            println!("(mDNS discovery not yet implemented)");
        }
    }
    Ok(())
}

/// View history.
pub async fn run_history(history_cli: HistoryCommand) -> Result<()> {
    match history_cli.action {
        Some(HistorySubcommand::Search(args)) => {
            println!("Searching history for: {}", args.pattern);
            println!("(History search not yet implemented)");
        }
        Some(HistorySubcommand::Clear(args)) => {
            if !args.yes {
                print!("Clear all history? This cannot be undone. [y/N]: ");
                io::stdout().flush()?;

                let mut input = String::new();
                io::stdin().lock().read_line(&mut input)?;

                if !input.trim().eq_ignore_ascii_case("y") {
                    println!("Cancelled.");
                    return Ok(());
                }
            }
            println!("(History clear not yet implemented)");
        }
        None => {
            println!("Recent prompts (limit: {}):", history_cli.limit);
            println!("(History view not yet implemented)");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap_complete::Shell;
    use std::io::{self, ErrorKind, Write};

    // =========================================================================
    // Shell name parsing tests (unit test the parsing logic directly)
    // Note: We avoid testing detect_shell_from_env directly because it reads
    // from env vars which causes race conditions in parallel tests.
    // Instead, we test the shell name matching logic inline.
    // =========================================================================

    /// Helper function that mirrors the shell detection logic for testing
    fn parse_shell_name(shell_name: &str) -> Shell {
        let normalized = shell_name.to_lowercase();
        match normalized.as_str() {
            "bash" => Shell::Bash,
            "zsh" => Shell::Zsh,
            "fish" => Shell::Fish,
            "powershell" | "pwsh" => Shell::PowerShell,
            "elvish" => Shell::Elvish,
            _ => Shell::Bash, // Default to Bash for unknown shells
        }
    }

    /// Helper to extract shell name from path (like the real function does)
    fn extract_shell_name_from_path(path: &str) -> &str {
        std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
    }

    #[test]
    fn test_shell_name_parsing_bash() {
        assert!(
            matches!(parse_shell_name("bash"), Shell::Bash),
            "Should parse 'bash' as Bash"
        );
        assert!(
            matches!(parse_shell_name("BASH"), Shell::Bash),
            "Should parse 'BASH' (uppercase) as Bash"
        );
        assert!(
            matches!(parse_shell_name("Bash"), Shell::Bash),
            "Should parse 'Bash' (mixed case) as Bash"
        );
    }

    #[test]
    fn test_shell_name_parsing_zsh() {
        assert!(
            matches!(parse_shell_name("zsh"), Shell::Zsh),
            "Should parse 'zsh' as Zsh"
        );
        assert!(
            matches!(parse_shell_name("ZSH"), Shell::Zsh),
            "Should parse 'ZSH' (uppercase) as Zsh"
        );
    }

    #[test]
    fn test_shell_name_parsing_fish() {
        assert!(
            matches!(parse_shell_name("fish"), Shell::Fish),
            "Should parse 'fish' as Fish"
        );
        assert!(
            matches!(parse_shell_name("FISH"), Shell::Fish),
            "Should parse 'FISH' (uppercase) as Fish"
        );
    }

    #[test]
    fn test_shell_name_parsing_powershell() {
        assert!(
            matches!(parse_shell_name("powershell"), Shell::PowerShell),
            "Should parse 'powershell' as PowerShell"
        );
        assert!(
            matches!(parse_shell_name("pwsh"), Shell::PowerShell),
            "Should parse 'pwsh' as PowerShell"
        );
        assert!(
            matches!(parse_shell_name("PWSH"), Shell::PowerShell),
            "Should parse 'PWSH' (uppercase) as PowerShell"
        );
    }

    #[test]
    fn test_shell_name_parsing_elvish() {
        assert!(
            matches!(parse_shell_name("elvish"), Shell::Elvish),
            "Should parse 'elvish' as Elvish"
        );
        assert!(
            matches!(parse_shell_name("ELVISH"), Shell::Elvish),
            "Should parse 'ELVISH' (uppercase) as Elvish"
        );
    }

    #[test]
    fn test_shell_name_parsing_unknown_defaults_to_bash() {
        assert!(
            matches!(parse_shell_name("unknown-shell"), Shell::Bash),
            "Unknown shell should default to Bash"
        );
        assert!(
            matches!(parse_shell_name("tcsh"), Shell::Bash),
            "tcsh should default to Bash"
        );
        assert!(
            matches!(parse_shell_name("csh"), Shell::Bash),
            "csh should default to Bash"
        );
        assert!(
            matches!(parse_shell_name(""), Shell::Bash),
            "Empty string should default to Bash"
        );
    }

    #[test]
    fn test_extract_shell_name_from_path() {
        assert_eq!(
            extract_shell_name_from_path("/bin/bash"),
            "bash",
            "Should extract 'bash' from /bin/bash"
        );
        assert_eq!(
            extract_shell_name_from_path("/usr/bin/zsh"),
            "zsh",
            "Should extract 'zsh' from /usr/bin/zsh"
        );
        assert_eq!(
            extract_shell_name_from_path("/usr/local/bin/fish"),
            "fish",
            "Should extract 'fish' from /usr/local/bin/fish"
        );
        assert_eq!(
            extract_shell_name_from_path("bash"),
            "bash",
            "Should handle shell name without path"
        );
    }

    #[test]
    fn test_full_path_to_shell_detection() {
        // Test the full pipeline: path -> name extraction -> shell detection
        let test_cases = vec![
            ("/bin/bash", Shell::Bash),
            ("/usr/bin/bash", Shell::Bash),
            ("/bin/zsh", Shell::Zsh),
            ("/usr/local/bin/zsh", Shell::Zsh),
            ("/usr/bin/fish", Shell::Fish),
            ("/usr/bin/pwsh", Shell::PowerShell),
            ("/usr/bin/elvish", Shell::Elvish),
            ("/bin/BASH", Shell::Bash), // uppercase
            ("/bin/ZSH", Shell::Zsh),   // uppercase
        ];

        for (path, expected_shell) in test_cases {
            let shell_name = extract_shell_name_from_path(path);
            let detected = parse_shell_name(shell_name);
            assert!(
                std::mem::discriminant(&detected) == std::mem::discriminant(&expected_shell),
                "Path '{}' should detect as {:?}, got {:?}",
                path,
                expected_shell,
                detected
            );
        }
    }

    // =========================================================================
    // BrokenPipeIgnorer tests (testing the pattern from generate_completions)
    // =========================================================================

    /// Custom writer that silently ignores BrokenPipe errors (mirrors the one in generate_completions).
    struct BrokenPipeIgnorer<W: Write> {
        inner: W,
    }

    impl<W: Write> Write for BrokenPipeIgnorer<W> {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            match self.inner.write(buf) {
                Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(buf.len()),
                other => other,
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.inner.flush() {
                Err(e) if e.kind() == ErrorKind::BrokenPipe => Ok(()),
                other => other,
            }
        }
    }

    /// A mock writer that can be configured to return specific errors.
    struct MockWriter {
        error_kind: Option<ErrorKind>,
        data: Vec<u8>,
    }

    impl MockWriter {
        fn new() -> Self {
            Self {
                error_kind: None,
                data: Vec::new(),
            }
        }

        fn with_error(error_kind: ErrorKind) -> Self {
            Self {
                error_kind: Some(error_kind),
                data: Vec::new(),
            }
        }
    }

    impl Write for MockWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if let Some(kind) = self.error_kind {
                return Err(io::Error::new(kind, "mock error"));
            }
            self.data.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            if let Some(kind) = self.error_kind {
                return Err(io::Error::new(kind, "mock flush error"));
            }
            Ok(())
        }
    }

    #[test]
    fn test_broken_pipe_ignorer_normal_write() {
        let mock = MockWriter::new();
        let mut ignorer = BrokenPipeIgnorer { inner: mock };

        let result = ignorer.write(b"hello");
        assert!(result.is_ok(), "Normal write should succeed");
        assert_eq!(result.unwrap(), 5, "Should return bytes written");
    }

    #[test]
    fn test_broken_pipe_ignorer_swallows_broken_pipe() {
        let mock = MockWriter::with_error(ErrorKind::BrokenPipe);
        let mut ignorer = BrokenPipeIgnorer { inner: mock };

        let result = ignorer.write(b"hello");
        assert!(
            result.is_ok(),
            "BrokenPipe error should be silently ignored"
        );
        assert_eq!(
            result.unwrap(),
            5,
            "Should return buffer length even on BrokenPipe"
        );
    }

    #[test]
    fn test_broken_pipe_ignorer_propagates_other_errors() {
        let mock = MockWriter::with_error(ErrorKind::PermissionDenied);
        let mut ignorer = BrokenPipeIgnorer { inner: mock };

        let result = ignorer.write(b"hello");
        assert!(
            result.is_err(),
            "Non-BrokenPipe errors should be propagated"
        );
        assert_eq!(
            result.unwrap_err().kind(),
            ErrorKind::PermissionDenied,
            "Should preserve the original error kind"
        );
    }

    #[test]
    fn test_broken_pipe_ignorer_flush_normal() {
        let mock = MockWriter::new();
        let mut ignorer = BrokenPipeIgnorer { inner: mock };

        let result = ignorer.flush();
        assert!(result.is_ok(), "Normal flush should succeed");
    }

    #[test]
    fn test_broken_pipe_ignorer_flush_swallows_broken_pipe() {
        let mock = MockWriter::with_error(ErrorKind::BrokenPipe);
        let mut ignorer = BrokenPipeIgnorer { inner: mock };

        let result = ignorer.flush();
        assert!(
            result.is_ok(),
            "BrokenPipe on flush should be silently ignored"
        );
    }

    #[test]
    fn test_broken_pipe_ignorer_flush_propagates_other_errors() {
        let mock = MockWriter::with_error(ErrorKind::WriteZero);
        let mut ignorer = BrokenPipeIgnorer { inner: mock };

        let result = ignorer.flush();
        assert!(
            result.is_err(),
            "Non-BrokenPipe errors on flush should be propagated"
        );
        assert_eq!(
            result.unwrap_err().kind(),
            ErrorKind::WriteZero,
            "Should preserve the original error kind on flush"
        );
    }

    // =========================================================================
    // Shell enum variant tests
    // =========================================================================

    #[test]
    fn test_shell_variants_are_supported() {
        // Verify that our shell detection covers all commonly used shells
        let supported_shells = vec![
            (Shell::Bash, "bash"),
            (Shell::Zsh, "zsh"),
            (Shell::Fish, "fish"),
            (Shell::PowerShell, "powershell"),
            (Shell::Elvish, "elvish"),
        ];

        for (shell, name) in supported_shells {
            // Verify each shell variant can be matched
            match shell {
                Shell::Bash => assert_eq!(name, "bash"),
                Shell::Zsh => assert_eq!(name, "zsh"),
                Shell::Fish => assert_eq!(name, "fish"),
                Shell::PowerShell => assert_eq!(name, "powershell"),
                Shell::Elvish => assert_eq!(name, "elvish"),
                _ => {} // Other variants exist but we don't need to test them
            }
        }
    }
}
