//! GitHub integration commands.
//!
//! Provides commands for GitHub Actions integration:
//! - `cortex github install` - Install GitHub Actions workflow
//! - `cortex github run` - Run GitHub agent in Actions context
//! - `cortex github status` - Check installation status

use crate::styled_output::{print_error, print_success, print_warning};
use anyhow::{Context, Result, bail};
use clap::Parser;
use std::path::PathBuf;

/// GitHub integration CLI.
#[derive(Debug, Parser)]
pub struct GitHubCli {
    #[command(subcommand)]
    pub subcommand: GitHubSubcommand,
}

/// GitHub subcommands.
#[derive(Debug, clap::Subcommand)]
pub enum GitHubSubcommand {
    /// Install GitHub Actions workflow for Cortex CI/CD automation.
    Install(InstallArgs),

    /// Run GitHub agent in Actions context.
    Run(RunArgs),

    /// Check GitHub Actions installation status.
    Status(StatusArgs),

    /// Uninstall/remove the Cortex GitHub workflow.
    Uninstall(UninstallArgs),

    /// Update the Cortex GitHub workflow to the latest version.
    Update(UpdateArgs),
}

/// Arguments for install command.
#[derive(Debug, Parser)]
pub struct InstallArgs {
    /// Path to the repository root (defaults to current directory).
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Force overwrite existing workflow file.
    #[arg(short, long)]
    pub force: bool,

    /// Include PR review automation in the workflow.
    /// When enabled, the agent will:
    ///   - Automatically review new and updated pull requests
    ///   - Analyze code changes for bugs, security issues, and best practices
    ///   - Suggest improvements with inline comments
    ///   - Respond to review comments and questions
    ///
    /// Triggered by: pull_request (opened, synchronize, reopened)
    #[arg(long, default_value_t = true)]
    pub pr_review: bool,

    /// Include issue automation.
    #[arg(long, default_value_t = true)]
    pub issue_automation: bool,

    /// Custom workflow name.
    #[arg(long, default_value = "Cortex")]
    pub workflow_name: String,
}

/// Arguments for run command.
#[derive(Debug, Parser)]
pub struct RunArgs {
    /// GitHub event type (issue_comment, pull_request, issues, etc.).
    #[arg(long, short)]
    pub event: String,

    /// GitHub token for API access.
    #[arg(long, short)]
    pub token: Option<String>,

    /// Path to the event payload JSON file.
    #[arg(long)]
    pub event_path: Option<PathBuf>,

    /// GitHub repository (owner/repo format).
    #[arg(long)]
    pub repository: Option<String>,

    /// GitHub run ID.
    #[arg(long)]
    pub run_id: Option<String>,

    /// Dry run mode - don't execute, just show what would happen.
    #[arg(long)]
    pub dry_run: bool,
}

/// Arguments for status command.
#[derive(Debug, Parser)]
pub struct StatusArgs {
    /// Path to the repository root (defaults to current directory).
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Output as JSON.
    #[arg(long)]
    pub json: bool,
}

/// Arguments for uninstall command.
#[derive(Debug, Parser)]
pub struct UninstallArgs {
    /// Path to the repository root (defaults to current directory).
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Workflow name to remove (defaults to "cortex").
    #[arg(long, default_value = "cortex")]
    pub workflow_name: String,

    /// Force removal without confirmation.
    #[arg(short, long)]
    pub force: bool,
}

/// Arguments for update command.
#[derive(Debug, Parser)]
pub struct UpdateArgs {
    /// Path to the repository root (defaults to current directory).
    #[arg(short, long)]
    pub path: Option<PathBuf>,

    /// Workflow name to update (defaults to "cortex").
    #[arg(long, default_value = "cortex")]
    pub workflow_name: String,

    /// Include PR review automation.
    #[arg(long, default_value_t = true)]
    pub pr_review: bool,

    /// Include issue automation.
    #[arg(long, default_value_t = true)]
    pub issue_automation: bool,
}

impl GitHubCli {
    /// Run the GitHub command.
    pub async fn run(self) -> Result<()> {
        match self.subcommand {
            GitHubSubcommand::Install(args) => run_install(args).await,
            GitHubSubcommand::Run(args) => run_github_agent(args).await,
            GitHubSubcommand::Status(args) => run_status(args).await,
            GitHubSubcommand::Uninstall(args) => run_uninstall(args).await,
            GitHubSubcommand::Update(args) => run_update(args).await,
        }
    }
}

