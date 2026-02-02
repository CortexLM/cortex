//! Security validation agent for Forge system.
//!
//! Performs security-focused code analysis including:
//! - Detection of hardcoded secrets and API keys
//! - Dependency vulnerability auditing
//! - Unsafe code block analysis
//! - Input validation pattern checking

use async_trait::async_trait;
use std::path::Path;
use std::time::Instant;
use tokio::fs;
use tokio::io::AsyncBufReadExt;

use super::{
    utils::{collect_source_files, truncate_line},
    AgentError, Finding, RuleInfo, Severity, ValidationAgent, ValidationContext, ValidationResult,
};

// ============================================================================
// Constants
// ============================================================================

/// Patterns that indicate potential hardcoded secrets.
const SECRET_PATTERNS: &[(&str, &str)] = &[
    ("api_key", "API key"),
    ("apikey", "API key"),
    ("api-key", "API key"),
    ("secret_key", "secret key"),
    ("secretkey", "secret key"),
    ("secret-key", "secret key"),
    ("password", "password"),
    ("passwd", "password"),
    ("private_key", "private key"),
    ("privatekey", "private key"),
    ("aws_access_key", "AWS access key"),
    ("aws_secret", "AWS secret"),
    ("github_token", "GitHub token"),
    ("gh_token", "GitHub token"),
    ("slack_token", "Slack token"),
    ("stripe_key", "Stripe key"),
    ("database_url", "database URL"),
    ("db_password", "database password"),
    ("jwt_secret", "JWT secret"),
    ("bearer", "bearer token"),
    ("authorization", "authorization header"),
];

/// Patterns for base64-encoded or hex-encoded potential secrets.
const ENCODED_SECRET_PATTERNS: &[&str] = &[
    // Common API key prefixes
    "sk_live_", "sk_test_", "pk_live_", "pk_test_", "xox",  // Slack tokens
    "ghp_", // GitHub personal access token
    "gho_", // GitHub OAuth token
    "ghu_", // GitHub user-to-server token
    "ghs_", // GitHub server-to-server token
    "ghr_", // GitHub refresh token
];

/// Known vulnerable crate patterns (simplified - in production would use advisory database).
const VULNERABLE_CRATES: &[(&str, &str, &str)] = &[
    ("chrono", "<0.4.20", "RUSTSEC-2020-0159: Potential segfault"),
    (
        "smallvec",
        "<0.6.14",
        "RUSTSEC-2019-0009: Double-free vulnerability",
    ),
    ("regex", "<1.5.5", "RUSTSEC-2022-0013: Denial of service"),
];

// ============================================================================
// SecurityAgent
// ============================================================================

/// Security-focused validation agent.
///
/// Checks for:
/// - `secrets_exposed`: Hardcoded secrets and API keys
/// - `dependencies_audit`: Known vulnerable dependencies
/// - `unsafe_code`: Unsafe blocks without justification
/// - `input_validation`: Basic input validation patterns
#[derive(Debug, Clone)]
pub struct SecurityAgent {
    /// Agent identifier.
    id: String,
    /// Agent name.
    name: String,
}

impl SecurityAgent {
    /// Create a new security agent with default settings.
    pub fn new() -> Self {
        Self {
            id: "security".to_string(),
            name: "Security Agent".to_string(),
        }
    }

