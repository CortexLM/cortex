//! Code review task.
//!
//! Handles reviewing code changes and generating review reports.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{TaskMeta, TaskType};

/// Review format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReviewFormat {
    /// Plain text.
    Text,
    /// Markdown.
    #[default]
    Markdown,
    /// JSON.
    Json,
    /// GitHub-style review.
    GitHub,
    /// Inline comments.
    Inline,
}

/// Review severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReviewSeverity {
    /// Informational note.
    #[default]
    Info,
    /// Suggestion for improvement.
    Suggestion,
    /// Warning about potential issues.
    Warning,
    /// Error that should be fixed.
    Error,
    /// Critical issue.
    Critical,
}

/// Review task for code review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewTask {
    /// Task metadata.
    pub meta: TaskMeta,
    /// Files to review.
    pub files: Vec<FileChange>,
    /// Review format.
    pub format: ReviewFormat,
    /// Focus areas.
    pub focus: Vec<ReviewFocus>,
    /// Include suggestions.
    pub include_suggestions: bool,
    /// Strict mode (fail on any error).
    pub strict: bool,
}

impl ReviewTask {
    /// Create a new review task.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            meta: TaskMeta::new(id, TaskType::Review),
            files: Vec::new(),
            format: ReviewFormat::Markdown,
            focus: Vec::new(),
            include_suggestions: true,
            strict: false,
        }
    }

    /// Add a file to review.
    pub fn add_file(mut self, file: FileChange) -> Self {
        self.files.push(file);
        self
    }

    /// Set format.
    pub fn with_format(mut self, format: ReviewFormat) -> Self {
        self.format = format;
        self
    }

    /// Add focus area.
    pub fn with_focus(mut self, focus: ReviewFocus) -> Self {
        self.focus.push(focus);
        self
    }

    /// Set strict mode.
    pub fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    /// Generate review prompt.
    pub fn review_prompt(&self) -> String {
        let mut prompt = String::from("Review the following code changes:\n\n");

        for file in &self.files {
            prompt.push_str(&format!("## File: {}\n", file.path.display()));
            prompt.push_str(&format!("Change type: {:?}\n\n", file.change_type));

            if let Some(ref diff) = file.diff {
                prompt.push_str("```diff\n");
                prompt.push_str(diff);
                prompt.push_str("\n```\n\n");
            }
        }

        if !self.focus.is_empty() {
            prompt.push_str("Focus on:\n");
            for focus in &self.focus {
                prompt.push_str(&format!("- {focus:?}\n"));
            }
            prompt.push('\n');
        }

        prompt.push_str("Provide a detailed review with:\n");
        prompt.push_str("1. Summary of changes\n");
        prompt.push_str("2. Issues found (if any)\n");
        prompt.push_str("3. Suggestions for improvement\n");
        prompt.push_str("4. Overall assessment\n");

        prompt
    }
}

/// Review focus area.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewFocus {
    /// Security issues.
    Security,
    /// Performance issues.
    Performance,
    /// Code quality.
    Quality,
    /// Error handling.
    ErrorHandling,
    /// Documentation.
    Documentation,
    /// Testing.
    Testing,
    /// Type safety.
    TypeSafety,
    /// Best practices.
    BestPractices,
    /// Accessibility.
    Accessibility,
}

/// File change for review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// File path.
    pub path: PathBuf,
    /// Change type.
    pub change_type: ChangeType,
    /// Original content.
    pub original: Option<String>,
    /// New content.
    pub modified: Option<String>,
    /// Diff.
    pub diff: Option<String>,
    /// Language.
    pub language: Option<String>,
}

impl FileChange {
    /// Create a new file change.
    pub fn new(path: impl Into<PathBuf>, change_type: ChangeType) -> Self {
        Self {
            path: path.into(),
            change_type,
            original: None,
            modified: None,
            diff: None,
            language: None,
        }
    }

    /// Set original content.
    pub fn with_original(mut self, content: impl Into<String>) -> Self {
        self.original = Some(content.into());
        self
    }