/// Install GitHub Actions workflow.
async fn run_install(args: InstallArgs) -> Result<()> {
    use cortex_engine::github::{WorkflowConfig, generate_workflow};

    // Validate workflow name is not empty or whitespace-only
    if args.workflow_name.trim().is_empty() {
        bail!(
            "Workflow name cannot be empty.\n\
            Please provide a valid workflow name using --workflow-name or use the default value."
        );
    }

    let repo_path = args.path.unwrap_or_else(|| PathBuf::from("."));

    // Security: Reject paths containing directory traversal sequences
    let path_str = repo_path.to_string_lossy();
    if path_str.contains("..") {
        bail!(
            "Security error: Path contains directory traversal sequence (..): {}\n\
            Please provide a direct path without '..' components.",
            repo_path.display()
        );
    }

    // Validate that the target path exists and is a directory
    if !repo_path.exists() {
        bail!(
            "Target path does not exist: {}\n\
            Please ensure the directory exists or run from a valid repository root.",
            repo_path.display()
        );
    }

    if !repo_path.is_dir() {
        bail!(
            "Target path is not a directory: {}\n\
            Please provide a valid repository root directory.",
            repo_path.display()
        );
    }

    // Security: Canonicalize and verify the path is within expected bounds
    let canonical_path = repo_path.canonicalize().with_context(|| {
        format!(
            "Failed to resolve path: {}\nPlease ensure the path is accessible.",
            repo_path.display()
        )
    })?;

    let workflows_dir = canonical_path.join(".github").join("workflows");
    let workflow_file = workflows_dir.join(format!("{}.yml", args.workflow_name));

    // Check if workflow already exists
    if workflow_file.exists() && !args.force {
        bail!(
            "Workflow file already exists: {}\nUse --force to overwrite.",
            workflow_file.display()
        );
    }

    // Generate workflow configuration
    let config = WorkflowConfig {
        name: args.workflow_name.clone(),
        pr_review: args.pr_review,
        issue_automation: args.issue_automation,
    };

    let workflow_content = generate_workflow(&config);

    // Create directories if needed
    std::fs::create_dir_all(&workflows_dir)
        .with_context(|| format!("Failed to create directory: {}", workflows_dir.display()))?;

    // Write workflow file
    std::fs::write(&workflow_file, &workflow_content)
        .with_context(|| format!("Failed to write workflow file: {}", workflow_file.display()))?;

    println!("GitHub Actions workflow installed!");
    println!("   Location: {}", workflow_file.display());
    println!();
    println!("Next steps:");
    println!("  1. Add CORTEX_API_KEY to your repository secrets");
    println!("     Settings → Secrets and variables → Actions → New repository secret");
    println!();
    println!("  2. Commit and push the workflow file:");
    println!("     git add .github/workflows/{}.yml", args.workflow_name);
    println!("     git commit -m \"Add Cortex CI/CD automation\"");
    println!("     git push");
    println!();
    println!("Features enabled:");
    if args.pr_review {
        println!("  • PR review automation (triggered on pull_request events)");
    }
    if args.issue_automation {
        println!("  • Issue automation (triggered on issue_comment events)");
    }

    Ok(())
}

/// Run GitHub agent in Actions context.
async fn run_github_agent(args: RunArgs) -> Result<()> {
    use cortex_engine::github::{GitHubEvent, parse_event};

    let token = args.token.ok_or_else(|| {
        anyhow::anyhow!("GitHub token required. Set GITHUB_TOKEN env var or use --token")
    })?;

    let repository = args.repository.ok_or_else(|| {
        anyhow::anyhow!(
            "GitHub repository required. Set GITHUB_REPOSITORY env var or use --repository"
        )
    })?;

    // Parse the event payload
    let event_path = args.event_path.ok_or_else(|| {
        anyhow::anyhow!(
            "Event payload path required. Set GITHUB_EVENT_PATH env var or use --event-path"
        )
    })?;

    let event_content = std::fs::read_to_string(&event_path)
        .with_context(|| format!("Failed to read event file: {}", event_path.display()))?;

    let event = parse_event(&args.event, &event_content)
        .with_context(|| format!("Failed to parse {} event", args.event))?;

    println!("Cortex GitHub Agent");
    println!("{}", "=".repeat(40));
    println!("Repository: {}", repository);
    println!("Event type: {}", args.event);
    if let Some(ref run_id) = args.run_id {
        println!("Run ID: {}", run_id);
    }
    println!();

    if args.dry_run {
        println!("Dry run mode - not executing");
        println!();
        print_event_summary(&event);
        return Ok(());
    }

    // Execute the appropriate agent based on event type
    match event {
        GitHubEvent::IssueComment(comment) => {
            handle_issue_comment(&token, &repository, &comment).await?;
        }
        GitHubEvent::PullRequest(pr) => {
            handle_pull_request(&token, &repository, &pr).await?;
        }
        GitHubEvent::PullRequestReview(review) => {
            handle_pull_request_review(&token, &repository, &review).await?;
        }
        GitHubEvent::Issues(issue) => {
            handle_issue(&token, &repository, &issue).await?;
        }
        GitHubEvent::Unknown(event_type) => {
            eprintln!(
                "\x1b[1;33mWarning:\x1b[0m Unknown event type: {}",
                event_type
            );
            println!(
                "   Supported events: issue_comment, pull_request, pull_request_review, issues"
            );
        }
    }

    Ok(())
}

