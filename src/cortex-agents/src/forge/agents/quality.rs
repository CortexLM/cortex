//! Code quality validation agent for Forge system.
//!
//! Performs code quality analysis including:
//! - Detection of TODO/FIXME/HACK comments
//! - Detection of unimplemented!() and todo!() macros
//! - Error handling quality checks (unwrap without context)
//! - Dead code detection via attributes
//! - Documentation coverage for public items

use async_trait::async_trait;
use std::path::Path;
use std::time::Instant;
use tokio::fs;
use tokio::io::AsyncBufReadExt;

use super::{
    utils::{collect_rust_files, is_test_path, truncate_line},
    AgentError, Finding, RuleInfo, Severity, ValidationAgent, ValidationContext, ValidationResult,
};

// ============================================================================
// QualityAgent
// ============================================================================

/// Code quality validation agent.
///
/// Checks for:
/// - `todo_comments`: TODO, FIXME, HACK comments
/// - `unimplemented_code`: unimplemented!() and todo!() macros
/// - `error_handling`: unwrap() without context
/// - `dead_code`: Items marked with #[allow(dead_code)]
/// - `documentation`: Missing docs on public items
#[derive(Debug, Clone)]
pub struct QualityAgent {
    /// Agent identifier.
    id: String,
    /// Agent name.
    name: String,
}

impl QualityAgent {
    /// Create a new quality agent with default settings.
    pub fn new() -> Self {
        Self {
            id: "quality".to_string(),
            name: "Code Quality Agent".to_string(),
        }
    }

