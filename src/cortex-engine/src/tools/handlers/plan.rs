//! Plan tool handler.
//!
//! Presents comprehensive implementation plans with multi-agent analysis for user approval.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::ToolHandler;
use crate::error::Result;
use crate::tools::context::ToolContext;
use crate::tools::spec::{ToolMetadata, ToolResult};

/// Plan task item with detailed analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanTask {
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_status")]
    pub status: PlanTaskStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtasks: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
}

fn default_status() -> PlanTaskStatus {
    PlanTaskStatus::Pending
}

/// Plan task status.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanTaskStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

/// Agent analysis report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentAnalysis {
    pub agent: String,
    pub role: String,
    pub findings: Vec<String>,
    pub recommendations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority_items: Option<Vec<String>>,
}

/// Plan input parameters matching the schema in definitions.rs.
#[derive(Debug, Clone, Deserialize)]
struct PlanArgs {
    title: String,
    description: String,
    #[serde(default)]
    architecture: Option<String>,
    #[serde(default)]
    tech_stack: Option<Vec<String>>,
    tasks: Vec<PlanTaskInput>,
    #[serde(default)]
    use_cases: Option<Vec<Value>>,
    agent_analyses: Vec<AgentAnalysisInput>,
    #[serde(default)]
    risks: Option<Vec<Value>>,
    #[serde(default)]
    success_criteria: Option<Vec<String>>,
    #[serde(default)]
    timeline: Option<String>,
    #[serde(default)]
    estimated_changes: Option<String>,
}