/// Print event summary for dry run mode.
fn print_event_summary(event: &cortex_engine::github::GitHubEvent) {
    use cortex_engine::github::GitHubEvent;

    match event {
        GitHubEvent::IssueComment(comment) => {
            println!("Event: Issue Comment");
            println!("  Action: {}", comment.action);
            println!("  Issue #: {}", comment.issue_number);
            println!("  Author: {}", comment.author);
            println!(
                "  Body preview: {}...",
                comment.body.chars().take(100).collect::<String>()
            );
        }
        GitHubEvent::PullRequest(pr) => {
            println!("Event: Pull Request");
            println!("  Action: {}", pr.action);
            println!("  PR #: {}", pr.number);
            println!("  Title: {}", pr.title);
            println!("  Author: {}", pr.author);
            println!("  Base: {} ← Head: {}", pr.base_branch, pr.head_branch);
        }
        GitHubEvent::PullRequestReview(review) => {
            println!("Event: Pull Request Review");
            println!("  Action: {}", review.action);
            println!("  PR #: {}", review.pr_number);
            println!("  Reviewer: {}", review.reviewer);
            println!("  State: {}", review.state);
        }
        GitHubEvent::Issues(issue) => {
            println!("Event: Issue");
            println!("  Action: {}", issue.action);
            println!("  Issue #: {}", issue.number);
            println!("  Title: {}", issue.title);
            println!("  Author: {}", issue.author);
        }
        GitHubEvent::Unknown(event_type) => {
            println!("Event: Unknown ({})", event_type);
        }
    }
}

/// Handle issue comment events.
async fn handle_issue_comment(
    token: &str,
    repository: &str,
    comment: &cortex_engine::github::IssueCommentEvent,
) -> Result<()> {
    use cortex_engine::github::GitHubClient;

    println!("Processing issue comment on #{}", comment.issue_number);
    println!("   Author: {}", comment.author);
    println!("   Action: {}", comment.action);

    // Only process new comments (not edits or deletions)
    if comment.action != "created" {
        println!("   Skipping: action is '{}'", comment.action);
        return Ok(());
    }

    // Check if comment mentions cortex or starts with /cortex
    let is_cortex_mention = comment.body.contains("@cortex")
        || comment.body.starts_with("/cortex")
        || comment.body.to_lowercase().contains("cortex help");

    if !is_cortex_mention {
        println!("   Skipping: no Cortex mention detected");
        return Ok(());
    }

    println!("   Cortex command detected!");

    // Initialize GitHub client
    let client = GitHubClient::new(token, repository)?;

    // Parse the command from the comment
    let command = parse_cortex_command(&comment.body);

    // Add reaction to show we're processing
    client.add_reaction(comment.comment_id, "eyes").await?;

    // Process the command
    let response = match command.as_str() {
        "help" => get_help_message(),
        "review" => {
            if comment.is_pull_request {
                "Starting code review... (not yet implemented)".to_string()
            } else {
                "This command is only available on pull requests.".to_string()
            }
        }
        "fix" => "Analyzing and suggesting fixes... (not yet implemented)".to_string(),
        _ => format!(
            "Unknown command: `{}`\n\nUse `/cortex help` to see available commands.",
            command
        ),
    };

    // Post response comment
    client
        .create_comment(comment.issue_number, &response)
        .await?;

    // Add success reaction
    client.add_reaction(comment.comment_id, "rocket").await?;

    println!("   Response posted");

    Ok(())
}

