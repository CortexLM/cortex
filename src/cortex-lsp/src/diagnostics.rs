//! LSP diagnostics types and utilities.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Diagnostic severity level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Information,
    Hint,
}

impl From<lsp_types::DiagnosticSeverity> for DiagnosticSeverity {
    fn from(severity: lsp_types::DiagnosticSeverity) -> Self {
        match severity {
            lsp_types::DiagnosticSeverity::ERROR => DiagnosticSeverity::Error,
            lsp_types::DiagnosticSeverity::WARNING => DiagnosticSeverity::Warning,
            lsp_types::DiagnosticSeverity::INFORMATION => DiagnosticSeverity::Information,
            lsp_types::DiagnosticSeverity::HINT => DiagnosticSeverity::Hint,
            _ => DiagnosticSeverity::Information,
        }
    }
}

impl DiagnosticSeverity {
    pub fn symbol(&self) -> &'static str {
        match self {
            DiagnosticSeverity::Error => "[ERROR]",
            DiagnosticSeverity::Warning => "[WARN]",
            DiagnosticSeverity::Information => "[INFO]",
            DiagnosticSeverity::Hint => "[HINT]",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            DiagnosticSeverity::Error => "error",
            DiagnosticSeverity::Warning => "warning",
            DiagnosticSeverity::Information => "info",
            DiagnosticSeverity::Hint => "hint",
        }
    }
}

/// A diagnostic message from LSP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    /// File path.
    pub path: PathBuf,
    /// Line number (1-based).
    pub line: u32,
    /// Column number (1-based).
    pub column: u32,
    /// End line (optional).
    pub end_line: Option<u32>,
    /// End column (optional).
    pub end_column: Option<u32>,
    /// Severity level.
    pub severity: DiagnosticSeverity,
    /// Diagnostic message.
    pub message: String,
    /// Source (e.g., "typescript", "eslint").
    pub source: Option<String>,
    /// Diagnostic code.
    pub code: Option<String>,
    /// Related information.
    pub related: Vec<RelatedInformation>,
}

impl Diagnostic {
    pub fn new(path: PathBuf, line: u32, column: u32, message: String) -> Self {
        Self {
            path,
            line,
            column,
            end_line: None,
            end_column: None,
            severity: DiagnosticSeverity::Error,
            message,
            source: None,
            code: None,
            related: Vec::new(),
        }
    }

    pub fn with_severity(mut self, severity: DiagnosticSeverity) -> Self {
        self.severity = severity;
        self
    }

    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    pub fn with_range(mut self, end_line: u32, end_column: u32) -> Self {
        self.end_line = Some(end_line);
        self.end_column = Some(end_column);
        self
    }

    /// Format diagnostic for display.
    pub fn format(&self) -> String {
        let location = format!("{}:{}:{}", self.path.display(), self.line, self.column);

        let source = self.source.as_deref().unwrap_or("lsp");
        let code = self
            .code
            .as_ref()
            .map(|c| format!("[{}]", c))
            .unwrap_or_default();

        format!(
            "{} {} {}{}: {}",
            self.severity.symbol(),
            location,
            source,
            code,
            self.message
        )
    }

    /// Format diagnostic for tool output.
    pub fn format_for_tool(&self) -> String {
        format!(
            "{}:{}:{}: {}: {}",
            self.path.display(),
            self.line,
            self.column,
            self.severity.label(),
            self.message
        )
    }
}

/// Related diagnostic information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelatedInformation {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub message: String,
}

/// Collection of diagnostics for a file.
#[derive(Debug, Clone, Default)]
pub struct FileDiagnostics {
    pub path: PathBuf,
    pub diagnostics: Vec<Diagnostic>,
}

impl FileDiagnostics {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            diagnostics: Vec::new(),
        }
    }

    pub fn add(&mut self, diagnostic: Diagnostic) {
        self.diagnostics.push(diagnostic);
    }

    pub fn errors(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Error)
    }

    pub fn warnings(&self) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(|d| d.severity == DiagnosticSeverity::Warning)
    }

    pub fn error_count(&self) -> usize {
        self.errors().count()
    }

    pub fn warning_count(&self) -> usize {
        self.warnings().count()
    }

    pub fn has_errors(&self) -> bool {
        self.error_count() > 0
    }

    pub fn format_summary(&self) -> String {
        let errors = self.error_count();
        let warnings = self.warning_count();
        format!(
            "{}: {} error(s), {} warning(s)",
            self.path.display(),
            errors,
            warnings
        )
    }
}

/// Diagnostics for the entire workspace.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceDiagnostics {
    pub files: std::collections::HashMap<PathBuf, FileDiagnostics>,
}

impl WorkspaceDiagnostics {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_file_diagnostics(&mut self, path: PathBuf, diagnostics: Vec<Diagnostic>) {
        let mut file_diags = FileDiagnostics::new(path.clone());
        file_diags.diagnostics = diagnostics;
        self.files.insert(path, file_diags);
    }

    pub fn clear_file(&mut self, path: &PathBuf) {
        self.files.remove(path);
    }

    pub fn all_diagnostics(&self) -> impl Iterator<Item = &Diagnostic> {
        self.files.values().flat_map(|f| f.diagnostics.iter())
    }

    pub fn total_errors(&self) -> usize {
        self.files.values().map(|f| f.error_count()).sum()
    }

    pub fn total_warnings(&self) -> usize {
        self.files.values().map(|f| f.warning_count()).sum()
    }

    pub fn format_summary(&self) -> String {
        let files = self.files.len();
        let errors = self.total_errors();
        let warnings = self.total_warnings();
        format!(
            "{} file(s) with diagnostics: {} error(s), {} warning(s)",
            files, errors, warnings
        )
    }
}
