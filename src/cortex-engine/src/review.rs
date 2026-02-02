//! Code review utilities.
//!
//! Provides comprehensive code review functionality including
//! diff analysis, change summarization, and review formatting.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Review mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReviewMode {
    /// Quick overview.
    Quick,
    /// Standard review.
    #[default]
    Standard,
    /// Detailed review.
    Detailed,
    /// Security-focused review.
    Security,
    /// Performance-focused review.
    Performance,
}

/// Review scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ReviewScope {
    /// Review all changes.
    #[default]
    All,
    /// Review staged changes.
    Staged,
    /// Review unstaged changes.
    Unstaged,
    /// Review specific commit.
    Commit,
    /// Review commit range.
    Range,
}

/// Review configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewConfig {
    /// Review mode.
    pub mode: ReviewMode,
    /// Review scope.
    pub scope: ReviewScope,
    /// Include context lines.
    pub context_lines: u32,
    /// Maximum files to review.
    pub max_files: usize,
    /// Maximum diff size (bytes).
    pub max_diff_size: usize,
    /// Focus areas.
    pub focus_areas: Vec<String>,
    /// Ignore patterns.
    pub ignore_patterns: Vec<String>,
    /// Include test files.
    pub include_tests: bool,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            mode: ReviewMode::Standard,
            scope: ReviewScope::All,
            context_lines: 3,
            max_files: 50,
            max_diff_size: 100_000,
            focus_areas: Vec::new(),
            ignore_patterns: vec![
                "*.lock".to_string(),
                "*.min.js".to_string(),
                "*.min.css".to_string(),
            ],
            include_tests: true,
        }
    }
}

/// File change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// File path.
    pub path: PathBuf,
    /// Change type.
    pub change_type: ChangeType,
    /// Lines added.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// Hunks.
    pub hunks: Vec<Hunk>,
    /// Language.
    pub language: Option<String>,
    /// Is binary.
    pub is_binary: bool,
}

impl FileChange {
    /// Create a new file change.
    pub fn new(path: impl Into<PathBuf>, change_type: ChangeType) -> Self {
        Self {
            path: path.into(),
            change_type,
            additions: 0,
            deletions: 0,
            hunks: Vec::new(),
            language: None,
            is_binary: false,
        }
    }

    /// Get total lines changed.
    pub fn lines_changed(&self) -> u32 {
        self.additions + self.deletions
    }

    /// Get change summary.
    pub fn summary(&self) -> String {
        format!(
            "{} (+{}, -{})",
            self.path.display(),
            self.additions,
            self.deletions
        )
    }
}

/// Change type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeType {
    /// Added file.
    Added,
    /// Modified file.
    Modified,
    /// Deleted file.
    Deleted,
    /// Renamed file.
    Renamed,
    /// Copied file.
    Copied,
}

impl ChangeType {
    /// Get display string.
    pub fn display(&self) -> &'static str {
        match self {
            Self::Added => "A",
            Self::Modified => "M",
            Self::Deleted => "D",
            Self::Renamed => "R",
            Self::Copied => "C",
        }
    }

    /// Get full name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Renamed => "renamed",
            Self::Copied => "copied",
        }
    }
}

/// Diff hunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hunk {
    /// Old start line.
    pub old_start: u32,
    /// Old line count.
    pub old_count: u32,
    /// New start line.
    pub new_start: u32,
    /// New line count.
    pub new_count: u32,
    /// Header text.
    pub header: String,
    /// Lines.
    pub lines: Vec<DiffLine>,
}

impl Hunk {
    /// Create a new hunk.
    pub fn new(old_start: u32, old_count: u32, new_start: u32, new_count: u32) -> Self {
        Self {
            old_start,
            old_count,
            new_start,
            new_count,
            header: String::new(),
            lines: Vec::new(),
        }
    }

    /// Get header string.
    pub fn header_string(&self) -> String {
        format!(
            "@@ -{},{} +{},{} @@{}",
            self.old_start,
            self.old_count,
            self.new_start,
            self.new_count,
            if self.header.is_empty() { "" } else { " " }
        ) + &self.header
    }
}