/// Handle pull request events.
async fn handle_pull_request(
    token: &str,
    repository: &str,
    pr: &cortex_engine::github::PullRequestEvent,
) -> Result<()> {
    use cortex_engine::github::GitHubClient;

    println!("🔀 Processing pull request #{}", pr.number);
    println!("   Title: {}", pr.title);
    println!("   Action: {}", pr.action);
    println!("   Author: {}", pr.author);

    // Only process opened or synchronized PRs
    if !matches!(pr.action.as_str(), "opened" | "synchronize" | "reopened") {
        println!("   Skipping: action is '{}'", pr.action);
        return Ok(());
    }

    let client = GitHubClient::new(token, repository)?;

    // Auto-review on PR open (if enabled)
    if pr.action == "opened" {
        println!("   New PR opened - preparing welcome message");

        let welcome_message = format!(
            "👋 Thanks for opening this PR, @{}!\n\n\
            I'm Cortex, your AI coding assistant. I can help with:\n\
            - `/cortex review` - Get a code review\n\
            - `/cortex help` - See all available commands\n\n\
            I'll analyze this PR automatically when ready.",
            pr.author
        );

        client.create_comment(pr.number, &welcome_message).await?;
    }

    // For synchronize events (new commits pushed)
    if pr.action == "synchronize" {
        println!("   New commits pushed to PR");
        // Could trigger re-review here
    }

    Ok(())
}

/// Handle pull request review events.
async fn handle_pull_request_review(
    _token: &str,
    _repository: &str,
    review: &cortex_engine::github::PullRequestReviewEvent,
) -> Result<()> {
    println!("Processing PR review on #{}", review.pr_number);
    println!("   Reviewer: {}", review.reviewer);
    println!("   State: {}", review.state);
    println!("   Action: {}", review.action);

    // Could respond to review requests here
    println!("   Note: PR review events not yet implemented");

    Ok(())
}

/// Handle issue events.
async fn handle_issue(
    token: &str,
    repository: &str,
    issue: &cortex_engine::github::IssueEvent,
) -> Result<()> {
    use cortex_engine::github::GitHubClient;

    println!("Processing issue #{}", issue.number);
    println!("   Title: {}", issue.title);
    println!("   Action: {}", issue.action);

    // Only greet on new issues
    if issue.action != "opened" {
        println!("   Skipping: action is '{}'", issue.action);
        return Ok(());
    }

    let client = GitHubClient::new(token, repository)?;

    // Check if issue mentions cortex
    let is_cortex_related = issue.title.to_lowercase().contains("Cortex")
        || issue.body.to_lowercase().contains("Cortex")
        || issue
            .labels
            .iter()
            .any(|l| l.to_lowercase().contains("Cortex"));

    if is_cortex_related {
        let greeting = format!(
            "👋 Thanks for opening this issue, @{}!\n\n\
            I'm Cortex, your AI coding assistant. I'll analyze this issue and provide suggestions.\n\n\
            In the meantime, you can use `/cortex help` to see what I can do.",
            issue.author
        );

        client.create_comment(issue.number, &greeting).await?;
    }

    Ok(())
}

/// Parse a cortex command from comment text.
fn parse_cortex_command(text: &str) -> String {
    // Look for /cortex <command> pattern
    if let Some(pos) = text.find("/cortex") {
        let after_cortex = &text[pos + 7..];
        let command = after_cortex.split_whitespace().next().unwrap_or("help");
        return command.to_string();
    }

    // Look for @cortex <command> pattern
    if let Some(pos) = text.find("@cortex") {
        let after_cortex = &text[pos + 7..];
        let command = after_cortex.split_whitespace().next().unwrap_or("help");
        return command.to_string();
    }

    "help".to_string()
}

