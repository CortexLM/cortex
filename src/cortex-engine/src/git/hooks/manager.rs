//! Hook Manager.
//!
//! The main entry point for managing git hooks.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use tracing::info;

use crate::error::{CortexError, Result};
use crate::git_info::{GitDiff, find_git_root};

use super::config::HookConfig;
use super::results::{HookExecutionResult, HookIssue, HookStatus, IssueCategory, IssueSeverity};
use super::scanning::{scan_pattern, scan_secrets, scan_todos, validate_conventional_commit};
use super::types::GitHook;

/// Cortex hook file marker.
const CORTEX_HOOK_MARKER: &str = "# Cortex Git Hook";

/// Default hook script template.
const HOOK_SCRIPT_TEMPLATE: &str = r#"#!/bin/bash
# Cortex Git Hook - {hook_name}
# Installed by Cortex CLI
# Do not edit manually - use 'cortex hooks' commands

exec cortex hook run {hook_name} "$@"
"#;

/// Windows hook script template.
const HOOK_SCRIPT_TEMPLATE_WIN: &str = r#"#!/bin/sh
# Cortex Git Hook - {hook_name}
# Installed by Cortex CLI
# Do not edit manually - use 'cortex hooks' commands

cortex.exe hook run {hook_name} "$@"
"#;

/// Manager for git hooks.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct HookManager {
    /// Repository root path.
    repo_path: PathBuf,
    /// Git hooks directory.
    hooks_dir: PathBuf,
    /// Hook configurations.
    configs: HashMap<GitHook, HookConfig>,
}

impl HookManager {
    /// Create a new hook manager for a repository.
    pub fn new(repo_path: impl AsRef<Path>) -> Result<Self> {
        let repo_path = find_git_root(repo_path.as_ref())?;
        let hooks_dir = repo_path.join(".git").join("hooks");

        Ok(Self {
            repo_path,
            hooks_dir,
            configs: HashMap::new(),
        })
    }

    /// Get the repository path.
    pub fn repo_path(&self) -> &Path {
        &self.repo_path
    }

    /// Get the hooks directory.
    pub fn hooks_dir(&self) -> &Path {
        &self.hooks_dir
    }

    /// Install a git hook.
    pub async fn install_hook(&self, hook: GitHook, config: HookConfig) -> Result<()> {
        let hook_path = self.hooks_dir.join(hook.filename());

        // Check for existing non-Cortex hook
        if hook_path.exists() && !self.is_cortex_hook(&hook_path)? {
            // Backup existing hook
            let backup_path = hook_path.with_extension("backup");
            fs::rename(&hook_path, &backup_path).map_err(|e| {
                CortexError::Internal(format!("Failed to backup existing hook: {}", e))
            })?;
            info!(
                "Backed up existing {} hook to {}",
                hook,
                backup_path.display()
            );
        }

        // Create hooks directory if needed
        if !self.hooks_dir.exists() {
            fs::create_dir_all(&self.hooks_dir).map_err(|e| {
                CortexError::Internal(format!("Failed to create hooks directory: {}", e))
            })?;
        }

        // Generate hook script
        let script = self.generate_hook_script(hook);

        // Write hook file
        fs::write(&hook_path, script)
            .map_err(|e| CortexError::Internal(format!("Failed to write hook file: {}", e)))?;

        // Make executable (Unix only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&hook_path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&hook_path, perms)?;
        }

        // Save config
        self.save_hook_config(hook, &config)?;