/// Diff line.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffLine {
    /// Line type.
    pub line_type: LineType,
    /// Line content.
    pub content: String,
    /// Old line number.
    pub old_line: Option<u32>,
    /// New line number.
    pub new_line: Option<u32>,
}

impl DiffLine {
    /// Create a context line.
    pub fn context(content: impl Into<String>, old_line: u32, new_line: u32) -> Self {
        Self {
            line_type: LineType::Context,
            content: content.into(),
            old_line: Some(old_line),
            new_line: Some(new_line),
        }
    }

    /// Create an addition line.
    pub fn addition(content: impl Into<String>, new_line: u32) -> Self {
        Self {
            line_type: LineType::Addition,
            content: content.into(),
            old_line: None,
            new_line: Some(new_line),
        }
    }

    /// Create a deletion line.
    pub fn deletion(content: impl Into<String>, old_line: u32) -> Self {
        Self {
            line_type: LineType::Deletion,
            content: content.into(),
            old_line: Some(old_line),
            new_line: None,
        }
    }

    /// Get prefix character.
    pub fn prefix(&self) -> char {
        match self.line_type {
            LineType::Context => ' ',
            LineType::Addition => '+',
            LineType::Deletion => '-',
        }
    }
}

/// Line type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineType {
    /// Context line.
    Context,
    /// Added line.
    Addition,
    /// Deleted line.
    Deletion,
}

/// Review result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResult {
    /// Files reviewed.
    pub files: Vec<FileChange>,
    /// Summary.
    pub summary: ReviewSummary,
    /// Findings.
    pub findings: Vec<Finding>,
    /// Suggestions.
    pub suggestions: Vec<Suggestion>,
}

impl ReviewResult {
    /// Create a new review result.
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            summary: ReviewSummary::default(),
            findings: Vec::new(),
            suggestions: Vec::new(),
        }
    }

    /// Add a file.
    pub fn add_file(&mut self, file: FileChange) {
        self.summary.total_additions += file.additions;
        self.summary.total_deletions += file.deletions;
        self.summary.files_changed += 1;
        self.files.push(file);
    }

    /// Add a finding.
    pub fn add_finding(&mut self, finding: Finding) {
        self.findings.push(finding);
    }

    /// Add a suggestion.
    pub fn add_suggestion(&mut self, suggestion: Suggestion) {
        self.suggestions.push(suggestion);
    }

    /// Format as markdown.
    pub fn format_markdown(&self) -> String {
        let mut output = String::new();

        output.push_str("# Code Review\n\n");

        // Summary
        output.push_str("## Summary\n\n");
        output.push_str(&format!(
            "- Files changed: {}\n",
            self.summary.files_changed
        ));
        output.push_str(&format!(
            "- Lines added: +{}\n",
            self.summary.total_additions
        ));
        output.push_str(&format!(
            "- Lines removed: -{}\n",
            self.summary.total_deletions
        ));
        output.push('\n');

        // Files
        output.push_str("## Changed Files\n\n");
        for file in &self.files {
            output.push_str(&format!(
                "- `{}` ({}) +{} -{}\n",
                file.path.display(),
                file.change_type.name(),
                file.additions,
                file.deletions
            ));
        }
        output.push('\n');

        // Findings
        if !self.findings.is_empty() {
            output.push_str("## Findings\n\n");
            for finding in &self.findings {
                output.push_str(&format!(
                    "### {} ({})\n\n",
                    finding.title,
                    finding.severity.name()
                ));
                output.push_str(&finding.description);
                output.push_str("\n\n");
                if let Some(ref path) = finding.file {
                    output.push_str(&format!("**File:** `{}`", path.display()));
                    if let Some(line) = finding.line {
                        output.push_str(&format!(" (line {line})"));
                    }
                    output.push_str("\n\n");
                }
            }
        }

        // Suggestions
        if !self.suggestions.is_empty() {
            output.push_str("## Suggestions\n\n");
            for (i, suggestion) in self.suggestions.iter().enumerate() {
                output.push_str(&format!("{}. {}\n", i + 1, suggestion.description));
            }
        }

        output
    }
}