    /// Set modified content.
    pub fn with_modified(mut self, content: impl Into<String>) -> Self {
        self.modified = Some(content.into());
        self
    }

    /// Set diff.
    pub fn with_diff(mut self, diff: impl Into<String>) -> Self {
        self.diff = Some(diff.into());
        self
    }

    /// Set language.
    pub fn with_language(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Detect language from path.
    pub fn detect_language(&mut self) {
        if let Some(ext) = self.path.extension().and_then(|e| e.to_str()) {
            self.language = Some(
                match ext {
                    "rs" => "rust",
                    "py" => "python",
                    "js" => "javascript",
                    "ts" => "typescript",
                    "tsx" => "typescript",
                    "jsx" => "javascript",
                    "go" => "go",
                    "java" => "java",
                    "rb" => "ruby",
                    "cpp" | "cc" | "cxx" => "cpp",
                    "c" | "h" => "c",
                    "cs" => "csharp",
                    "swift" => "swift",
                    "kt" | "kts" => "kotlin",
                    "scala" => "scala",
                    "php" => "php",
                    "sh" | "bash" => "bash",
                    "sql" => "sql",
                    "html" => "html",
                    "css" => "css",
                    "json" => "json",
                    "yaml" | "yml" => "yaml",
                    "toml" => "toml",
                    "md" => "markdown",
                    _ => ext,
                }
                .to_string(),
            );
        }
    }
}

/// Change type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    /// File added.
    Added,
    /// File modified.
    Modified,
    /// File deleted.
    Deleted,
    /// File renamed.
    Renamed,
    /// File copied.
    Copied,
}

/// Review result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    /// Overall summary.
    pub summary: String,
    /// Individual comments.
    pub comments: Vec<ReviewComment>,
    /// Overall assessment.
    pub assessment: ReviewAssessment,
    /// Statistics.
    pub stats: ReviewStats,
}

impl ReviewResult {
    /// Create a new review result.
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            comments: Vec::new(),
            assessment: ReviewAssessment::Approved,
            stats: ReviewStats::default(),
        }
    }

    /// Add a comment.
    pub fn add_comment(mut self, comment: ReviewComment) -> Self {
        self.stats.add_comment(&comment);
        self.comments.push(comment);
        self
    }

    /// Set assessment.
    pub fn with_assessment(mut self, assessment: ReviewAssessment) -> Self {
        self.assessment = assessment;
        self
    }

    /// Format as markdown.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();

        md.push_str("# Code Review\n\n");
        md.push_str(&format!("## Summary\n\n{}\n\n", self.summary));

        md.push_str(&format!("## Assessment: {:?}\n\n", self.assessment));

        if !self.comments.is_empty() {
            md.push_str("## Comments\n\n");

            for comment in &self.comments {
                let icon = match comment.severity {
                    ReviewSeverity::Critical => "[!]",
                    ReviewSeverity::Error => "[E]",
                    ReviewSeverity::Warning => "[W]",
                    ReviewSeverity::Suggestion => "[S]",
                    ReviewSeverity::Info => "[I]",
                };

                md.push_str(&format!("### {} {:?}\n\n", icon, comment.severity));

                if let Some(ref path) = comment.file {
                    md.push_str(&format!("**File:** `{}`", path.display()));
                    if let Some(line) = comment.line {
                        md.push_str(&format!(", line {line}"));
                    }
                    md.push_str("\n\n");
                }

                md.push_str(&format!("{}\n\n", comment.message));

                if let Some(ref suggestion) = comment.suggestion {
                    md.push_str(&format!("**Suggestion:** {suggestion}\n\n"));
                }
            }
        }

        md.push_str("## Statistics\n\n");
        md.push_str(&format!("- Critical: {}\n", self.stats.critical));
        md.push_str(&format!("- Errors: {}\n", self.stats.errors));
        md.push_str(&format!("- Warnings: {}\n", self.stats.warnings));
        md.push_str(&format!("- Suggestions: {}\n", self.stats.suggestions));
        md.push_str(&format!("- Info: {}\n", self.stats.info));

        md
    }

    /// Check if review passes.
    pub fn passes(&self, strict: bool) -> bool {
        if strict {
            self.stats.critical == 0 && self.stats.errors == 0 && self.stats.warnings == 0
        } else {
            self.stats.critical == 0 && self.stats.errors == 0
        }
    }
}