    /// Check for exposed secrets in source files.
    async fn check_secrets_exposed(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "secrets_exposed";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Critical);
        let files = collect_source_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if let Err(e) = self
                .scan_file_for_secrets(&file_path, rule_id, severity, findings)
                .await
            {
                tracing::warn!(
                    "Failed to scan file {} for secrets: {}",
                    file_path.display(),
                    e
                );
            }
        }

        Ok(())
    }

    /// Scan a single file for potential secrets.
    async fn scan_file_for_secrets(
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
            let line_lower = line.to_lowercase();

            // Skip comments that are likely documentation
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("///") || trimmed.starts_with("//!")
            {
                // Still check for actual secret values in comments
                if !contains_suspicious_value(&line) {
                    continue;
                }
            }

            // Check for secret patterns
            for (pattern, description) in SECRET_PATTERNS {
                if line_lower.contains(pattern) {
                    // Check if it looks like an actual assignment with a value
                    if looks_like_hardcoded_secret(&line) {
                        findings.push(
                            Finding::new(
                                rule_id,
                                severity,
                                format!("Potential hardcoded {} detected", description),
                            )
                            .at_file(file_path)
                            .at_line(line_num)
                            .with_snippet(truncate_line(&line, 80))
                            .with_suggestion(
                                "Use environment variables or a secrets manager instead",
                            ),
                        );
                        break; // One finding per line
                    }
                }
            }

            // Check for encoded secret patterns
            for pattern in ENCODED_SECRET_PATTERNS {
                if line.contains(pattern) {
                    findings.push(
                        Finding::new(
                            rule_id,
                            severity,
                            format!("Potential hardcoded token detected (prefix: {})", pattern),
                        )
                        .at_file(file_path)
                        .at_line(line_num)
                        .with_snippet(truncate_line(&line, 80))
                        .with_suggestion("Use environment variables or a secrets manager"),
                    );
                    break;
                }
            }
        }

        Ok(())
    }

    /// Check for vulnerable dependencies in Cargo.lock.
    async fn check_dependencies_audit(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "dependencies_audit";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Warning);
        let cargo_lock = ctx.project_path.join("Cargo.lock");

        if !cargo_lock.exists() {
            // No Cargo.lock, skip check
            return Ok(());
        }

        let content = fs::read_to_string(&cargo_lock).await?;
        let parsed_deps = parse_cargo_lock(&content);

        for (name, version) in parsed_deps {
            for (vuln_name, vuln_version, advisory) in VULNERABLE_CRATES {
                if name == *vuln_name && version_matches_vulnerable(&version, vuln_version) {
                    findings.push(
                        Finding::new(
                            rule_id,
                            severity,
                            format!("Vulnerable dependency: {} {} ({})", name, version, advisory),
                        )
                        .at_file(&cargo_lock)
                        .with_suggestion(format!("Update {} to a patched version", name)),
                    );
                }
            }
        }

        Ok(())
    }

    /// Check for unsafe code blocks without safety documentation.
    async fn check_unsafe_code(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "unsafe_code";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Warning);
        let files = collect_source_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if file_path.extension().is_some_and(|ext| ext == "rs") {
                if let Err(e) = self
                    .scan_file_for_unsafe(&file_path, rule_id, severity, findings)
                    .await
                {
                    tracing::warn!(
                        "Failed to scan file {} for unsafe: {}",
                        file_path.display(),
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Scan a single file for undocumented unsafe blocks.
    async fn scan_file_for_unsafe(
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

            // Check for unsafe block or function
            if trimmed.contains("unsafe ") || trimmed.starts_with("unsafe{") {
                // Look for SAFETY comment in preceding lines
                let has_safety_comment = (idx > 0 && idx <= lines.len())
                    && lines[..idx].iter().rev().take(5).any(|prev_line| {
                        let prev_trimmed = prev_line.trim();
                        prev_trimmed.contains("// SAFETY:")
                            || prev_trimmed.contains("// Safety:")
                            || prev_trimmed.contains("/// # Safety")
                            || prev_trimmed.contains("/// # SAFETY")
                    });

                if !has_safety_comment {
                    findings.push(
                        Finding::new(
                            rule_id,
                            severity,
                            "Unsafe code block without safety documentation",
                        )
                        .at_file(file_path)
                        .at_line(line_num)
                        .with_snippet(truncate_line(line, 80))
                        .with_suggestion(
                            "Add a `// SAFETY:` comment explaining why this unsafe code is sound",
                        ),
                    );
                }
            }
        }

        Ok(())
    }

    /// Check for basic input validation patterns.
    async fn check_input_validation(
        &self,
        ctx: &ValidationContext,
        findings: &mut Vec<Finding>,
    ) -> Result<(), AgentError> {
        let rule_id = "input_validation";
        if !ctx.config.is_rule_enabled(rule_id, true) {
            return Ok(());
        }

        let severity = ctx.config.get_severity(rule_id, Severity::Info);
        let files = collect_source_files(&ctx.project_path, ctx).await?;

        for file_path in files {
            if file_path.extension().is_some_and(|ext| ext == "rs") {
                if let Err(e) = self
                    .scan_file_for_input_issues(&file_path, rule_id, severity, findings)
                    .await
                {
                    tracing::warn!(
                        "Failed to scan file {} for input issues: {}",
                        file_path.display(),
                        e
                    );
                }
            }
        }

        Ok(())
    }

    /// Scan for common input validation issues.
    async fn scan_file_for_input_issues(
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

            // Check for direct format! usage with user input (potential injection)
            if line.contains("format!") && line.contains("user") && !line.contains("{:?}") {
                findings.push(
                    Finding::new(
                        rule_id,
                        severity,
                        "Potential format string injection with user input",
                    )
                    .at_file(file_path)
                    .at_line(line_num)
                    .with_snippet(truncate_line(&line, 80))
                    .with_suggestion("Use {:?} for user-provided strings to prevent injection"),
                );
            }

            // Check for SQL-like string concatenation
            if (line.to_lowercase().contains("select ")
                || line.to_lowercase().contains("insert ")
                || line.to_lowercase().contains("update ")
                || line.to_lowercase().contains("delete "))
                && line.contains("format!")
            {
                findings.push(
                    Finding::new(
                        rule_id,
                        Severity::Error,
                        "Potential SQL injection vulnerability",
                    )
                    .at_file(file_path)
                    .at_line(line_num)
                    .with_snippet(truncate_line(&line, 80))
                    .with_suggestion("Use parameterized queries instead of string formatting"),
                );
            }
        }

        Ok(())
    }
}

