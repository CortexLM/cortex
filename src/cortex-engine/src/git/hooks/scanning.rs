//! Scanning Functions.
//!
//! Functions for scanning code for secrets, TODOs, and custom patterns.

use std::fs;
use std::path::Path;

use regex::Regex;

use crate::error::{CortexError, Result};
use crate::git_info::GitDiff;

use super::config::PatternCheck;
use super::results::{HookIssue, IssueCategory, IssueSeverity};
use super::utils::should_ignore_path;

/// Scan for secrets in staged changes.
pub async fn scan_secrets(repo_path: &Path, diff: &GitDiff) -> Result<Vec<HookIssue>> {
    let mut issues = Vec::new();

    let secret_patterns: Vec<(&str, &str)> = vec![
        ("AWS Access Key", r"AKIA[0-9A-Z]{16}"),
        (
            "AWS Secret Key",
            r#"(?i)aws(.{0,20})?['"][0-9a-zA-Z/+]{40}['"]"#,
        ),
        ("GitHub Token", r"ghp_[0-9a-zA-Z]{36}"),
        ("GitLab Token", r"glpat-[0-9a-zA-Z_-]{20}"),
        (
            "Generic API Key",
            r#"(?i)(api[_-]?key|apikey)['"]?\s*[:=]\s*['"][0-9a-zA-Z]{20,}['"]"#,
        ),
        (
            "Private Key",
            r"-----BEGIN (RSA |DSA |EC )?PRIVATE KEY-----",
        ),
        (
            "Password Assignment",
            r#"(?i)(password|passwd|pwd)['"]?\s*[:=]\s*['"][^'"]{8,}['"]"#,
        ),
    ];

    for file in &diff.files {
        // Skip binary files and common non-code files
        if should_ignore_path(&file.path) {
            continue;
        }

        // Read file content
        let file_path = repo_path.join(&file.path);
        if !file_path.exists() {
            continue;
        }

        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue, // Skip binary files
        };

        for (name, pattern) in &secret_patterns {
            if let Ok(re) = Regex::new(pattern) {
                for (line_num, line) in content.lines().enumerate() {
                    if re.is_match(line) {
                        issues.push(HookIssue::secret(&file.path, line_num + 1, name));
                    }
                }
            }
        }
    }

    Ok(issues)
}

/// Scan for TODO/FIXME comments.
pub async fn scan_todos(repo_path: &Path, diff: &GitDiff) -> Result<Vec<HookIssue>> {
    let mut issues = Vec::new();
    let todo_pattern = Regex::new(r"(?i)\b(TODO|FIXME|XXX|HACK)\b").unwrap();

    for file in &diff.files {
        if should_ignore_path(&file.path) {
            continue;
        }

        let file_path = repo_path.join(&file.path);
        if !file_path.exists() {
            continue;
        }

        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (line_num, line) in content.lines().enumerate() {
            if let Some(_m) = todo_pattern.find(line) {
                issues.push(HookIssue::todo(&file.path, line_num + 1, line.trim()));
            }
        }
    }

    Ok(issues)
}

/// Scan for custom pattern matches.
pub async fn scan_pattern(
    repo_path: &Path,
    diff: &GitDiff,
    pattern: &PatternCheck,
) -> Result<Vec<HookIssue>> {
    let mut issues = Vec::new();
    let re = Regex::new(&pattern.pattern).map_err(|e| {
        CortexError::Internal(format!("Invalid pattern '{}': {}", pattern.pattern, e))
    })?;

    for file in &diff.files {
        // Check file pattern filter
        if !pattern.file_patterns.is_empty() {
            let matches_filter = pattern.file_patterns.iter().any(|p| {
                glob::Pattern::new(p)
                    .map(|pat| pat.matches(&file.path.to_string_lossy()))
                    .unwrap_or(false)
            });
            if !matches_filter {
                continue;
            }
        }

        let file_path = repo_path.join(&file.path);
        if !file_path.exists() {
            continue;
        }

        let content = match fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        for (line_num, line) in content.lines().enumerate() {
            if re.is_match(line) {
                let severity = if pattern.blocking {
                    IssueSeverity::Error
                } else {
                    IssueSeverity::Warning
                };

                issues.push(
                    HookIssue::new(IssueCategory::CustomPattern, severity, &pattern.message)
                        .with_file(&file.path)
                        .with_line(line_num + 1),
                );
            }
        }
    }

    Ok(issues)
}

/// Validate conventional commit format.
pub fn validate_conventional_commit(message: &str) -> Option<HookIssue> {
    let first_line = message.lines().next()?;

    // Conventional commit format: type(scope): description
    let conventional_re = Regex::new(
        r"^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)(\([a-z0-9-]+\))?: .+",
    )
    .unwrap();

    if !conventional_re.is_match(first_line) {
        Some(
            HookIssue::new(
                IssueCategory::CommitMessage,
                IssueSeverity::Error,
                "Commit message doesn't follow conventional commits format",
            )
            .with_suggestion(
                "Use format: type(scope): description\n\
                 Types: feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert",
            ),
        )
    } else {
        None
    }
}