/// Get help message for Cortex commands.
fn get_help_message() -> String {
    r#"## Cortex Commands

| Command | Description |
|---------|-------------|
| `/cortex help` | Show this help message |
| `/cortex review` | Request a code review (PRs only) |
| `/cortex fix` | Suggest fixes for issues |
| `/cortex explain` | Explain the code changes |
| `/cortex test` | Suggest tests for changes |

### Tips
- Mention `@cortex` anywhere in your comment to get my attention
- I automatically review new pull requests when configured
- Use labels like `cortex:review` to trigger specific actions

[Learn more](https://docs.cortex.foundation/github)"#
        .to_string()
}

/// Check GitHub Actions installation status.
async fn run_status(args: StatusArgs) -> Result<()> {
    let repo_path = args.path.unwrap_or_else(|| PathBuf::from("."));

    let mut status = InstallationStatus::default();

    // Check for workflow files
    let workflows_dir = repo_path.join(".github").join("workflows");
    if workflows_dir.exists() {
        for entry in std::fs::read_dir(&workflows_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "yml" || e == "yaml") {
                let content = std::fs::read_to_string(&path)?;
                if content.contains("Cortex") {
                    status.workflow_installed = true;
                    status.workflow_path = Some(path.clone());

                    // Check workflow features
                    if content.contains("issue_comment") {
                        status.features.push("issue_comment".to_string());
                    }
                    if content.contains("pull_request") {
                        status.features.push("pull_request".to_string());
                    }
                    if content.contains("issues") {
                        status.features.push("issues".to_string());
                    }
                    break;
                }
            }
        }
    }

    // Check for .github directory
    status.github_dir_exists = repo_path.join(".github").exists();

    // Check if we're in a git repo
    status.is_git_repo = repo_path.join(".git").exists();

    if args.json {
        let json = serde_json::to_string_pretty(&status)?;
        println!("{}", json);
        // Return non-zero exit code if workflow is not installed
        if !status.workflow_installed {
            std::process::exit(1);
        }
        return Ok(());
    }

    println!("GitHub Actions Status");
    println!("{}", "=".repeat(40));
    println!();

    if !status.is_git_repo {
        print_warning("Not a git repository.");
        println!("   Run this command from a git repository root.");
        std::process::exit(1);
    }

    if !status.github_dir_exists {
        print_error(".github directory not found.");
        println!("   Run `cortex github install` to set up GitHub Actions.");
        std::process::exit(1);
    }

    if status.workflow_installed {
        print_success("Cortex workflow is installed.");
        if let Some(ref path) = status.workflow_path {
            println!("   Path: {}", path.display());
        }
        println!();
        println!("Features enabled:");
        for feature in &status.features {
            println!("  • {}", feature);
        }
    } else {
        print_error("Cortex workflow not found.");
        println!("   Run `cortex github install` to set up GitHub Actions.");
        std::process::exit(1);
    }

    Ok(())
}

/// Uninstall/remove GitHub Actions workflow.
async fn run_uninstall(args: UninstallArgs) -> Result<()> {
    use std::io::{self, Write};

    let repo_path = args.path.unwrap_or_else(|| PathBuf::from("."));

    // Validate path exists
    if !repo_path.exists() {
        bail!("Path does not exist: {}", repo_path.display());
    }

    // Check for workflow files
    let workflows_dir = repo_path.join(".github").join("workflows");

    // Try multiple possible workflow file names
    let possible_names = vec![
        format!("{}.yml", args.workflow_name),
        format!("{}.yaml", args.workflow_name),
    ];

    let mut found_workflow: Option<PathBuf> = None;
    for name in &possible_names {
        let path = workflows_dir.join(name);
        if path.exists() {
            // Verify it's a Cortex workflow
            if let Ok(content) = std::fs::read_to_string(&path)
                && (content.contains("Cortex") || content.contains("cortex"))
            {
                found_workflow = Some(path);
                break;
            }
        }
    }

    let workflow_path = match found_workflow {
        Some(p) => p,
        None => {
            bail!(
                "Cortex workflow '{}' not found in {}.\n\
                Use `cortex github status` to check installation status.",
                args.workflow_name,
                workflows_dir.display()
            );
        }
    };

    // Confirm removal unless --force
    if !args.force {
        print!(
            "Remove Cortex workflow '{}'? [y/N]: ",
            workflow_path.display()
        );
        io::stdout().flush()?;

        let mut input = String::new();
        io::stdin().read_line(&mut input)?;

        if !input.trim().eq_ignore_ascii_case("y") {
            println!("Cancelled.");
            return Ok(());
        }
    }

    // Remove the workflow file
    std::fs::remove_file(&workflow_path)
        .with_context(|| format!("Failed to remove workflow: {}", workflow_path.display()))?;

    println!("Cortex workflow removed successfully!");
    println!("   Removed: {}", workflow_path.display());
    println!();
    println!(
        "Note: You may also want to remove the CORTEX_API_KEY secret from your repository settings."
    );

    Ok(())
}