/// Task input from the LLM.
#[derive(Debug, Clone, Deserialize)]
struct PlanTaskInput {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    subtasks: Option<Vec<String>>,
    #[serde(default)]
    dependencies: Option<Vec<String>>,
    #[serde(default)]
    complexity: Option<String>,
    #[serde(default)]
    estimated_time: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

/// Agent analysis input from the LLM.
#[derive(Debug, Clone, Deserialize)]
struct AgentAnalysisInput {
    agent: String,
    role: String,
    findings: Vec<String>,
    recommendations: Vec<String>,
    #[serde(default)]
    risk_level: Option<String>,
    #[serde(default)]
    priority_items: Option<Vec<String>>,
}

/// Plan handler for presenting implementation plans.
pub struct PlanHandler;

impl PlanHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PlanHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for PlanHandler {
    fn name(&self) -> &str {
        "Plan"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        // Parse the arguments according to the schema
        let args: PlanArgs = serde_json::from_value(arguments).map_err(|e| {
            crate::error::CortexError::ToolExecution {
                tool: "Plan".to_string(),
                message: format!("Failed to parse plan arguments: {}", e),
            }
        })?;

        // Validate: max 50 tasks
        if args.tasks.len() > 50 {
            return Err(crate::error::CortexError::ToolExecution {
                tool: "Plan".to_string(),
                message: format!("Too many tasks: {} (max 50)", args.tasks.len()),
            });
        }

        // Convert tasks to internal format
        let tasks: Vec<PlanTask> = args
            .tasks
            .into_iter()
            .map(|t| {
                let status = match t.status.as_deref() {
                    Some("in_progress") => PlanTaskStatus::InProgress,
                    Some("completed") => PlanTaskStatus::Completed,
                    _ => PlanTaskStatus::Pending,
                };
                PlanTask {
                    id: t.id,
                    title: t.title,
                    description: t.description,
                    status,
                    subtasks: t.subtasks,
                    dependencies: t.dependencies,
                    estimated_time: t.estimated_time,
                    complexity: t.complexity,
                }
            })
            .collect();

        // Convert agent analyses to internal format
        let agent_analyses: Vec<AgentAnalysis> = args
            .agent_analyses
            .into_iter()
            .map(|a| AgentAnalysis {
                agent: a.agent,
                role: a.role,
                findings: a.findings,
                recommendations: a.recommendations,
                risk_level: a.risk_level,
                priority_items: a.priority_items,
            })
            .collect();

        // Create plan data structure for metadata
        let plan_data = json!({
            "type": "plan",
            "title": args.title,
            "description": args.description,
            "architecture": args.architecture,
            "tech_stack": args.tech_stack,
            "tasks": tasks,
            "use_cases": args.use_cases,
            "agent_analyses": agent_analyses,
            "risks": args.risks,
            "success_criteria": args.success_criteria,
            "timeline": args.timeline,
            "estimated_changes": args.estimated_changes,
            "status": "pending_approval"
        });

        // Format output as readable markdown
        let mut output = format!("# {}\n\n", args.title);
        output.push_str(&format!("{}\n\n", args.description));

        // Architecture section
        if let Some(ref arch) = args.architecture {
            output.push_str("## Architecture\n\n");
            output.push_str(&format!("{}\n\n", arch));
        }

        // Tech stack section
        if let Some(ref stack) = args.tech_stack {
            if !stack.is_empty() {
                output.push_str("## Tech Stack\n\n");
                for tech in stack {
                    output.push_str(&format!("- {}\n", tech));
                }
                output.push('\n');
            }
        }

        // Tasks section
        output.push_str("## Tasks\n\n");
        for task in &tasks {
            let status_icon = match task.status {
                PlanTaskStatus::Pending => "[ ]",
                PlanTaskStatus::InProgress => "[~]",
                PlanTaskStatus::Completed => "[x]",
            };

            let complexity_badge = task
                .complexity
                .as_ref()
                .map(|c| format!(" [{}]", c))
                .unwrap_or_default();

            let time_badge = task
                .estimated_time
                .as_ref()
                .map(|t| format!(" (~{})", t))
                .unwrap_or_default();

            output.push_str(&format!(
                "{} **{}**: {}{}{}\n",
                status_icon, task.id, task.title, complexity_badge, time_badge
            ));

            if let Some(ref desc) = task.description {
                output.push_str(&format!("   {}\n", desc));
            }

            if let Some(ref subtasks) = task.subtasks {
                for subtask in subtasks {
                    output.push_str(&format!("   - {}\n", subtask));
                }
            }

            if let Some(ref deps) = task.dependencies {
                if !deps.is_empty() {
                    output.push_str(&format!("   Dependencies: {}\n", deps.join(", ")));
                }
            }
            output.push('\n');
        }

        // Agent analyses section
        output.push_str("## Expert Analyses\n\n");
        for analysis in &agent_analyses {
            let risk_badge = analysis
                .risk_level
                .as_ref()
                .map(|r| format!(" [Risk: {}]", r))
                .unwrap_or_default();

            output.push_str(&format!(
                "### {} ({}){}\n\n",
                analysis.agent, analysis.role, risk_badge
            ));

            output.push_str("**Findings:**\n");
            for finding in &analysis.findings {
                output.push_str(&format!("- {}\n", finding));
            }
            output.push('\n');

            output.push_str("**Recommendations:**\n");
            for rec in &analysis.recommendations {
                output.push_str(&format!("- {}\n", rec));
            }

            if let Some(ref priorities) = analysis.priority_items {
                if !priorities.is_empty() {
                    output.push_str("\n**Priority Items:**\n");
                    for item in priorities {
                        output.push_str(&format!("- ⚠️ {}\n", item));
                    }
                }
            }
            output.push('\n');
        }

        // Risks section
        if let Some(ref risks) = args.risks {
            if !risks.is_empty() {
                output.push_str("## Risks\n\n");
                for risk in risks {
                    if let Some(risk_str) = risk.as_str() {
                        output.push_str(&format!("- {}\n", risk_str));
                    } else if let Some(risk_obj) = risk.as_object() {
                        let risk_text = risk_obj
                            .get("risk")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Unknown risk");
                        let level = risk_obj
                            .get("level")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let mitigation = risk_obj.get("mitigation").and_then(|v| v.as_str());

                        output.push_str(&format!("- **[{}]** {}", level.to_uppercase(), risk_text));
                        if let Some(mit) = mitigation {
                            output.push_str(&format!("\n  Mitigation: {}", mit));
                        }
                        output.push('\n');
                    }
                }
                output.push('\n');
            }
        }

        // Success criteria section
        if let Some(ref criteria) = args.success_criteria {
            if !criteria.is_empty() {
                output.push_str("## Success Criteria\n\n");
                for criterion in criteria {
                    output.push_str(&format!("- {}\n", criterion));
                }
                output.push('\n');
            }
        }

        // Timeline and scope
        if args.timeline.is_some() || args.estimated_changes.is_some() {
            output.push_str("## Scope\n\n");
            if let Some(ref timeline) = args.timeline {
                output.push_str(&format!("**Timeline:** {}\n", timeline));
            }
            if let Some(ref changes) = args.estimated_changes {
                output.push_str(&format!("**Estimated Changes:** {}\n", changes));
            }
            output.push('\n');
        }

        output.push_str("---\n*Awaiting approval to proceed with implementation.*\n");

        Ok(ToolResult::success(output).with_metadata(ToolMetadata {
            duration_ms: 0,
            exit_code: None,
            files_modified: vec![],
            data: Some(plan_data),
        }))
    }
}
