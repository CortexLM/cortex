//! Result aggregator agent for Forge system.
//!
//! Collects and aggregates results from all validation agents,
//! producing a comprehensive validation report with:
//! - Overall status determination
//! - Findings grouped by severity
//! - Final recommendation (proceed/block)

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Instant;

use super::{
    AgentError, Finding, RuleInfo, Severity, ValidationAgent, ValidationContext, ValidationResult,
    ValidationStatus,
};

// ============================================================================
// ForgeRecommendation
// ============================================================================

/// Final recommendation from the aggregator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgeRecommendation {
    /// All checks passed, safe to proceed.
    Proceed,
    /// Passed with warnings, may proceed with caution.
    ProceedWithCaution,
    /// Critical or error findings, should not proceed.
    Block,
    /// Validation was skipped or incomplete.
    Inconclusive,
}

impl ForgeRecommendation {
    /// Returns display name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Proceed => "Proceed",
            Self::ProceedWithCaution => "Proceed with Caution",
            Self::Block => "Block",
            Self::Inconclusive => "Inconclusive",
        }
    }

    /// Returns emoji/icon.
    pub fn icon(&self) -> &'static str {
        match self {
            Self::Proceed => "✅",
            Self::ProceedWithCaution => "⚠️",
            Self::Block => "🛑",
            Self::Inconclusive => "❓",
        }
    }
}

// ============================================================================
// ForgeResponse
// ============================================================================

/// Comprehensive response from the Forge validation system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeResponse {
    /// Overall recommendation.
    pub recommendation: ForgeRecommendation,
    /// Overall validation status.
    pub status: ValidationStatus,
    /// Individual agent results.
    pub agent_results: HashMap<String, AgentSummary>,
    /// All findings grouped by severity.
    pub findings_by_severity: HashMap<Severity, Vec<Finding>>,
    /// Total finding counts by severity.
    pub finding_counts: HashMap<Severity, usize>,
    /// Total duration in milliseconds.
    pub total_duration_ms: u64,
    /// Human-readable summary.
    pub summary: String,
    /// Detailed report (Markdown formatted).
    pub detailed_report: String,
}

impl ForgeResponse {
    /// Create a new response.
    pub fn new() -> Self {
        Self {
            recommendation: ForgeRecommendation::Inconclusive,
            status: ValidationStatus::Skipped,
            agent_results: HashMap::new(),
            findings_by_severity: HashMap::new(),
            finding_counts: HashMap::new(),
            total_duration_ms: 0,
            summary: String::new(),
            detailed_report: String::new(),
        }
    }

    /// Check if validation passed (can proceed).
    pub fn can_proceed(&self) -> bool {
        matches!(
            self.recommendation,
            ForgeRecommendation::Proceed | ForgeRecommendation::ProceedWithCaution
        )
    }

    /// Get critical findings count.
    pub fn critical_count(&self) -> usize {
        self.finding_counts
            .get(&Severity::Critical)
            .copied()
            .unwrap_or(0)
    }

    /// Get error findings count.
    pub fn error_count(&self) -> usize {
        self.finding_counts
            .get(&Severity::Error)
            .copied()
            .unwrap_or(0)
    }

    /// Get warning findings count.
    pub fn warning_count(&self) -> usize {
        self.finding_counts
            .get(&Severity::Warning)
            .copied()
            .unwrap_or(0)
    }

    /// Get info findings count.
    pub fn info_count(&self) -> usize {
        self.finding_counts
            .get(&Severity::Info)
            .copied()
            .unwrap_or(0)
    }
}

impl Default for ForgeResponse {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary of a single agent's results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSummary {
    /// Agent ID.
    pub agent_id: String,
    /// Agent name.
    pub agent_name: String,
    /// Validation status.
    pub status: ValidationStatus,
    /// Finding counts by severity.
    pub finding_counts: HashMap<Severity, usize>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Summary message.
    pub summary: String,
}

// ============================================================================
// AggregatorAgent
// ============================================================================

/// Aggregator agent that collects results from all validation agents.
///
/// This agent depends on all other validation agents and produces
/// a comprehensive ForgeResponse with:
/// - Overall status (Pass only if ALL agents passed)
/// - Summary of all findings grouped by severity
/// - Final recommendation (proceed/block)
#[derive(Debug, Clone)]
pub struct AggregatorAgent {
    /// Agent identifier.
    id: String,
    /// Agent name.
    name: String,
    /// IDs of agents to aggregate.
    agent_ids: Vec<String>,
}

