//! Forge orchestration system for code validation.
//!
//! The Forge system provides a comprehensive validation framework for
//! analyzing codebases through specialized validation agents.
//!
//! # Architecture
//!
//! - **ValidationAgent**: Trait defining the interface for all validation agents
//! - **SecurityAgent**: Security-focused analysis (secrets, vulnerabilities, unsafe code)
//! - **QualityAgent**: Code quality checks (TODOs, unwrap, documentation)
//! - **AggregatorAgent**: Combines results from all agents into a final report
//! - **ForgeOrchestrator**: DAG-based orchestration for agent execution
//! - **ForgeConfig**: TOML-based configuration system
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_agents::forge::agents::{
//!     ValidationAgent, ValidationContext, SecurityAgent, QualityAgent, AggregatorAgent
//! };
//!
//! // Create agents
//! let security = SecurityAgent::new();
//! let quality = QualityAgent::new();
//! let aggregator = AggregatorAgent::new();
//!
//! // Run validation
//! let ctx = ValidationContext::new("/path/to/project");
//! let security_result = security.validate(&ctx).await?;
//! let quality_result = quality.validate(&ctx).await?;
//!
//! // Aggregate results
//! let ctx = ctx
//!     .with_previous_result(security_result)
//!     .with_previous_result(quality_result);
//! let final_result = aggregator.validate(&ctx).await?;
//! ```
//!
//! # Orchestration
//!
//! For complex workflows with dependencies:
//!
//! ```rust,ignore
//! use cortex_agents::forge::{ForgeOrchestrator, ForgeConfig};
//! use cortex_agents::forge::config::AgentConfig as OrchestratorAgentConfig;
//! use cortex_agents::forge::protocol::ValidationResult as OrchestratorValidationResult;
//!
//! let mut config = ForgeConfig::new();
//! config.add_agent(OrchestratorAgentConfig::new("lint").with_priority(10));
//! config.add_agent(
//!     OrchestratorAgentConfig::new("security")
//!         .depends_on("lint")
//!         .with_priority(5)
//! );
//!
//! let orchestrator = ForgeOrchestrator::new(config);
//! let response = orchestrator.run(|agent_id, config| async move {
//!     // Execute agent logic
//!     Ok(OrchestratorValidationResult::pass(agent_id))
//! }).await?;
//! ```

pub mod agents;
pub mod config;
pub mod orchestrator;
pub mod protocol;

// Re-export commonly used types at the forge module level
pub use agents::{
    create_agent_from_toml, create_agent_from_toml_str, AgentConfig, AgentError, AggregatorAgent,
    DynamicAgent, Finding, QualityAgent, RuleInfo, SecurityAgent, Severity, ValidationAgent,
    ValidationContext, ValidationResult, ValidationStatus,
};

// Re-export aggregator-specific types
pub use agents::aggregator::{AgentSummary, ForgeRecommendation, ForgeResponse};

// Re-export config types for orchestration
pub use config::{
    AgentMetadata, AgentRulesFile, AggregatorActions, AggregatorThresholds, ConfigError,
    ConfigLoader, ConfigResult, DynamicRuleDefinition, ForgeConfig, GlobalConfig, OutputFormat,
};

// Re-export orchestrator types
pub use orchestrator::{
    ForgeOrchestrator, OrchestratorBuilder, OrchestratorError, OrchestratorOptions,
    OrchestratorResult,
};

use std::path::Path;

/// Load all agents from a configuration directory.
///
/// This function scans the `.cortex/forge/agents/` directory and creates
/// agents dynamically from their rules.toml files. ALL agents are loaded
/// from their TOML configuration files - there are no hardcoded agents.
///
/// # Arguments
///
/// * `agents_dir` - Path to the agents directory (e.g., `.cortex/forge/agents/`)
///
/// # Returns
///
/// A vector of boxed ValidationAgent trait objects that can be used with the orchestrator.
///
/// # Example
///
/// ```rust,ignore
/// use cortex_agents::forge::load_agents_from_directory;
/// use std::path::Path;
///
/// let agents = load_agents_from_directory(Path::new(".cortex/forge/agents")).await?;
/// for agent in agents {
///     println!("Loaded agent: {}", agent.id());
/// }
/// ```
pub async fn load_agents_from_directory(
    agents_dir: &Path,
) -> Result<Vec<Box<dyn ValidationAgent>>, AgentError> {
    let mut agents: Vec<Box<dyn ValidationAgent>> = Vec::new();

    if !agents_dir.exists() {
        return Ok(agents);
    }

    let mut entries = tokio::fs::read_dir(agents_dir)
        .await
        .map_err(AgentError::Io)?;

    while let Some(entry) = entries.next_entry().await.map_err(AgentError::Io)? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let rules_file = path.join("rules.toml");
        if !rules_file.exists() {
            continue;
        }

        // Always load agent dynamically from its rules.toml configuration
        let dynamic_agent = create_agent_from_toml(&rules_file).await?;
        agents.push(Box::new(dynamic_agent));
    }

    Ok(agents)
}

/// Create a dynamic agent by ID from a rules file.
///
/// This function loads agent configuration from a TOML rules file.
/// If no rules file is provided, it creates a default built-in agent
/// for backwards compatibility with known agent IDs (security, quality, aggregator).
///
/// # Arguments
///
/// * `agent_id` - The agent identifier
/// * `rules_file` - Optional path to a rules.toml file for the agent configuration
///
/// # Returns
///
/// A boxed ValidationAgent trait object.
///
/// # Behavior
///
/// - If `rules_file` is provided, the agent is always loaded dynamically from the TOML file
/// - If `rules_file` is `None`, a default built-in agent is created for known IDs
/// - Returns an error if `agent_id` is unknown and no `rules_file` is provided
pub async fn create_agent(
    agent_id: &str,
    rules_file: Option<&Path>,
) -> Result<Box<dyn ValidationAgent>, AgentError> {
    // If a rules file is provided, always load dynamically from it
    if let Some(path) = rules_file {
        let dynamic_agent = create_agent_from_toml(path).await?;
        return Ok(Box::new(dynamic_agent));
    }

    // No rules file provided - create default built-in agents for backwards compatibility
    match agent_id {
        "security" => Ok(Box::new(SecurityAgent::new())),
        "quality" => Ok(Box::new(QualityAgent::new())),
        "aggregator" => Ok(Box::new(AggregatorAgent::new())),
        _ => Err(AgentError::Config(format!(
            "Unknown agent '{}' and no rules file provided",
            agent_id
        ))),
    }
}