        info!("Installed {} hook", hook);
        Ok(())
    }

    /// Uninstall a git hook.
    pub fn uninstall_hook(&self, hook: GitHook) -> Result<()> {
        let hook_path = self.hooks_dir.join(hook.filename());

        if !hook_path.exists() {
            return Ok(());
        }

        // Only remove if it's a Cortex hook
        if !self.is_cortex_hook(&hook_path)? {
            return Err(CortexError::Internal(format!(
                "Hook {} is not a Cortex hook - not removing",
                hook
            )));
        }

        fs::remove_file(&hook_path)
            .map_err(|e| CortexError::Internal(format!("Failed to remove hook: {}", e)))?;

        // Restore backup if exists
        let backup_path = hook_path.with_extension("backup");
        if backup_path.exists() {
            fs::rename(&backup_path, &hook_path)
                .map_err(|e| CortexError::Internal(format!("Failed to restore backup: {}", e)))?;
            info!("Restored backup {} hook", hook);
        }

        // Remove config
        self.remove_hook_config(hook)?;

        info!("Uninstalled {} hook", hook);
        Ok(())
    }

    /// Install all hooks.
    pub async fn install_all(&self, config: HookConfig) -> Result<()> {
        for hook in GitHook::all() {
            self.install_hook(hook, config.clone()).await?;
        }
        Ok(())
    }

    /// Uninstall all Cortex hooks.
    pub fn uninstall_all(&self) -> Result<()> {
        for hook in GitHook::all() {
            self.uninstall_hook(hook)?;
        }
        Ok(())
    }

    /// List installed hooks.
    pub fn list_installed(&self) -> Result<Vec<(GitHook, bool)>> {
        let mut hooks = Vec::new();

        for hook in GitHook::all() {
            let hook_path = self.hooks_dir.join(hook.filename());
            if hook_path.exists() {
                let is_cortex = self.is_cortex_hook(&hook_path)?;
                hooks.push((hook, is_cortex));
            }
        }

        Ok(hooks)
    }

    /// Get status of all hooks.
    pub fn status(&self) -> Result<Vec<HookStatus>> {
        let mut statuses = Vec::new();

        for hook in GitHook::all() {
            let hook_path = self.hooks_dir.join(hook.filename());
            let installed = hook_path.exists();
            let is_cortex = if installed {
                self.is_cortex_hook(&hook_path)?
            } else {
                false
            };
            let config = if is_cortex {
                self.load_hook_config(hook).ok()
            } else {
                None
            };

            statuses.push(HookStatus {
                hook,
                installed,
                is_cortex,
                config,
                path: hook_path,
            });
        }

        Ok(statuses)
    }

    /// Run a hook manually.
    pub async fn run_hook(&self, hook: GitHook, args: &[&str]) -> Result<HookExecutionResult> {
        let start = Instant::now();
        let config = self.load_hook_config(hook).unwrap_or_default();

        if !config.enabled {
            return Ok(HookExecutionResult::success(hook, 0).with_output("Hook disabled"));
        }

        let result = match hook {
            GitHook::PreCommit => self.run_pre_commit(&config).await,
            GitHook::PrepareCommitMsg => {
                let msg_file = args.first().map(|s| PathBuf::from(s));
                self.run_prepare_commit_msg(&config, msg_file.as_ref())
                    .await
            }
            GitHook::CommitMsg => {
                let msg_file = args.first().map(|s| PathBuf::from(s));
                self.run_commit_msg(&config, msg_file.as_ref()).await
            }
            GitHook::PostCommit => self.run_post_commit(&config).await,
            GitHook::PrePush => self.run_pre_push(&config).await,
            GitHook::PreRebase => self.run_pre_rebase(&config).await,
        };

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(mut result) => {
                result.duration_ms = duration_ms;
                Ok(result)
            }
            Err(e) => Ok(HookExecutionResult::failure(
                hook,
                1,
                vec![HookIssue::new(
                    IssueCategory::CodeQuality,
                    IssueSeverity::Error,
                    e.to_string(),
                )],
                duration_ms,
            )),
        }
    }

    /// Check if a hook file is a Cortex hook.
    fn is_cortex_hook(&self, path: &Path) -> Result<bool> {
        let content = fs::read_to_string(path)
            .map_err(|e| CortexError::Internal(format!("Failed to read hook: {}", e)))?;
        Ok(content.contains(CORTEX_HOOK_MARKER))
    }

    /// Generate hook script content.
    fn generate_hook_script(&self, hook: GitHook) -> String {
        let template = if cfg!(windows) {
            HOOK_SCRIPT_TEMPLATE_WIN
        } else {
            HOOK_SCRIPT_TEMPLATE
        };

        template
            .replace(
                "# Cortex Git Hook",
                &format!("{}\n# Cortex Git Hook", CORTEX_HOOK_MARKER),
            )
            .replace("{hook_name}", hook.filename())
    }

    /// Save hook configuration.
    fn save_hook_config(&self, hook: GitHook, config: &HookConfig) -> Result<()> {
        let config_dir = self.repo_path.join(".cortex").join("hooks");
        fs::create_dir_all(&config_dir).map_err(|e| {
            CortexError::Internal(format!("Failed to create config directory: {}", e))
        })?;

        let config_path = config_dir.join(format!("{}.toml", hook.filename()));
        let toml = toml::to_string_pretty(config)
            .map_err(|e| CortexError::Internal(format!("Failed to serialize config: {}", e)))?;

        fs::write(&config_path, toml)
            .map_err(|e| CortexError::Internal(format!("Failed to write config: {}", e)))?;

        Ok(())
    }

    /// Load hook configuration.
    fn load_hook_config(&self, hook: GitHook) -> Result<HookConfig> {
        let config_path = self
            .repo_path
            .join(".cortex")
            .join("hooks")
            .join(format!("{}.toml", hook.filename()));

        if !config_path.exists() {
            return Ok(HookConfig::default());
        }

        let content = fs::read_to_string(&config_path)
            .map_err(|e| CortexError::Internal(format!("Failed to read config: {}", e)))?;

        toml::from_str(&content)
            .map_err(|e| CortexError::Internal(format!("Failed to parse config: {}", e)))
    }

    /// Remove hook configuration.
    fn remove_hook_config(&self, hook: GitHook) -> Result<()> {
        let config_path = self
            .repo_path
            .join(".cortex")
            .join("hooks")
            .join(format!("{}.toml", hook.filename()));

        if config_path.exists() {
            fs::remove_file(&config_path)
                .map_err(|e| CortexError::Internal(format!("Failed to remove config: {}", e)))?;
        }

        Ok(())
    }

    // ========================================================================
    // Hook Implementations
    // ========================================================================

    /// Run pre-commit hook.
    async fn run_pre_commit(&self, config: &HookConfig) -> Result<HookExecutionResult> {
        let mut issues = Vec::new();
        let mut warnings = Vec::new();

        // Get staged changes
        let diff = GitDiff::staged(&self.repo_path)?;

        if diff.files.is_empty() {
            return Ok(HookExecutionResult::success(GitHook::PreCommit, 0)
                .with_output("No staged changes"));
        }

        // Check for secrets
        if config.check_secrets {
            let secret_issues = scan_secrets(&self.repo_path, &diff).await?;
            issues.extend(secret_issues);
        }

        // Check for TODOs
        if config.check_todos {
            let todo_issues = scan_todos(&self.repo_path, &diff).await?;
            for issue in todo_issues {
                if issue.severity == IssueSeverity::Warning {
                    warnings.push(issue.message.clone());
                }
            }
        }

        // Check custom patterns
        for pattern in &config.custom_patterns {
            let pattern_issues = scan_pattern(&self.repo_path, &diff, pattern).await?;
            issues.extend(pattern_issues);
        }

        // Determine pass/fail
        let has_blocking = issues.iter().any(|i| i.severity == IssueSeverity::Error);

        if has_blocking && config.block_on_issues {
            Ok(HookExecutionResult::failure(
                GitHook::PreCommit,
                1,
                issues,
                0,
            ))
        } else {
            let mut result = HookExecutionResult::success(GitHook::PreCommit, 0);
            result.issues = issues;
            result.warnings = warnings;
            Ok(result)
        }
    }

    /// Run prepare-commit-msg hook.
    async fn run_prepare_commit_msg(
        &self,
        config: &HookConfig,
        msg_file: Option<&PathBuf>,
    ) -> Result<HookExecutionResult> {
        // If AI review enabled, generate commit message
        if config.ai_review {
            let _diff = GitDiff::staged(&self.repo_path)?;
            let diff_content = GitDiff::content(&self.repo_path, true)?;

            // Generate commit message using AI
            let message = self.generate_commit_message(&diff_content, config).await?;

            // Write to message file if provided
            if let Some(file) = msg_file {
                fs::write(file, &message).map_err(|e| {
                    CortexError::Internal(format!("Failed to write commit message: {}", e))
                })?;
            }

            Ok(HookExecutionResult::success(GitHook::PrepareCommitMsg, 0)
                .with_generated_message(&message))
        } else {
            Ok(HookExecutionResult::success(GitHook::PrepareCommitMsg, 0))
        }
    }

    /// Run commit-msg hook.
    async fn run_commit_msg(
        &self,
        config: &HookConfig,
        msg_file: Option<&PathBuf>,
    ) -> Result<HookExecutionResult> {
        let mut issues = Vec::new();

        let message = if let Some(file) = msg_file {
            fs::read_to_string(file).map_err(|e| {
                CortexError::Internal(format!("Failed to read commit message: {}", e))
            })?
        } else {
            return Ok(HookExecutionResult::success(GitHook::CommitMsg, 0));
        };

        // Validate message length
        if message.len() < config.min_message_length {
            issues.push(HookIssue::new(
                IssueCategory::CommitMessage,
                IssueSeverity::Error,
                format!(
                    "Commit message too short ({} chars, min {})",
                    message.len(),
                    config.min_message_length
                ),
            ));
        }

        // Validate subject line length
        if let Some(subject) = message.lines().next() {
            if subject.len() > config.max_subject_length {
                issues.push(HookIssue::new(
                    IssueCategory::CommitMessage,
                    IssueSeverity::Warning,
                    format!(
                        "Subject line too long ({} chars, max {})",
                        subject.len(),
                        config.max_subject_length
                    ),
                ));
            }
        }

        // Validate conventional commits format
        if config.conventional_commits {
            if let Some(issue) = validate_conventional_commit(&message) {
                issues.push(issue);
            }
        }

        // Check required sections
        for section in &config.required_sections {
            if !message.contains(section) {
                issues.push(HookIssue::new(
                    IssueCategory::CommitMessage,
                    IssueSeverity::Warning,
                    format!("Missing required section: {}", section),
                ));
            }
        }

        let has_blocking = issues.iter().any(|i| i.severity == IssueSeverity::Error);

        if has_blocking && config.block_on_issues {
            Ok(HookExecutionResult::failure(
                GitHook::CommitMsg,
                1,
                issues,
                0,
            ))
        } else {
            let mut result = HookExecutionResult::success(GitHook::CommitMsg, 0);
            result.issues = issues;
            Ok(result)
        }
    }

    /// Run post-commit hook.
    async fn run_post_commit(&self, _config: &HookConfig) -> Result<HookExecutionResult> {
        // Post-commit is informational only
        Ok(HookExecutionResult::success(GitHook::PostCommit, 0))
    }

    /// Run pre-push hook.
    async fn run_pre_push(&self, config: &HookConfig) -> Result<HookExecutionResult> {
        let mut issues = Vec::new();

        // Get commits being pushed
        // This is a simplified implementation - real version would parse refs
        let diff = GitDiff::unstaged(&self.repo_path)?;

        // Full secrets scan
        if config.check_secrets {
            let secret_issues = scan_secrets(&self.repo_path, &diff).await?;
            issues.extend(secret_issues);
        }

        let has_blocking = issues.iter().any(|i| i.severity == IssueSeverity::Error);

        if has_blocking && config.block_on_issues {
            Ok(HookExecutionResult::failure(GitHook::PrePush, 1, issues, 0))
        } else {
            let mut result = HookExecutionResult::success(GitHook::PrePush, 0);
            result.issues = issues;
            Ok(result)
        }
    }

    /// Run pre-rebase hook.
    async fn run_pre_rebase(&self, _config: &HookConfig) -> Result<HookExecutionResult> {
        // Pre-rebase is typically a warning hook
        Ok(HookExecutionResult::success(GitHook::PreRebase, 0))
    }

    /// Generate commit message using AI.
    async fn generate_commit_message(&self, diff: &str, _config: &HookConfig) -> Result<String> {
        // This is a placeholder - actual implementation would call the AI
        // using the configured model

        // Extract likely type from changes
        let commit_type = if diff.contains("test") || diff.contains("spec") {
            "test"
        } else if diff.contains("README") || diff.contains(".md") {
            "docs"
        } else if diff.contains("fix") || diff.contains("bug") {
            "fix"
        } else {
            "feat"
        };

        Ok(format!(
            "{}(scope): describe your changes here\n\n\
             # Changes detected in this commit:\n\
             # - {} files changed\n\n\
             # Please edit this message to describe your changes.",
            commit_type,
            diff.lines()
                .filter(|l| l.starts_with("+++") || l.starts_with("---"))
                .count()
                / 2
        ))
    }
}
