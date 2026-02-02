//! Code review prompts for Cortex CLI.
//!
//! This module contains prompts used for various code review scenarios,
//! including uncommitted changes, branch comparisons, and commit reviews.

/// Prompt for reviewing uncommitted changes (staged and unstaged).
pub const UNCOMMITTED_CHANGES_PROMPT: &str = r#"Review the current code changes (staged, unstaged, and untracked files) and provide prioritized findings.

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

/// Prompt template for reviewing against a base branch.
///
/// Placeholders:
/// - `{branch}` - The base branch name
/// - `{merge_base}` - The merge base commit SHA
pub const BASE_BRANCH_PROMPT: &str = r#"Review the code changes against the base branch '{branch}'.

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

/// Fallback prompt for reviewing against a base branch when merge base isn't available.
///
/// Placeholder: `{branch}` - The base branch name
pub const BASE_BRANCH_PROMPT_FALLBACK: &str = r#"Review the code changes against the base branch '{branch}'.

First find the merge base:
```bash
git merge-base HEAD "$(git rev-parse --abbrev-ref '{branch}@{{upstream}}')"
```

Then run `git diff <merge_base_sha>` to see the changes.

Provide prioritized, actionable findings."#;

/// Prompt template for reviewing a specific commit.
///
/// Placeholders:
/// - `{sha}` - The commit SHA
/// - `{title_info}` - Optional commit title info
pub const COMMIT_REVIEW_PROMPT: &str = r#"Review the code changes introduced by commit {sha}.

Run `git show {sha}` to see the commit details and changes.

{title_info}

Provide prioritized, actionable findings focusing on:
1. **Bugs**: Logic errors introduced by this commit
2. **Code Quality**: Does this commit follow best practices?
3. **Completeness**: Are there missing pieces?
4. **Testing**: Should there be tests for these changes?"#;

/// Prompt template for reviewing a range of commits.
///
/// Placeholders:
/// - `{from}` - Starting commit SHA
/// - `{to}` - Ending commit SHA
pub const COMMIT_RANGE_PROMPT: &str = r#"Review the code changes in commits {from}..{to}.

Run `git log --oneline {from}..{to}` to see the commits.
Run `git diff {from}..{to}` to see all changes.

Provide prioritized, actionable findings for this set of changes."#;

/// Build a review prompt based on the target type.
///
/// # Arguments
///
/// * `target` - The type of review target
///
/// # Returns
///
/// The formatted review prompt string.
pub fn build_review_prompt(target: ReviewTarget) -> String {
    match target {
        ReviewTarget::Uncommitted => UNCOMMITTED_CHANGES_PROMPT.to_string(),

        ReviewTarget::BaseBranch {
            branch,
            merge_base: Some(base),
        } => BASE_BRANCH_PROMPT
            .replace("{branch}", &branch)
            .replace("{merge_base}", &base),

        ReviewTarget::BaseBranch {
            branch,
            merge_base: None,
        } => BASE_BRANCH_PROMPT_FALLBACK.replace("{branch}", &branch),

        ReviewTarget::Commit { sha, title } => {
            let title_info = if let Some(t) = title {
                format!("Commit message: \"{}\"", t)
            } else {
                String::new()
            };
            COMMIT_REVIEW_PROMPT
                .replace("{sha}", &sha)
                .replace("{title_info}", &title_info)
        }

        ReviewTarget::CommitRange { from, to } => COMMIT_RANGE_PROMPT
            .replace("{from}", &from)
            .replace("{to}", &to),

        ReviewTarget::Custom { instructions } => instructions,
    }
}

/// Target for code review operations.
#[derive(Debug, Clone)]
pub enum ReviewTarget {
    /// Review uncommitted changes (staged and unstaged).
    Uncommitted,
    /// Review against a base branch.
    BaseBranch {
        branch: String,
        merge_base: Option<String>,
    },
    /// Review a specific commit.
    Commit { sha: String, title: Option<String> },
    /// Review a range of commits.
    CommitRange { from: String, to: String },
    /// Custom review instructions.
    Custom { instructions: String },
}

impl ReviewTarget {
    /// Create a review target for uncommitted changes.
    pub fn uncommitted() -> Self {
        Self::Uncommitted
    }

    /// Create a review target for a base branch.
    pub fn against_branch(branch: impl Into<String>) -> Self {
        Self::BaseBranch {
            branch: branch.into(),
            merge_base: None,
        }
    }

    /// Create a review target for a base branch with known merge base.
    pub fn against_branch_with_base(
        branch: impl Into<String>,
        merge_base: impl Into<String>,
    ) -> Self {
        Self::BaseBranch {
            branch: branch.into(),
            merge_base: Some(merge_base.into()),
        }
    }

    /// Create a review target for a specific commit.
    pub fn commit(sha: impl Into<String>) -> Self {
        Self::Commit {
            sha: sha.into(),
            title: None,
        }
    }

    /// Create a review target for a specific commit with title.
    pub fn commit_with_title(sha: impl Into<String>, title: impl Into<String>) -> Self {
        Self::Commit {
            sha: sha.into(),
            title: Some(title.into()),
        }
    }

    /// Create a review target for a range of commits.
    pub fn commit_range(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self::CommitRange {
            from: from.into(),
            to: to.into(),
        }
    }

    /// Create a custom review target.
    pub fn custom(instructions: impl Into<String>) -> Self {
        Self::Custom {
            instructions: instructions.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uncommitted_prompt() {
        let prompt = build_review_prompt(ReviewTarget::uncommitted());
        assert!(prompt.contains("staged, unstaged, and untracked"));
        assert!(prompt.contains("Critical Issues"));
    }

    #[test]
    fn test_base_branch_prompt_with_merge_base() {
        let prompt = build_review_prompt(ReviewTarget::against_branch_with_base("main", "abc123"));
        assert!(prompt.contains("main"));
        assert!(prompt.contains("abc123"));
        assert!(prompt.contains("merge base"));
    }

    #[test]
    fn test_base_branch_prompt_fallback() {
        let prompt = build_review_prompt(ReviewTarget::against_branch("main"));
        assert!(prompt.contains("main"));
        assert!(prompt.contains("git merge-base"));
    }

    #[test]
    fn test_commit_prompt() {
        let prompt = build_review_prompt(ReviewTarget::commit_with_title("abc123", "Fix bug"));
        assert!(prompt.contains("abc123"));
        assert!(prompt.contains("Fix bug"));
    }

    #[test]
    fn test_commit_range_prompt() {
        let prompt = build_review_prompt(ReviewTarget::commit_range("abc123", "def456"));
        assert!(prompt.contains("abc123"));
        assert!(prompt.contains("def456"));
    }
}