/// Update GitHub Actions workflow to latest version.
async fn run_update(args: UpdateArgs) -> Result<()> {
    use cortex_engine::github::{WorkflowConfig, generate_workflow};

    let repo_path = args.path.unwrap_or_else(|| PathBuf::from("."));

    // Validate path exists
    if !repo_path.exists() {
        bail!("Path does not exist: {}", repo_path.display());
    }

    let workflows_dir = repo_path.join(".github").join("workflows");

    // Try to find existing workflow
    let possible_names = vec![
        format!("{}.yml", args.workflow_name),
        format!("{}.yaml", args.workflow_name),
    ];

    let mut existing_path: Option<PathBuf> = None;
    for name in &possible_names {
        let path = workflows_dir.join(name);
        if path.exists() {
            existing_path = Some(path);
            break;
        }
    }

    let workflow_file = match existing_path {
        Some(p) => p,
        None => {
            bail!(
                "Cortex workflow '{}' not found. Use `cortex github install` first.",
                args.workflow_name
            );
        }
    };

    // Generate updated workflow
    let config = WorkflowConfig {
        name: args.workflow_name.clone(),
        pr_review: args.pr_review,
        issue_automation: args.issue_automation,
    };

    let workflow_content = generate_workflow(&config);

    // Write updated workflow
    std::fs::write(&workflow_file, &workflow_content)
        .with_context(|| format!("Failed to write workflow file: {}", workflow_file.display()))?;

    println!("Cortex workflow updated successfully!");
    println!("   Path: {}", workflow_file.display());
    println!();
    println!("Features enabled:");
    if args.pr_review {
        println!("  • PR review automation");
    }
    if args.issue_automation {
        println!("  • Issue automation");
    }
    println!();
    println!("Next steps:");
    println!("  1. Commit and push the updated workflow:");
    println!("     git add {}", workflow_file.display());
    println!("     git commit -m \"Update Cortex workflow\"");
    println!("     git push");

    Ok(())
}

/// Installation status information.
#[derive(Debug, Default, serde::Serialize)]
struct InstallationStatus {
    is_git_repo: bool,
    github_dir_exists: bool,
    workflow_installed: bool,
    workflow_path: Option<PathBuf>,
    features: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cortex_command() {
        assert_eq!(parse_cortex_command("/cortex help"), "help");
        assert_eq!(parse_cortex_command("/cortex review"), "review");
        assert_eq!(parse_cortex_command("@cortex fix"), "fix");
        assert_eq!(parse_cortex_command("Please @cortex help me"), "help");
        assert_eq!(parse_cortex_command("No command here"), "help");
    }

    #[test]
    fn test_get_help_message() {
        let help = get_help_message();
        assert!(help.contains("/cortex help"));
        assert!(help.contains("/cortex review"));
    }

    #[tokio::test]
    async fn test_install_validates_path_exists() {
        let args = InstallArgs {
            path: Some(PathBuf::from("/nonexistent/path/that/does/not/exist")),
            force: false,
            pr_review: true,
            issue_automation: true,
            workflow_name: "test".to_string(),
        };

        let result = run_install(args).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Target path does not exist"),
            "Expected 'Target path does not exist' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_install_validates_path_is_directory() {
        use std::io::Write;

        // Create a temporary file to test with
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("cortex_test_file_{}", std::process::id()));

        // Create the file
        let mut file = std::fs::File::create(&temp_file).expect("Failed to create temp file");
        file.write_all(b"test content")
            .expect("Failed to write to temp file");
        drop(file);

        let args = InstallArgs {
            path: Some(temp_file.clone()),
            force: false,
            pr_review: true,
            issue_automation: true,
            workflow_name: "test".to_string(),
        };

        let result = run_install(args).await;

        // Clean up
        let _ = std::fs::remove_file(&temp_file);

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Target path is not a directory"),
            "Expected 'Target path is not a directory' error, got: {}",
            err
        );
    }

    #[tokio::test]
    async fn test_install_accepts_valid_directory() {
        // Create a temporary directory
        let temp_dir = std::env::temp_dir().join(format!("cortex_test_dir_{}", std::process::id()));
        std::fs::create_dir_all(&temp_dir).expect("Failed to create temp dir");

        let args = InstallArgs {
            path: Some(temp_dir.clone()),
            force: false,
            pr_review: true,
            issue_automation: true,
            workflow_name: "test-workflow".to_string(),
        };

        let result = run_install(args).await;

        // Clean up
        let workflow_path = temp_dir
            .join(".github")
            .join("workflows")
            .join("test-workflow.yml");
        let _ = std::fs::remove_file(&workflow_path);
        let _ = std::fs::remove_dir_all(&temp_dir);

        assert!(
            result.is_ok(),
            "Expected successful install to valid directory, got error: {:?}",
            result
        );
    }
}