    /// Check for TODO/FIXME/HACK comments.
    async fn check_todo_comments(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "todo_comments";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Warning);
        let files = collect_rust_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if let Err(e) = self
                .scan_file_for_todos(&file_path, rule_id, severity, findings)
                .await
            {
                tracing::warn!(
                    "Failed to scan file {} for todos: {}",
                    file_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Scan a single file for TODO/FIXME/HACK comments.
    async fn scan_file_for_todos(
        &self,
        file_path: &Path,
        rule_id: &str,
        severity: Severity,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let file = fs::File::open(file_path).await?;
        let reader = tokio::io::BufReader::new(file);
        let mut lines = reader.lines();
        let mut line_num: u32 = 0;

        let patterns = [
            ("TODO", "Incomplete task marked with TODO"),
            ("FIXME", "Bug or issue marked with FIXME"),
            ("HACK", "Workaround or hack that needs proper solution"),
            ("XXX", "Attention required"),
        ];

        while let Some(line) = lines.next_line().await? {
            line_num += 1;
            let upper = line.to_uppercase();

            for (pattern, description) in &patterns {
                // Check for pattern in comments
                if let Some(comment_start) = line.find("//") {
                    let comment = &line[comment_start..];
                    if comment.to_uppercase().contains(pattern) {
                        findings.push(
                            Finding::new(rule_id, severity, *description)
                                .at_file(file_path)
                                .at_line(line_num)
                                .with_snippet(truncate_line(&line, 80))
                                .with_suggestion("Complete the task or create a tracking issue"),
                        );
                        break;
                    }
                }

                // Check for pattern in block comments
                if upper.contains(&format!("/* {}", pattern))
                    || upper.contains(&format!("* {}", pattern))
                {
                    findings.push(
                        Finding::new(rule_id, severity, *description)
                            .at_file(file_path)
                            .at_line(line_num)
                            .with_snippet(truncate_line(&line, 80))
                            .with_suggestion("Complete the task or create a tracking issue"),
                    );
                    break;
                }
            }
        }

        Ok(())
    }

    /// Check for unimplemented!() and todo!() macros.
    async fn check_unimplemented_code(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "unimplemented_code";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Error);
        let files = collect_rust_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if let Err(e) = self
                .scan_file_for_unimplemented(&file_path, rule_id, severity, findings)
                .await
            {
                tracing::warn!(
                    "Failed to scan file {} for unimplemented: {}",
                    file_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Scan a single file for unimplemented!() and todo!() macros.
    async fn scan_file_for_unimplemented(
        &self,
        file_path: &Path,
        rule_id: &str,
        severity: Severity,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let file = fs::File::open(file_path).await?;
        let reader = tokio::io::BufReader::new(file);
        let mut lines = reader.lines();
        let mut line_num: u32 = 0;

        while let Some(line) = lines.next_line().await? {
            line_num += 1;
            let trimmed = line.trim();

            // Skip lines that are entirely comments
            if trimmed.starts_with("//") {
                continue;
            }

            // Check for unimplemented!()
            if line.contains("unimplemented!") {
                findings.push(
                    Finding::new(
                        rule_id,
                        severity,
                        "Code marked as unimplemented - will panic at runtime",
                    )
                    .at_file(file_path)
                    .at_line(line_num)
                    .with_snippet(truncate_line(&line, 80))
                    .with_suggestion("Implement the required functionality or remove the code"),
                );
            }

            // Check for todo!()
            if line.contains("todo!") && !line.contains("// todo!") && !line.contains("//todo!") {
                findings.push(
                    Finding::new(
                        rule_id,
                        severity,
                        "Code marked with todo!() macro - will panic at runtime",
                    )
                    .at_file(file_path)
                    .at_line(line_num)
                    .with_snippet(truncate_line(&line, 80))
                    .with_suggestion("Implement the required functionality"),
                );
            }

            // Check for panic!("not implemented")
            let lower = line.to_lowercase();
            if lower.contains("panic!") && lower.contains("not implemented") {
                findings.push(
                    Finding::new(
                        rule_id,
                        severity,
                        "Panic with 'not implemented' message found",
                    )
                    .at_file(file_path)
                    .at_line(line_num)
                    .with_snippet(truncate_line(&line, 80))
                    .with_suggestion("Implement the required functionality"),
                );
            }
        }

        Ok(())
    }

    /// Check for error handling issues (unwrap without context).
    async fn check_error_handling(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "error_handling";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Warning);
        let files = collect_rust_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if let Err(e) = self
                .scan_file_for_error_handling(&file_path, rule_id, severity, findings)
                .await
            {
                tracing::warn!(
                    "Failed to scan file {} for error handling: {}",
                    file_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Scan a single file for error handling issues.
    async fn scan_file_for_error_handling(
        &self,
        file_path: &Path,
        rule_id: &str,
        severity: Severity,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let file = fs::File::open(file_path).await?;
        let reader = tokio::io::BufReader::new(file);
        let mut lines = reader.lines();
        let mut line_num: u32 = 0;

        while let Some(line) = lines.next_line().await? {
            line_num += 1;
            let trimmed = line.trim();

            // Skip test modules
            if trimmed.contains("#[cfg(test)]") || trimmed.contains("#[test]") {
                // Test code can use unwrap()
                continue;
            }

            // Check for bare .unwrap() calls (not followed by expect-like context)
            if line.contains(".unwrap()") {
                // Check if it's in a test file using robust heuristics
                let is_test_file = is_test_path(file_path);

                if !is_test_file {
                    // Check if the next line or same line has a comment explaining it
                    let has_context = line.contains("// ")
                        && (line.contains("safe")
                            || line.contains("always")
                            || line.contains("guaranteed"));

                    if !has_context {
                        findings.push(
                            Finding::new(
                                rule_id,
                                severity,
                                "Use of unwrap() without context - prefer expect() with explanation",
                            )
                            .at_file(file_path)
                            .at_line(line_num)
                            .with_snippet(truncate_line(&line, 80))
                            .with_suggestion(
                                "Replace .unwrap() with .expect(\"reason\") to provide context on panic",
                            ),
                        );
                    }
                }
            }

            // Check for ignored Results: let _ = result
            if trimmed.starts_with("let _") && line.contains('=') && !line.contains("let __") {
                // Heuristic: if the right side looks like it could return a Result
                let rhs = line.split('=').nth(1).unwrap_or("");
                if rhs.contains("(")
                    && !rhs.contains("iter")
                    && !rhs.contains("into_iter")
                    && !rhs.contains("drain")
                {
                    findings.push(
                        Finding::new(
                            rule_id,
                            Severity::Info,
                            "Potentially ignored Result - consider handling the error",
                        )
                        .at_file(file_path)
                        .at_line(line_num)
                        .with_snippet(truncate_line(&line, 80))
                        .with_suggestion(
                            "Handle the Result explicitly or use `_ = expr;` if intentional",
                        ),
                    );
                }
            }
        }

        Ok(())
    }

    /// Check for dead code markers.
    async fn check_dead_code(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "dead_code";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Info);
        let files = collect_rust_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if let Err(e) = self
                .scan_file_for_dead_code(&file_path, rule_id, severity, findings)
                .await
            {
                tracing::warn!(
                    "Failed to scan file {} for dead code: {}",
                    file_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Scan a single file for dead code markers.
    async fn scan_file_for_dead_code(
        &self,
        file_path: &Path,
        rule_id: &str,
        severity: Severity,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let content = fs::read_to_string(file_path).await?;
        let lines: Vec<&str> = content.lines().collect();

        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as u32;
            let trimmed = line.trim();

            // Check for #[allow(dead_code)]
            if trimmed == "#[allow(dead_code)]" {
                // Find what it's applied to
                let next_line = lines.get(idx + 1).map(|s| s.trim()).unwrap_or("");
                let context = if next_line.starts_with("fn ") {
                    "function"
                } else if next_line.starts_with("struct ") {
                    "struct"
                } else if next_line.starts_with("enum ") {
                    "enum"
                } else if next_line.starts_with("const ") {
                    "constant"
                } else if next_line.starts_with("static ") {
                    "static"
                } else {
                    "item"
                };

                findings.push(
                    Finding::new(
                        rule_id,
                        severity,
                        format!("Dead code marker found - {} may be unused", context),
                    )
                    .at_file(file_path)
                    .at_line(line_num)
                    .with_snippet(format!("{}\n{}", trimmed, next_line))
                    .with_suggestion("Remove unused code or document why it's needed"),
                );
            }

            // Also flag #[allow(unused)] patterns
            if trimmed.starts_with("#[allow(unused") {
                findings.push(
                    Finding::new(rule_id, severity, "Unused code suppression found")
                        .at_file(file_path)
                        .at_line(line_num)
                        .with_snippet(truncate_line(trimmed, 80))
                        .with_suggestion(
                            "Remove unused code or document why suppression is needed",
                        ),
                );
            }
        }

        Ok(())
    }

    /// Check for missing documentation on public items.
    async fn check_documentation(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "documentation";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Info);
        let files = collect_rust_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if let Err(e) = self
                .scan_file_for_docs(&file_path, rule_id, severity, findings)
                .await
            {
                tracing::warn!(
                    "Failed to scan file {} for docs: {}",
                    file_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Scan a single file for missing documentation.
    async fn scan_file_for_docs(
        &self,
        file_path: &Path,
        rule_id: &str,
        severity: Severity,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let content = fs::read_to_string(file_path).await?;
        let lines: Vec<&str> = content.lines().collect();

        for (idx, line) in lines.iter().enumerate() {
            let line_num = (idx + 1) as u32;
            let trimmed = line.trim();

            // Check for public items
            let is_pub_item = trimmed.starts_with("pub fn ")
                || trimmed.starts_with("pub struct ")
                || trimmed.starts_with("pub enum ")
                || trimmed.starts_with("pub trait ")
                || trimmed.starts_with("pub type ")
                || trimmed.starts_with("pub const ")
                || trimmed.starts_with("pub mod ")
                || trimmed.starts_with("pub static ");

            if is_pub_item {
                // Check if preceded by doc comment
                let has_doc = idx > 0
                    && lines[..idx].iter().rev().take(5).any(|prev_line| {
                        let prev_trimmed = prev_line.trim();
                        prev_trimmed.starts_with("///")
                            || prev_trimmed.starts_with("//!")
                            || prev_trimmed.starts_with("#[doc")
                    });

                if !has_doc {
                    // Extract the item name
                    let item_name = extract_item_name(trimmed);
                    findings.push(
                        Finding::new(
                            rule_id,
                            severity,
                            format!("Missing documentation for public item: {}", item_name),
                        )
                        .at_file(file_path)
                        .at_line(line_num)
                        .with_snippet(truncate_line(trimmed, 80))
                        .with_suggestion("Add /// documentation comment above this item"),
                    );
                }
            }
        }

        Ok(())
    }
}

impl Default for QualityAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ValidationAgent for QualityAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn dependencies(&self) -> Vec<String> {
        // Quality agent has no dependencies
        Vec::new()
    }

    fn rules(&self) -> Vec<RuleInfo> {
        vec![
            RuleInfo::new(
                "todo_comments",
                "TODO Comments",
                "Find TODO, FIXME, HACK, and XXX comments that indicate incomplete work",
            )
            .with_severity(Severity::Warning),
            RuleInfo::new(
                "unimplemented_code",
                "Unimplemented Code",
                "Detect unimplemented!() and todo!() macros that will panic at runtime",
            )
            .with_severity(Severity::Error),
            RuleInfo::new(
                "error_handling",
                "Error Handling",
                "Check for unwrap() without context and ignored Results",
            )
            .with_severity(Severity::Warning),
            RuleInfo::new(
                "dead_code",
                "Dead Code",
                "Find code marked with #[allow(dead_code)] that may be unused",
            )
            .with_severity(Severity::Info),
            RuleInfo::new(
                "documentation",
                "Documentation",
                "Check for missing documentation on public items",
            )
            .with_severity(Severity::Info)
            .enabled_by_default(false), // Often too noisy
        ]
    }

    async fn validate(&self, ctx: &ValidationContext) -> Result<ValidationResult, AgentError> {
        let start = Instant::now();
        let mut result = ValidationResult::new(&self.id);
        let mut findings = Vec::new();

        // Run all checks
        self.check_todo_comments(ctx, &mut findings).await?;
        self.check_unimplemented_code(ctx, &mut findings).await?;
        self.check_error_handling(ctx, &mut findings).await?;
        self.check_dead_code(ctx, &mut findings).await?;
        self.check_documentation(ctx, &mut findings).await?;

        // Add all findings to result
        for finding in findings {
            result.add_finding(finding);
        }

        // Set duration and summary
        let duration_ms = start.elapsed().as_millis() as u64;
        let counts = result.count_by_severity();

        let summary = format!(
            "Quality scan complete: {} errors, {} warnings, {} info",
            counts.get(&Severity::Error).unwrap_or(&0),
            counts.get(&Severity::Warning).unwrap_or(&0),
            counts.get(&Severity::Info).unwrap_or(&0)
        );

        Ok(result.with_duration(duration_ms).with_summary(summary))
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Extract item name from a Rust declaration.
fn extract_item_name(line: &str) -> String {
    // Simple extraction - works for basic cases
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() >= 3 {
        // pub fn name, pub struct Name, etc.
        let name = parts[2];
        // Remove generic params and parentheses
        name.split(['<', '(', '{'])
            .next()
            .unwrap_or(name)
            .to_string()
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_item_name() {
        assert_eq!(extract_item_name("pub fn foo()"), "foo");
        assert_eq!(extract_item_name("pub struct Foo<T>"), "Foo");
        assert_eq!(extract_item_name("pub enum Bar {"), "Bar");
    }

    #[tokio::test]
    async fn test_quality_agent_rules() {
        let agent = QualityAgent::new();
        let rules = agent.rules();
        assert_eq!(rules.len(), 5);

        let rule_ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(rule_ids.contains(&"todo_comments"));
        assert!(rule_ids.contains(&"unimplemented_code"));
        assert!(rule_ids.contains(&"error_handling"));
        assert!(rule_ids.contains(&"dead_code"));
        assert!(rule_ids.contains(&"documentation"));
    }

    #[tokio::test]
    async fn test_quality_agent_id() {
        let agent = QualityAgent::new();
        assert_eq!(agent.id(), "quality");
        assert_eq!(agent.name(), "Code Quality Agent");
        assert!(agent.dependencies().is_empty());
    }
}