/// Review comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    /// File path.
    pub file: Option<PathBuf>,
    /// Line number.
    pub line: Option<u32>,
    /// End line (for ranges).
    pub end_line: Option<u32>,
    /// Severity.
    pub severity: ReviewSeverity,
    /// Message.
    pub message: String,
    /// Suggested fix.
    pub suggestion: Option<String>,
    /// Category.
    pub category: Option<String>,
}

impl ReviewComment {
    /// Create a new comment.
    pub fn new(severity: ReviewSeverity, message: impl Into<String>) -> Self {
        Self {
            file: None,
            line: None,
            end_line: None,
            severity,
            message: message.into(),
            suggestion: None,
            category: None,
        }
    }

    /// Set file.
    pub fn in_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.file = Some(path.into());
        self
    }

    /// Set line.
    pub fn at_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    /// Set line range.
    pub fn at_lines(mut self, start: u32, end: u32) -> Self {
        self.line = Some(start);
        self.end_line = Some(end);
        self
    }

    /// Set suggestion.
    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    /// Set category.
    pub fn with_category(mut self, category: impl Into<String>) -> Self {
        self.category = Some(category.into());
        self
    }
}

/// Review assessment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewAssessment {
    /// Approved without changes.
    Approved,
    /// Approved with minor suggestions.
    ApprovedWithSuggestions,
    /// Changes requested.
    ChangesRequested,
    /// Rejected.
    Rejected,
}

/// Review statistics.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewStats {
    /// Critical issues.
    pub critical: u32,
    /// Errors.
    pub errors: u32,
    /// Warnings.
    pub warnings: u32,
    /// Suggestions.
    pub suggestions: u32,
    /// Info.
    pub info: u32,
    /// Files reviewed.
    pub files_reviewed: u32,
    /// Lines reviewed.
    pub lines_reviewed: u32,
}

impl ReviewStats {
    /// Add a comment to stats.
    pub fn add_comment(&mut self, comment: &ReviewComment) {
        match comment.severity {
            ReviewSeverity::Critical => self.critical += 1,
            ReviewSeverity::Error => self.errors += 1,
            ReviewSeverity::Warning => self.warnings += 1,
            ReviewSeverity::Suggestion => self.suggestions += 1,
            ReviewSeverity::Info => self.info += 1,
        }
    }

    /// Get total comments.
    pub fn total(&self) -> u32 {
        self.critical + self.errors + self.warnings + self.suggestions + self.info
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_task() {
        let task = ReviewTask::new("review-1")
            .with_format(ReviewFormat::Markdown)
            .with_focus(ReviewFocus::Security);

        assert_eq!(task.format, ReviewFormat::Markdown);
        assert!(task.focus.contains(&ReviewFocus::Security));
    }

    #[test]
    fn test_review_result() {
        let result = ReviewResult::new("Good code")
            .add_comment(ReviewComment::new(
                ReviewSeverity::Warning,
                "Consider error handling",
            ))
            .with_assessment(ReviewAssessment::ApprovedWithSuggestions);

        assert_eq!(result.stats.warnings, 1);
        assert!(result.passes(false));
        assert!(!result.passes(true));
    }

    #[test]
    fn test_file_change() {
        let mut change = FileChange::new("/src/main.rs", ChangeType::Modified);
        change.detect_language();

        assert_eq!(change.language, Some("rust".to_string()));
    }

    #[test]
    fn test_to_markdown() {
        let result = ReviewResult::new("Summary")
            .add_comment(ReviewComment::new(ReviewSeverity::Info, "Test comment"));

        let md = result.to_markdown();
        assert!(md.contains("# Code Review"));
        assert!(md.contains("Summary"));
    }
}