impl Default for ReviewResult {
    fn default() -> Self {
        Self::new()
    }
}

/// Review summary.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReviewSummary {
    /// Files changed.
    pub files_changed: u32,
    /// Total additions.
    pub total_additions: u32,
    /// Total deletions.
    pub total_deletions: u32,
}

impl ReviewSummary {
    /// Get total lines changed.
    pub fn total_lines(&self) -> u32 {
        self.total_additions + self.total_deletions
    }
}

/// Review finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Title.
    pub title: String,
    /// Description.
    pub description: String,
    /// Severity.
    pub severity: Severity,
    /// Category.
    pub category: FindingCategory,
    /// File path.
    pub file: Option<PathBuf>,
    /// Line number.
    pub line: Option<u32>,
    /// Code snippet.
    pub snippet: Option<String>,
}

impl Finding {
    /// Create a new finding.
    pub fn new(
        title: impl Into<String>,
        description: impl Into<String>,
        severity: Severity,
    ) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            severity,
            category: FindingCategory::Other,
            file: None,
            line: None,
            snippet: None,
        }
    }

    /// Set category.
    pub fn with_category(mut self, category: FindingCategory) -> Self {
        self.category = category;
        self
    }

    /// Set file location.
    pub fn at_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Set line number.
    pub fn at_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    /// Set code snippet.
    pub fn with_snippet(mut self, snippet: impl Into<String>) -> Self {
        self.snippet = Some(snippet.into());
        self
    }
}

/// Finding severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Severity {
    /// Informational.
    #[default]
    Info,
    /// Warning.
    Warning,
    /// Error.
    Error,
    /// Critical.
    Critical,
}

impl Severity {
    /// Get display name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Info => "Info",
            Self::Warning => "Warning",
            Self::Error => "Error",
            Self::Critical => "Critical",
        }
    }

    /// Get emoji.
    pub fn emoji(&self) -> &'static str {
        match self {
            Self::Info => "[I]",
            Self::Warning => "[W]",
            Self::Error => "[E]",
            Self::Critical => "[!]",
        }
    }
}

/// Finding category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum FindingCategory {
    /// Security issue.
    Security,
    /// Performance issue.
    Performance,
    /// Code style.
    Style,
    /// Logic error.
    Logic,
    /// Best practices.
    BestPractice,
    /// Documentation.
    Documentation,
    /// Testing.
    Testing,
    /// Other.
    #[default]
    Other,
}

/// Suggestion for improvement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    /// Description.
    pub description: String,
    /// Priority.
    pub priority: Priority,
    /// File.
    pub file: Option<PathBuf>,
    /// Original code.
    pub original: Option<String>,
    /// Suggested code.
    pub suggested: Option<String>,
}

impl Suggestion {
    /// Create a new suggestion.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            description: description.into(),
            priority: Priority::Medium,
            file: None,
            original: None,
            suggested: None,
        }
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Set file.
    pub fn at_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.file = Some(file.into());
        self
    }

    /// Set code change.
    pub fn with_change(
        mut self,
        original: impl Into<String>,
        suggested: impl Into<String>,
    ) -> Self {
        self.original = Some(original.into());
        self.suggested = Some(suggested.into());
        self
    }
}

/// Priority level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Priority {
    /// Low priority.
    Low,
    /// Medium priority.
    #[default]
    Medium,
    /// High priority.
    High,
}

/// Review formatter.
pub struct ReviewFormatter {
    /// Include line numbers.
    pub line_numbers: bool,
    /// Include colors.
    pub colors: bool,
    /// Maximum line width.
    pub max_width: usize,
}

impl ReviewFormatter {
    /// Create a new formatter.
    pub fn new() -> Self {
        Self {
            line_numbers: true,
            colors: true,
            max_width: 120,
        }
    }