impl Default for SecurityAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ValidationAgent for SecurityAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn dependencies(&self) -> Vec<String> {
        // Security agent has no dependencies
        Vec::new()
    }

    fn rules(&self) -> Vec<RuleInfo> {
        vec![
            RuleInfo::new(
                "secrets_exposed",
                "Secrets Exposed",
                "Check for hardcoded secrets, API keys, and credentials in source code",
            )
            .with_severity(Severity::Critical),
            RuleInfo::new(
                "dependencies_audit",
                "Dependencies Audit",
                "Check for known vulnerable dependencies in Cargo.lock",
            )
            .with_severity(Severity::Warning),
            RuleInfo::new(
                "unsafe_code",
                "Unsafe Code",
                "Detect unsafe code blocks without proper safety documentation",
            )
            .with_severity(Severity::Warning),
            RuleInfo::new(
                "input_validation",
                "Input Validation",
                "Check for basic input validation patterns and potential injection points",
            )
            .with_severity(Severity::Info),
        ]
    }

    async fn validate(&self, ctx: &ValidationContext) -> Result<ValidationResult, AgentError> {
        let start = Instant::now();
        let mut result = ValidationResult::new(&self.id);
        let mut findings = Vec::new();

        // Run all checks
        self.check_secrets_exposed(ctx, &mut findings).await?;
        self.check_dependencies_audit(ctx, &mut findings).await?;
        self.check_unsafe_code(ctx, &mut findings).await?;
        self.check_input_validation(ctx, &mut findings).await?;

        // Add all findings to result
        for finding in findings {
            result.add_finding(finding);
        }

        // Set duration and summary
        let duration_ms = start.elapsed().as_millis() as u64;
        let counts = result.count_by_severity();

        let summary = format!(
            "Security scan complete: {} critical, {} errors, {} warnings, {} info",
            counts.get(&Severity::Critical).unwrap_or(&0),
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

/// Check if a line looks like it contains a hardcoded secret value.
fn looks_like_hardcoded_secret(line: &str) -> bool {
    // Look for assignment patterns with string literals
    let has_assignment = line.contains('=') || line.contains(':');
    let has_string = line.contains('"') || line.contains('\'');

    if !has_assignment || !has_string {
        return false;
    }

    // Check for patterns that suggest actual values
    let lower = line.to_lowercase();

    // Environment variable references are OK
    if lower.contains("env!") || lower.contains("env::var") || lower.contains("std::env") {
        return false;
    }

    // Placeholder values are OK
    if lower.contains("xxx")
        || lower.contains("your_")
        || lower.contains("example")
        || lower.contains("<your")
        || lower.contains("${{")
        || lower.contains("${")
    {
        return false;
    }

    // Look for actual secret-like values (long alphanumeric strings)
    if let Some(start_quote) = line.find('"') {
        if let Some(end_quote) = line[start_quote + 1..].find('"') {
            let value = &line[start_quote + 1..start_quote + 1 + end_quote];
            // Long alphanumeric strings are suspicious
            if value.len() > 20
                && value
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return true;
            }
        }
    }

    false
}

/// Check if a line contains suspicious secret-like values.
fn contains_suspicious_value(line: &str) -> bool {
    // Check for encoded secret patterns even in comments
    for pattern in ENCODED_SECRET_PATTERNS {
        if line.contains(pattern) {
            return true;
        }
    }
    false
}

/// Parse Cargo.lock to extract dependency versions.
fn parse_cargo_lock(content: &str) -> Vec<(String, String)> {
    let mut deps = Vec::new();
    let mut current_name: Option<String> = None;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("name = ") {
            current_name = trimmed
                .strip_prefix("name = ")
                .and_then(|s| s.strip_prefix('"'))
                .and_then(|s| s.strip_suffix('"'))
                .map(String::from);
        } else if trimmed.starts_with("version = ") {
            if let Some(name) = current_name.take() {
                if let Some(version) = trimmed
                    .strip_prefix("version = ")
                    .and_then(|s| s.strip_prefix('"'))
                    .and_then(|s| s.strip_suffix('"'))
                {
                    deps.push((name, version.to_string()));
                }
            }
        }
    }

    deps
}