impl AggregatorAgent {
    /// Create a new aggregator agent with no default dependencies.
    ///
    /// Use `with_agents()` or `add_agent()` to configure which agents
    /// this aggregator depends on. The aggregator will collect results
    /// from all configured agents.
    pub fn new() -> Self {
        Self {
            id: "aggregator".to_string(),
            name: "Result Aggregator".to_string(),
            agent_ids: Vec::new(),
        }
    }

    /// Create an aggregator with custom agent dependencies.
    ///
    /// # Arguments
    ///
    /// * `agent_ids` - IDs of agents whose results should be aggregated
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let aggregator = AggregatorAgent::with_agents(vec![
    ///     "security".to_string(),
    ///     "quality".to_string(),
    ///     "custom-lint".to_string(),
    /// ]);
    /// ```
    pub fn with_agents(agent_ids: Vec<String>) -> Self {
        Self {
            id: "aggregator".to_string(),
            name: "Result Aggregator".to_string(),
            agent_ids,
        }
    }

    /// Add an agent dependency to this aggregator.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - ID of the agent to depend on
    pub fn add_agent(&mut self, agent_id: impl Into<String>) {
        self.agent_ids.push(agent_id.into());
    }

    /// Set the list of agent dependencies.
    ///
    /// # Arguments
    ///
    /// * `agent_ids` - IDs of agents whose results should be aggregated
    pub fn set_agents(&mut self, agent_ids: Vec<String>) {
        self.agent_ids = agent_ids;
    }

    /// Aggregate results from all previous agents.
    fn aggregate_results(&self, ctx: &ValidationContext) -> Result<ForgeResponse, AgentError> {
        let mut response = ForgeResponse::new();
        let mut all_findings: Vec<Finding> = Vec::new();
        let mut worst_status = ValidationStatus::Passed;
        let mut total_duration: u64 = 0;
        let mut missing_agents = Vec::new();

        // Collect results from each agent
        for agent_id in &self.agent_ids {
            if let Some(result) = ctx.get_previous_result(agent_id) {
                // Track worst status
                worst_status = combine_status(worst_status, result.status);

                // Collect findings
                all_findings.extend(result.findings.clone());

                // Add agent summary
                response.agent_results.insert(
                    agent_id.clone(),
                    AgentSummary {
                        agent_id: result.agent_id.clone(),
                        agent_name: agent_id.clone(), // We don't have name in result
                        status: result.status,
                        finding_counts: result.count_by_severity(),
                        duration_ms: result.duration_ms,
                        summary: result.summary.clone(),
                    },
                );

                total_duration += result.duration_ms;
            } else {
                missing_agents.push(agent_id.clone());
            }
        }

        // Handle missing agents
        if !missing_agents.is_empty() {
            return Err(AgentError::MissingDependency(
                self.id.clone(),
                missing_agents.join(", "),
            ));
        }

        // Group findings by severity
        for finding in &all_findings {
            response
                .findings_by_severity
                .entry(finding.severity)
                .or_default()
                .push(finding.clone());

            *response.finding_counts.entry(finding.severity).or_insert(0) += 1;
        }

        // Determine overall status
        response.status = worst_status;

        // Determine recommendation
        response.recommendation = determine_recommendation(&response.finding_counts, worst_status);

        // Set duration
        response.total_duration_ms = total_duration;

        // Generate summary
        response.summary = generate_summary(&response);

        // Generate detailed report
        response.detailed_report = generate_detailed_report(&response, &all_findings);

        Ok(response)
    }
}

impl Default for AggregatorAgent {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ValidationAgent for AggregatorAgent {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn dependencies(&self) -> Vec<String> {
        self.agent_ids.clone()
    }

    fn rules(&self) -> Vec<RuleInfo> {
        // Aggregator doesn't have its own rules
        Vec::new()
    }

    async fn validate(&self, ctx: &ValidationContext) -> Result<ValidationResult, AgentError> {
        let start = Instant::now();

        // Aggregate all results
        let forge_response = self.aggregate_results(ctx)?;

        // Create validation result
        let mut result = ValidationResult::new(&self.id);
        result.status = forge_response.status;
        result.summary = forge_response.summary.clone();

        // Add a meta-finding with the ForgeResponse as JSON
        let response_json =
            serde_json::to_string_pretty(&forge_response).map_err(AgentError::Serialization)?;

        // Store the full response in the summary for downstream consumers
        result.summary = format!(
            "{}\n\n---\nForge Response:\n```json\n{}\n```",
            forge_response.summary, response_json
        );

        // Set duration
        let duration_ms = start.elapsed().as_millis() as u64;
        Ok(result.with_duration(duration_ms))
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Combine two validation statuses, returning the worst one.
fn combine_status(a: ValidationStatus, b: ValidationStatus) -> ValidationStatus {
    match (a, b) {
        (ValidationStatus::Failed, _) | (_, ValidationStatus::Failed) => ValidationStatus::Failed,
        (ValidationStatus::Skipped, _) | (_, ValidationStatus::Skipped) => {
            ValidationStatus::Skipped
        }
        (ValidationStatus::PassedWithWarnings, _) | (_, ValidationStatus::PassedWithWarnings) => {
            ValidationStatus::PassedWithWarnings
        }
        _ => ValidationStatus::Passed,
    }
}

/// Determine the recommendation based on finding counts and status.
fn determine_recommendation(
    counts: &HashMap<Severity, usize>,
    status: ValidationStatus,
) -> ForgeRecommendation {
    let critical = counts.get(&Severity::Critical).copied().unwrap_or(0);
    let errors = counts.get(&Severity::Error).copied().unwrap_or(0);
    let warnings = counts.get(&Severity::Warning).copied().unwrap_or(0);

    if status == ValidationStatus::Skipped {
        return ForgeRecommendation::Inconclusive;
    }

    if critical > 0 {
        return ForgeRecommendation::Block;
    }

    if errors > 0 {
        return ForgeRecommendation::Block;
    }

    if warnings > 0 {
        return ForgeRecommendation::ProceedWithCaution;
    }

    ForgeRecommendation::Proceed
}

/// Generate a human-readable summary.
fn generate_summary(response: &ForgeResponse) -> String {
    let icon = response.recommendation.icon();
    let rec_name = response.recommendation.name();

    let critical = response.critical_count();
    let errors = response.error_count();
    let warnings = response.warning_count();
    let info = response.info_count();

    format!(
        "{} {} - {} critical, {} errors, {} warnings, {} info findings across {} agents",
        icon,
        rec_name,
        critical,
        errors,
        warnings,
        info,
        response.agent_results.len()
    )
}

/// Generate a detailed Markdown report.
fn generate_detailed_report(response: &ForgeResponse, all_findings: &[Finding]) -> String {
    let mut report = String::new();

    // Header
    report.push_str("# Forge Validation Report\n\n");

    // Recommendation banner
    report.push_str(&format!(
        "## {} {}\n\n",
        response.recommendation.icon(),
        response.recommendation.name()
    ));

    // Summary stats
    report.push_str("### Summary\n\n");
    report.push_str(&format!(
        "| Metric | Value |\n|--------|-------|\n| Status | {:?} |\n| Duration | {}ms |\n| Agents | {} |\n",
        response.status,
        response.total_duration_ms,
        response.agent_results.len()
    ));

    // Finding counts table
    report.push_str("\n### Findings by Severity\n\n");
    report.push_str("| Severity | Count |\n|----------|-------|\n");
    for severity in [
        Severity::Critical,
        Severity::Error,
        Severity::Warning,
        Severity::Info,
    ] {
        let count = response.finding_counts.get(&severity).copied().unwrap_or(0);
        if count > 0 {
            report.push_str(&format!(
                "| {} {} | {} |\n",
                severity.icon(),
                severity.name(),
                count
            ));
        }
    }

    // Agent results
    report.push_str("\n### Agent Results\n\n");
    for (agent_id, summary) in &response.agent_results {
        report.push_str(&format!(
            "#### {}\n- Status: {:?}\n- Duration: {}ms\n- {}\n\n",
            agent_id, summary.status, summary.duration_ms, summary.summary
        ));
    }

    // Detailed findings (grouped by severity, limited output)
    if !all_findings.is_empty() {
        report.push_str("### Detailed Findings\n\n");

        for severity in [
            Severity::Critical,
            Severity::Error,
            Severity::Warning,
            Severity::Info,
        ] {
            if let Some(findings) = response.findings_by_severity.get(&severity) {
                if !findings.is_empty() {
                    report.push_str(&format!(
                        "#### {} {} Findings\n\n",
                        severity.icon(),
                        severity.name()
                    ));

                    // Limit output to first 10 findings per severity
                    let display_count = findings.len().min(10);
                    for finding in findings.iter().take(display_count) {
                        report.push_str(&format!(
                            "- **[{}]** {}\n",
                            finding.rule_id, finding.message
                        ));
                        if let Some(ref file) = finding.file {
                            report.push_str(&format!("  - File: `{}`", file.display()));
                            if let Some(line) = finding.line {
                                report.push_str(&format!(":{}", line));
                            }
                            report.push('\n');
                        }
                        if let Some(ref suggestion) = finding.suggestion {
                            report.push_str(&format!("  - 💡 {}\n", suggestion));
                        }
                    }

                    if findings.len() > display_count {
                        report.push_str(&format!(
                            "\n*...and {} more {} findings*\n",
                            findings.len() - display_count,
                            severity.name().to_lowercase()
                        ));
                    }
                    report.push('\n');
                }
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_forge_recommendation() {
        assert_eq!(ForgeRecommendation::Proceed.name(), "Proceed");
        assert_eq!(ForgeRecommendation::Block.icon(), "🛑");
    }

    #[test]
    fn test_forge_response_counts() {
        let mut response = ForgeResponse::new();
        response.finding_counts.insert(Severity::Critical, 2);
        response.finding_counts.insert(Severity::Warning, 5);

        assert_eq!(response.critical_count(), 2);
        assert_eq!(response.error_count(), 0);
        assert_eq!(response.warning_count(), 5);
    }

    #[test]
    fn test_combine_status() {
        assert_eq!(
            combine_status(ValidationStatus::Passed, ValidationStatus::Passed),
            ValidationStatus::Passed
        );
        assert_eq!(
            combine_status(ValidationStatus::Passed, ValidationStatus::Failed),
            ValidationStatus::Failed
        );
        assert_eq!(
            combine_status(
                ValidationStatus::PassedWithWarnings,
                ValidationStatus::Passed
            ),
            ValidationStatus::PassedWithWarnings
        );
    }

    #[test]
    fn test_determine_recommendation() {
        let mut counts = HashMap::new();
        assert_eq!(
            determine_recommendation(&counts, ValidationStatus::Passed),
            ForgeRecommendation::Proceed
        );

        counts.insert(Severity::Warning, 1);
        assert_eq!(
            determine_recommendation(&counts, ValidationStatus::PassedWithWarnings),
            ForgeRecommendation::ProceedWithCaution
        );

        counts.insert(Severity::Error, 1);
        assert_eq!(
            determine_recommendation(&counts, ValidationStatus::Failed),
            ForgeRecommendation::Block
        );

        counts.insert(Severity::Critical, 1);
        assert_eq!(
            determine_recommendation(&counts, ValidationStatus::Failed),
            ForgeRecommendation::Block
        );
    }

    #[tokio::test]
    async fn test_aggregator_agent_dependencies() {
        // Default aggregator has no dependencies
        let agent = AggregatorAgent::new();
        assert_eq!(agent.id(), "aggregator");
        assert_eq!(agent.name(), "Result Aggregator");
        assert!(agent.dependencies().is_empty());

        // with_agents configures dependencies
        let agent_with_deps =
            AggregatorAgent::with_agents(vec!["security".to_string(), "quality".to_string()]);
        let deps = agent_with_deps.dependencies();
        assert!(deps.contains(&"security".to_string()));
        assert!(deps.contains(&"quality".to_string()));
    }

    #[test]
    fn test_aggregator_agent_add_agent() {
        let mut agent = AggregatorAgent::new();
        assert!(agent.dependencies().is_empty());

        agent.add_agent("security");
        agent.add_agent("quality");

        let deps = agent.dependencies();
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&"security".to_string()));
        assert!(deps.contains(&"quality".to_string()));
    }

    #[test]
    fn test_generate_summary() {
        let mut response = ForgeResponse::new();
        response.recommendation = ForgeRecommendation::ProceedWithCaution;
        response.finding_counts.insert(Severity::Warning, 3);
        response.agent_results.insert(
            "test".to_string(),
            AgentSummary {
                agent_id: "test".to_string(),
                agent_name: "Test".to_string(),
                status: ValidationStatus::PassedWithWarnings,
                finding_counts: HashMap::new(),
                duration_ms: 100,
                summary: "Test summary".to_string(),
            },
        );

        let summary = generate_summary(&response);
        assert!(summary.contains("Proceed with Caution"));
        assert!(summary.contains("3 warnings"));
    }
}
