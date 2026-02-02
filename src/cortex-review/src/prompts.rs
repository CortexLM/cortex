//! Review prompts for the AI.

use crate::{Result, ReviewTarget};
use std::path::Path;
use tokio::process::Command;

/// Prompt for reviewing uncommitted changes.
const UNCOMMITTED_PROMPT: &str = r#"Review the current code changes (staged, unstaged, and untracked files) and provide prioritized findings.

Focus on:
1. **Critical Issues**: Security vulnerabilities, data loss risks, crashes
2. **Bugs**: Logic errors, edge cases, race conditions
3. **Code Quality**: Maintainability, readability, best practices
4. **Performance**: Inefficiencies, unnecessary allocations
5. **Testing**: Missing tests, inadequate coverage

For each finding:
- Severity: Critical / High / Medium / Low
- Location: file:line
- Description: What's wrong
- Suggestion: How to fix it

Start by running `git diff` and `git diff --staged` to see the changes."#;

/// Prompt for reviewing against a base branch.
const BASE_BRANCH_PROMPT: &str = r#"Review the code changes against the base branch '{branch}'.

The merge base commit for this comparison is {merge_base}.
Run `git diff {merge_base}` to inspect the changes relative to '{branch}'.

Provide prioritized, actionable findings focusing on:
1. **Critical Issues**: Security vulnerabilities, data loss risks
2. **Bugs**: Logic errors, edge cases, race conditions
3. **Breaking Changes**: API changes, compatibility issues
4. **Code Quality**: Maintainability, readability
5. **Testing**: Missing tests for new functionality

For each finding, provide:
- Severity level
- File and line location
- Clear description
- Actionable suggestion"#;

/// Prompt for reviewing against a base branch (fallback without merge base).
const BASE_BRANCH_PROMPT_FALLBACK: &str = r#"Review the code changes against the base branch '{branch}'.

First find the merge base:
```bash
git merge-base HEAD "$(git rev-parse --abbrev-ref '{branch}@{{upstream}}')"
```

Then run `git diff <merge_base_sha>` to see the changes.

Provide prioritized, actionable findings."#;

/// Prompt for reviewing a specific commit.
const COMMIT_PROMPT: &str = r#"Review the code changes introduced by commit {sha}.

Run `git show {sha}` to see the commit details and changes.

{title_info}

Provide prioritized, actionable findings focusing on:
1. **Bugs**: Logic errors introduced by this commit
2. **Code Quality**: Does this commit follow best practices?
3. **Completeness**: Are there missing pieces?
4. **Testing**: Should there be tests for these changes?"#;

/// Prompt for reviewing a commit range.
const RANGE_PROMPT: &str = r#"Review the code changes in commits {from}..{to}.

Run `git log --oneline {from}..{to}` to see the commits.
Run `git diff {from}..{to}` to see all changes.

Provide prioritized, actionable findings for this set of changes."#;

/// Build the review prompt for a target.
pub fn build_review_prompt(target: &ReviewTarget, merge_base: Option<&str>) -> String {
    match target {
        ReviewTarget::UncommittedChanges => UNCOMMITTED_PROMPT.to_string(),

        ReviewTarget::BaseBranch { branch, .. } => {
            if let Some(base) = merge_base {
                BASE_BRANCH_PROMPT
                    .replace("{branch}", branch)
                    .replace("{merge_base}", base)
            } else {
                BASE_BRANCH_PROMPT_FALLBACK.replace("{branch}", branch)
            }
        }

        ReviewTarget::Commit { sha, title } => {
            let title_info = if let Some(t) = title {
                format!("Commit message: \"{}\"", t)
            } else {
                String::new()
            };
            COMMIT_PROMPT
                .replace("{sha}", sha)
                .replace("{title_info}", &title_info)
        }

        ReviewTarget::CommitRange { from, to } => {
            RANGE_PROMPT.replace("{from}", from).replace("{to}", to)
        }

        ReviewTarget::Custom { instructions } => instructions.clone(),
    }
}

/// Get merge base between HEAD and a branch.
pub async fn get_merge_base(repo_path: &Path, branch: &str) -> Result<Option<String>> {
    // First try to find the upstream
    let upstream = Command::new("git")
        .args([
            "rev-parse",
            "--abbrev-ref",
            &format!("{}@{{upstream}}", branch),
        ])
        .current_dir(repo_path)
        .output()
        .await?;

    let target_ref = if upstream.status.success() {
        String::from_utf8_lossy(&upstream.stdout).trim().to_string()
    } else {
        branch.to_string()
    };

    // Get merge base
    let output = Command::new("git")
        .args(["merge-base", "HEAD", &target_ref])
        .current_dir(repo_path)
        .output()
        .await?;

    if output.status.success() {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Some(sha))
    } else {
        Ok(None)
    }
}

/// Get commit title from SHA.
pub async fn get_commit_title(repo_path: &Path, sha: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%s", sha])
        .current_dir(repo_path)
        .output()
        .await?;

    if output.status.success() {
        let title = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !title.is_empty() {
            return Ok(Some(title));
        }
    }

    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_uncommitted_prompt() {
        let target = ReviewTarget::UncommittedChanges;
        let prompt = build_review_prompt(&target, None);
        // Check for a reliable phrase from the prompt
        assert!(prompt.contains("current code changes"));
    }

    #[test]
    fn test_build_branch_prompt() {
        let target = ReviewTarget::against_branch("main");
        let prompt = build_review_prompt(&target, Some("abc123"));
        assert!(prompt.contains("main"));
        assert!(prompt.contains("abc123"));
    }
}