    /// Format a file change.
    pub fn format_file(&self, file: &FileChange) -> String {
        let mut output = String::new();

        output.push_str(&format!(
            "=== {} ({}) ===\n",
            file.path.display(),
            file.change_type.name()
        ));

        for hunk in &file.hunks {
            output.push_str(&hunk.header_string());
            output.push('\n');

            for line in &hunk.lines {
                output.push(line.prefix());
                output.push_str(&line.content);
                output.push('\n');
            }
        }

        output
    }

    /// Format a complete review.
    pub fn format_review(&self, review: &ReviewResult) -> String {
        let mut output = String::new();

        // Header
        output.push_str(&format!(
            "Code Review: {} files changed, +{} -{}\n",
            review.summary.files_changed,
            review.summary.total_additions,
            review.summary.total_deletions
        ));
        output.push_str(&"=".repeat(60));
        output.push('\n');
        output.push('\n');

        // Files
        for file in &review.files {
            output.push_str(&self.format_file(file));
            output.push('\n');
        }

        // Findings
        if !review.findings.is_empty() {
            output.push_str("FINDINGS:\n");
            for finding in &review.findings {
                output.push_str(&format!(
                    "  [{:?}] {}: {}\n",
                    finding.severity, finding.title, finding.description
                ));
            }
            output.push('\n');
        }

        // Suggestions
        if !review.suggestions.is_empty() {
            output.push_str("SUGGESTIONS:\n");
            for suggestion in &review.suggestions {
                output.push_str(&format!("  - {}\n", suggestion.description));
            }
        }

        output
    }
}

impl Default for ReviewFormatter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_change() {
        let mut file = FileChange::new("/test/file.rs", ChangeType::Modified);
        file.additions = 10;
        file.deletions = 5;

        assert_eq!(file.lines_changed(), 15);
        assert!(file.summary().contains("+10"));
    }

    #[test]
    fn test_change_type() {
        assert_eq!(ChangeType::Added.display(), "A");
        assert_eq!(ChangeType::Modified.name(), "modified");
    }

    #[test]
    fn test_diff_line() {
        let addition = DiffLine::addition("new code", 10);
        assert_eq!(addition.prefix(), '+');
        assert_eq!(addition.new_line, Some(10));
        assert_eq!(addition.old_line, None);

        let deletion = DiffLine::deletion("old code", 5);
        assert_eq!(deletion.prefix(), '-');
    }

    #[test]
    fn test_hunk() {
        let hunk = Hunk::new(1, 5, 1, 7);
        assert!(hunk.header_string().contains("@@ -1,5 +1,7 @@"));
    }

    #[test]
    fn test_review_result() {
        let mut review = ReviewResult::new();

        let mut file = FileChange::new("/test.rs", ChangeType::Modified);
        file.additions = 20;
        file.deletions = 10;
        review.add_file(file);

        assert_eq!(review.summary.files_changed, 1);
        assert_eq!(review.summary.total_additions, 20);
        assert_eq!(review.summary.total_deletions, 10);
    }

    #[test]
    fn test_finding() {
        let finding = Finding::new("Test Issue", "Description", Severity::Warning)
            .with_category(FindingCategory::Security)
            .at_file("/test.rs")
            .at_line(42);

        assert_eq!(finding.severity, Severity::Warning);
        assert_eq!(finding.line, Some(42));
    }

    #[test]
    fn test_severity_order() {
        assert!(Severity::Critical > Severity::Error);
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn test_markdown_output() {
        let mut review = ReviewResult::new();

        let mut file = FileChange::new("/test.rs", ChangeType::Added);
        file.additions = 50;
        review.add_file(file);

        review.add_finding(Finding::new("Issue", "Found an issue", Severity::Warning));

        let markdown = review.format_markdown();
        assert!(markdown.contains("# Code Review"));
        assert!(markdown.contains("Files changed: 1"));
        assert!(markdown.contains("## Findings"));
    }
}