/// Check if a version matches a vulnerable version range.
/// Uses semantic version comparison for accurate results.
fn version_matches_vulnerable(version: &str, vulnerable_pattern: &str) -> bool {
    if let Some(max_version) = vulnerable_pattern.strip_prefix('<') {
        // Parse versions and compare semantically
        compare_semver(version, max_version) == Some(std::cmp::Ordering::Less)
    } else {
        version == vulnerable_pattern
    }
}

/// Compare two semantic versions.
/// Returns Ordering::Less if a < b, Equal if a == b, Greater if a > b.
/// Returns None if parsing fails.
fn compare_semver(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let parse_version = |v: &str| -> Option<Vec<u64>> {
        v.split('.')
            .map(|part| {
                // Handle versions with pre-release suffixes like "1.0.0-alpha"
                let numeric_part = part.split('-').next()?;
                numeric_part.parse::<u64>().ok()
            })
            .collect()
    };

    let a_parts = parse_version(a)?;
    let b_parts = parse_version(b)?;

    // Compare each component
    let max_len = a_parts.len().max(b_parts.len());
    for i in 0..max_len {
        let a_val = a_parts.get(i).copied().unwrap_or(0);
        let b_val = b_parts.get(i).copied().unwrap_or(0);

        match a_val.cmp(&b_val) {
            std::cmp::Ordering::Equal => continue,
            other => return Some(other),
        }
    }

    Some(std::cmp::Ordering::Equal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_looks_like_hardcoded_secret() {
        assert!(!looks_like_hardcoded_secret(
            "let api_key = env!(\"API_KEY\");"
        ));
        assert!(!looks_like_hardcoded_secret("api_key: \"${API_KEY}\""));
        assert!(!looks_like_hardcoded_secret(
            "password: \"your_password_here\""
        ));
    }

    #[test]
    fn test_parse_cargo_lock() {
        let content = r#"
[[package]]
name = "serde"
version = "1.0.123"

[[package]]
name = "tokio"
version = "1.20.0"
"#;
        let deps = parse_cargo_lock(content);
        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0], ("serde".to_string(), "1.0.123".to_string()));
        assert_eq!(deps[1], ("tokio".to_string(), "1.20.0".to_string()));
    }

    #[test]
    fn test_version_matches_vulnerable() {
        // Basic comparison
        assert!(version_matches_vulnerable("0.4.19", "<0.4.20"));
        assert!(!version_matches_vulnerable("0.4.20", "<0.4.20"));
        assert!(!version_matches_vulnerable("0.5.0", "<0.4.20"));

        // Semver edge cases - string comparison would fail these
        assert!(version_matches_vulnerable("0.4.9", "<0.4.20")); // '9' > '2' in string comparison
        assert!(version_matches_vulnerable("0.4.1", "<0.4.20"));
        assert!(!version_matches_vulnerable("0.4.21", "<0.4.20"));

        // Major/minor version comparisons
        assert!(version_matches_vulnerable("0.3.99", "<0.4.20"));
        assert!(!version_matches_vulnerable("1.0.0", "<0.4.20"));

        // Exact match
        assert!(version_matches_vulnerable("1.0.0", "1.0.0"));
        assert!(!version_matches_vulnerable("1.0.1", "1.0.0"));
    }

    #[test]
    fn test_compare_semver() {
        use std::cmp::Ordering;

        assert_eq!(compare_semver("1.0.0", "2.0.0"), Some(Ordering::Less));
        assert_eq!(compare_semver("2.0.0", "1.0.0"), Some(Ordering::Greater));
        assert_eq!(compare_semver("1.0.0", "1.0.0"), Some(Ordering::Equal));

        // Multi-digit version numbers
        assert_eq!(compare_semver("0.4.9", "0.4.20"), Some(Ordering::Less));
        assert_eq!(compare_semver("0.4.19", "0.4.20"), Some(Ordering::Less));
        assert_eq!(compare_semver("0.4.20", "0.4.20"), Some(Ordering::Equal));
        assert_eq!(compare_semver("0.4.21", "0.4.20"), Some(Ordering::Greater));

        // Different number of components
        assert_eq!(compare_semver("1.0", "1.0.0"), Some(Ordering::Equal));
        assert_eq!(compare_semver("1.0.0", "1.0"), Some(Ordering::Equal));
    }

    #[tokio::test]
    async fn test_security_agent_rules() {
        let agent = SecurityAgent::new();
        let rules = agent.rules();
        assert_eq!(rules.len(), 4);

        let rule_ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(rule_ids.contains(&"secrets_exposed"));
        assert!(rule_ids.contains(&"dependencies_audit"));
        assert!(rule_ids.contains(&"unsafe_code"));
        assert!(rule_ids.contains(&"input_validation"));
    }
}
