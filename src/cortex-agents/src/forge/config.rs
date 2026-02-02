//! TOML configuration system for Forge orchestration.
//!
//! This module handles loading and managing agent configurations from TOML files.
//! It supports both project-local (`.cortex/forge/`) and global (`~/.cortex/forge/`)
//! configuration directories.
//!
//! # Configuration Structure
//!
//! ```text
//! .cortex/forge/
//! ├── forge.toml           # Main configuration
//! └── agents/
//!     ├── security/
//!     │   └── rules.toml   # Security agent rules
//!     └── quality/
//!         └── rules.toml   # Quality agent rules
//! ```
//!
//! # Example
//!
//! ```rust
//! use cortex_agents::forge::{ConfigLoader, ForgeConfig};
//!
//! // Load configuration from default locations
//! let loader = ConfigLoader::new();
//! // In real use: let config = loader.load().await?;
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors that can occur during configuration loading.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to read a configuration file.
    #[error("Failed to read config file '{path}': {source}")]
    ReadError {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to parse TOML content.
    #[error("Failed to parse TOML in '{path}': {source}")]
    ParseError {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },

    /// Configuration validation failed.
    #[error("Configuration validation error: {0}")]
    ValidationError(String),

    /// Agent configuration not found.
    #[error("Agent configuration not found: {0}")]
    AgentNotFound(String),

    /// Invalid configuration value.
    #[error("Invalid configuration value for '{key}': {message}")]
    InvalidValue { key: String, message: String },
}

/// Result type for configuration operations.
pub type ConfigResult<T> = std::result::Result<T, ConfigError>;

/// Configuration for an individual rule.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuleConfig {
    /// Unique identifier for the rule.
    pub id: String,

    /// Human-readable name.
    #[serde(default)]
    pub name: String,

    /// Whether the rule is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Rule severity override.
    #[serde(default)]
    pub severity: Option<String>,

    /// Additional rule-specific options.
    #[serde(default, flatten)]
    pub options: HashMap<String, toml::Value>,
}

fn default_true() -> bool {
    true
}

impl Default for RuleConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            severity: None,
            options: HashMap::new(),
        }
    }
}

impl RuleConfig {
    /// Create a new rule configuration.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            ..Default::default()
        }
    }

    /// Set the rule name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Disable the rule.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Set severity override.
    pub fn with_severity(mut self, severity: impl Into<String>) -> Self {
        self.severity = Some(severity.into());
        self
    }
}

/// Configuration for an agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Agent identifier (set from TOML key when parsed from config).
    #[serde(default)]
    pub id: String,

    /// Human-readable agent name.
    #[serde(default)]
    pub name: String,

    /// Whether the agent is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Agents that this agent depends on.
    #[serde(default)]
    pub depends_on: Vec<String>,

    /// Whether this agent requires all dependencies to pass.
    #[serde(default = "default_true")]
    pub require_dependencies_pass: bool,

    /// Timeout in seconds for this agent's execution.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Priority for execution order (higher = earlier).
    #[serde(default)]
    pub priority: i32,

    /// Rules configuration for this agent.
    #[serde(default)]
    pub rules: Vec<RuleConfig>,

    /// Additional agent-specific settings.
    #[serde(default, flatten)]
    pub settings: HashMap<String, toml::Value>,
}

fn default_timeout() -> u64 {
    300 // 5 minutes default
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            depends_on: Vec::new(),
            require_dependencies_pass: true,
            timeout_seconds: default_timeout(),
            priority: 0,
            rules: Vec::new(),
            settings: HashMap::new(),
        }
    }
}

impl AgentConfig {
    /// Create a new agent configuration.
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        Self {
            name: id.clone(),
            id,
            ..Default::default()
        }
    }

    /// Set the agent name.
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Add a dependency.
    pub fn depends_on(mut self, agent_id: impl Into<String>) -> Self {
        self.depends_on.push(agent_id.into());
        self
    }

    /// Set multiple dependencies.
    pub fn with_dependencies(mut self, deps: Vec<String>) -> Self {
        self.depends_on = deps;
        self
    }

    /// Set timeout.
    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = seconds;
        self
    }

    /// Set priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Add a rule configuration.
    pub fn with_rule(mut self, rule: RuleConfig) -> Self {
        self.rules.push(rule);
        self
    }

    /// Add rules.
    pub fn with_rules(mut self, rules: Vec<RuleConfig>) -> Self {
        self.rules = rules;
        self
    }

    /// Disable the agent.
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Get a setting value.
    pub fn get_setting(&self, key: &str) -> Option<&toml::Value> {
        self.settings.get(key)
    }

    /// Get enabled rules only.
    pub fn enabled_rules(&self) -> impl Iterator<Item = &RuleConfig> {
        self.rules.iter().filter(|r| r.enabled)
    }
}

/// Main Forge configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForgeConfig {
    /// Configuration version.
    #[serde(default = "default_version")]
    pub version: String,

    /// Global settings.
    #[serde(default)]
    pub global: GlobalConfig,

    /// Agent configurations by ID.
    #[serde(default)]
    pub agents: HashMap<String, AgentConfig>,

    /// Default settings for all agents.
    #[serde(default)]
    pub defaults: AgentDefaults,
}

fn default_version() -> String {
    "1".to_string()
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            global: GlobalConfig::default(),
            agents: HashMap::new(),
            defaults: AgentDefaults::default(),
        }
    }
}

/// Global Forge settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GlobalConfig {
    /// Maximum number of agents to run in parallel.
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,

    /// Whether to fail fast on first error.
    #[serde(default)]
    pub fail_fast: bool,

    /// Global timeout in seconds.
    #[serde(default = "default_global_timeout")]
    pub timeout_seconds: u64,

    /// Output format for results.
    #[serde(default)]
    pub output_format: OutputFormat,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            max_parallel: default_max_parallel(),
            fail_fast: false,
            timeout_seconds: default_global_timeout(),
            output_format: OutputFormat::default(),
        }
    }
}

fn default_max_parallel() -> usize {
    4
}

fn default_global_timeout() -> u64 {
    600 // 10 minutes
}

/// Output format for Forge results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum OutputFormat {
    /// JSON output.
    #[default]
    Json,
    /// Human-readable text output.
    Text,
    /// Markdown output.
    Markdown,
}

/// Default settings for agents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentDefaults {
    /// Default timeout in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u64,

    /// Default: require dependencies to pass.
    #[serde(default = "default_true")]
    pub require_dependencies_pass: bool,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout(),
            require_dependencies_pass: true,
        }
    }
}

impl ForgeConfig {
    /// Create a new empty configuration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an agent configuration.
    pub fn add_agent(&mut self, config: AgentConfig) {
        self.agents.insert(config.id.clone(), config);
    }

    /// Get an agent configuration by ID.
    pub fn get_agent(&self, id: &str) -> Option<&AgentConfig> {
        self.agents.get(id)
    }

    /// Get a mutable agent configuration by ID.
    pub fn get_agent_mut(&mut self, id: &str) -> Option<&mut AgentConfig> {
        self.agents.get_mut(id)
    }

    /// Get all enabled agents.
    pub fn enabled_agents(&self) -> impl Iterator<Item = &AgentConfig> {
        self.agents.values().filter(|a| a.enabled)
    }

    /// Validate the configuration.
    pub fn validate(&self) -> ConfigResult<()> {
        // Check for circular dependencies
        self.check_circular_dependencies()?;

        // Check that all dependencies exist
        for agent in self.agents.values() {
            for dep in &agent.depends_on {
                if !self.agents.contains_key(dep) {
                    return Err(ConfigError::ValidationError(format!(
                        "Agent '{}' depends on unknown agent '{}'",
                        agent.id, dep
                    )));
                }
            }
        }

        Ok(())
    }

    /// Check for circular dependencies using DFS.
    fn check_circular_dependencies(&self) -> ConfigResult<()> {
        use std::collections::HashSet;

        fn dfs(
            agent_id: &str,
            agents: &HashMap<String, AgentConfig>,
            visiting: &mut HashSet<String>,
            visited: &mut HashSet<String>,
        ) -> ConfigResult<()> {
            if visiting.contains(agent_id) {
                return Err(ConfigError::ValidationError(format!(
                    "Circular dependency detected involving agent '{agent_id}'"
                )));
            }

            if visited.contains(agent_id) {
                return Ok(());
            }

            visiting.insert(agent_id.to_string());

            if let Some(agent) = agents.get(agent_id) {
                for dep in &agent.depends_on {
                    dfs(dep, agents, visiting, visited)?;
                }
            }

            visiting.remove(agent_id);
            visited.insert(agent_id.to_string());

            Ok(())
        }

        let mut visiting = HashSet::new();
        let mut visited = HashSet::new();

        for agent_id in self.agents.keys() {
            dfs(agent_id, &self.agents, &mut visiting, &mut visited)?;
        }

        Ok(())
    }

    /// Parse from TOML string.
    pub fn from_toml(content: &str) -> ConfigResult<Self> {
        let mut config: Self = toml::from_str(content).map_err(|e| ConfigError::ParseError {
            path: PathBuf::from("<string>"),
            source: e,
        })?;

        // Set agent IDs from their map keys
        for (id, agent) in config.agents.iter_mut() {
            if agent.id.is_empty() {
                agent.id = id.clone();
            }
            // Set name to id if not specified
            if agent.name.is_empty() {
                agent.name = id.clone();
            }
        }

        Ok(config)
    }

    /// Serialize to TOML string.
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Merge another configuration into this one.
    ///
    /// Agent configurations are merged by ID, with the other config taking precedence.
    pub fn merge(&mut self, other: ForgeConfig) {
        // Merge global settings (other takes precedence)
        self.global = other.global;
        self.defaults = other.defaults;

        // Merge agents
        for (id, agent) in other.agents {
            self.agents.insert(id, agent);
        }
    }
}

/// Configuration loader that searches multiple locations.
#[derive(Debug, Clone)]
pub struct ConfigLoader {
    /// Project-local configuration directory.
    project_config_dir: Option<PathBuf>,
    /// Global configuration directory.
    global_config_dir: Option<PathBuf>,
}

impl Default for ConfigLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigLoader {
    /// Create a new config loader with default paths.
    pub fn new() -> Self {
        let global_config_dir = dirs::home_dir().map(|h| h.join(".cortex").join("forge"));

        Self {
            project_config_dir: None,
            global_config_dir,
        }
    }

    /// Set the project root to look for `.cortex/forge/` directory.
    pub fn with_project_root(mut self, root: impl AsRef<Path>) -> Self {
        self.project_config_dir = Some(root.as_ref().join(".cortex").join("forge"));
        self
    }

    /// Set a custom project config directory.
    pub fn with_project_config_dir(mut self, path: impl AsRef<Path>) -> Self {
        self.project_config_dir = Some(path.as_ref().to_path_buf());
        self
    }

    /// Set a custom global config directory.
    pub fn with_global_config_dir(mut self, path: impl AsRef<Path>) -> Self {
        self.global_config_dir = Some(path.as_ref().to_path_buf());
        self
    }

    /// Load configuration from all available locations.
    ///
    /// Project-local configuration takes precedence over global configuration.
    pub async fn load(&self) -> ConfigResult<ForgeConfig> {
        let mut config = ForgeConfig::default();

        // Load global config first
        if let Some(ref global_dir) = self.global_config_dir {
            if let Some(global_config) = self.load_from_dir(global_dir).await? {
                config = global_config;
            }
        }

        // Load project config and merge (takes precedence)
        if let Some(ref project_dir) = self.project_config_dir {
            if let Some(project_config) = self.load_from_dir(project_dir).await? {
                config.merge(project_config);
            }
        }

        // Load agent-specific rules
        self.load_agent_rules(&mut config).await?;

        // Validate final configuration
        config.validate()?;

        Ok(config)
    }

    /// Load configuration from a specific directory.
    async fn load_from_dir(&self, dir: &Path) -> ConfigResult<Option<ForgeConfig>> {
        let config_file = dir.join("forge.toml");

        if !config_file.exists() {
            return Ok(None);
        }

        let content =
            tokio::fs::read_to_string(&config_file)
                .await
                .map_err(|e| ConfigError::ReadError {
                    path: config_file.clone(),
                    source: e,
                })?;

        let config = toml::from_str(&content).map_err(|e| ConfigError::ParseError {
            path: config_file,
            source: e,
        })?;

        Ok(Some(config))
    }

    /// Load agent-specific rules from `agents/<agent>/rules.toml` files.
    async fn load_agent_rules(&self, config: &mut ForgeConfig) -> ConfigResult<()> {
        for dir in [&self.project_config_dir, &self.global_config_dir]
            .into_iter()
            .flatten()
        {
            let agents_dir = dir.join("agents");
            if !agents_dir.exists() {
                continue;
            }

            let mut entries = match tokio::fs::read_dir(&agents_dir).await {
                Ok(entries) => entries,
                Err(_) => continue,
            };

            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                let agent_id = match path.file_name().and_then(|n| n.to_str()) {
                    Some(name) => name.to_string(),
                    None => continue,
                };

                let rules_file = path.join("rules.toml");
                if !rules_file.exists() {
                    continue;
                }

                let content = tokio::fs::read_to_string(&rules_file).await.map_err(|e| {
                    ConfigError::ReadError {
                        path: rules_file.clone(),
                        source: e,
                    }
                })?;

                let rules_config = AgentRulesFile::from_toml(&content).map_err(|e| match e {
                    ConfigError::ParseError { source, .. } => ConfigError::ParseError {
                        path: rules_file.clone(),
                        source,
                    },
                    other => other,
                })?;

                // Merge rules into agent config or create a new agent config
                if let Some(agent) = config.agents.get_mut(&agent_id) {
                    // Agent exists, extend its rules
                    agent.rules.extend(rules_config.to_rule_configs());

                    // Update agent metadata if provided
                    if !rules_config.agent.name.is_empty() && agent.name.is_empty() {
                        agent.name = rules_config.agent.name.clone();
                    }
                } else {
                    // Agent doesn't exist, create it from TOML config
                    let mut new_agent = AgentConfig::new(&agent_id);
                    new_agent.name = if rules_config.agent.name.is_empty() {
                        agent_id.clone()
                    } else {
                        rules_config.agent.name.clone()
                    };
                    new_agent.enabled = rules_config.agent.enabled;
                    new_agent.priority = rules_config.agent.priority;
                    new_agent.rules = rules_config.to_rule_configs();

                    config.add_agent(new_agent);
                }
            }
        }

        Ok(())
    }

    /// Load a specific agent's configuration.
    pub async fn load_agent(&self, agent_id: &str) -> ConfigResult<AgentConfig> {
        let config = self.load().await?;
        config
            .get_agent(agent_id)
            .cloned()
            .ok_or_else(|| ConfigError::AgentNotFound(agent_id.to_string()))
    }

    /// Check if configuration exists.
    pub fn config_exists(&self) -> bool {
        let project_exists = self
            .project_config_dir
            .as_ref()
            .is_some_and(|d| d.join("forge.toml").exists());

        let global_exists = self
            .global_config_dir
            .as_ref()
            .is_some_and(|d| d.join("forge.toml").exists());

        project_exists || global_exists
    }
}

/// Agent metadata from rules TOML files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentMetadata {
    /// Agent identifier.
    pub id: String,

    /// Human-readable name.
    #[serde(default)]
    pub name: String,

    /// Description of what the agent does.
    #[serde(default)]
    pub description: String,

    /// Whether the agent is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Priority for execution order (higher = earlier).
    #[serde(default)]
    pub priority: i32,
}

impl Default for AgentMetadata {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            enabled: true,
            priority: 0,
        }
    }
}

/// A rule definition that can be loaded from TOML configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DynamicRuleDefinition {
    /// Whether the rule is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Rule severity (info, warning, error, critical).
    #[serde(default)]
    pub severity: Option<String>,

    /// Description of the rule.
    #[serde(default)]
    pub description: String,

    /// Regex patterns to match for this rule.
    #[serde(default)]
    pub patterns: Vec<String>,

    /// Patterns to exclude from matching.
    #[serde(default)]
    pub exclude_patterns: Vec<String>,

    /// Maximum allowed occurrences (for counting rules).
    #[serde(default)]
    pub max_allowed: Option<u32>,

    /// Whether to check Cargo.lock for this rule.
    #[serde(default)]
    pub check_cargo_lock: bool,

    /// Whether to check package-lock.json for this rule.
    #[serde(default)]
    pub check_package_lock: bool,

    /// Whether a safety comment is required (for unsafe code checks).
    #[serde(default)]
    pub require_safety_comment: bool,

    /// Files where this rule is allowed.
    #[serde(default)]
    pub allowed_files: Vec<String>,

    /// Whether to allow in test files.
    #[serde(default)]
    pub allow_in_tests: bool,

    /// Whether to require module documentation.
    #[serde(default)]
    pub require_module_docs: bool,

    /// Whether to require function documentation.
    #[serde(default)]
    pub require_function_docs: bool,

    /// Minimum documentation length.
    #[serde(default)]
    pub min_doc_length: Option<u32>,

    /// Additional rule-specific options stored as generic map.
    #[serde(default, flatten)]
    pub extra: HashMap<String, toml::Value>,
}

impl Default for DynamicRuleDefinition {
    fn default() -> Self {
        Self {
            enabled: true,
            severity: None,
            description: String::new(),
            patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            max_allowed: None,
            check_cargo_lock: false,
            check_package_lock: false,
            require_safety_comment: false,
            allowed_files: Vec::new(),
            allow_in_tests: false,
            require_module_docs: false,
            require_function_docs: false,
            min_doc_length: None,
            extra: HashMap::new(),
        }
    }
}

/// Aggregator-specific thresholds configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregatorThresholds {
    /// Maximum allowed errors before blocking.
    #[serde(default)]
    pub max_errors: u32,

    /// Maximum allowed warnings.
    #[serde(default = "default_max_warnings")]
    pub max_warnings: u32,

    /// Whether all agents must pass.
    #[serde(default = "default_true")]
    pub require_all_pass: bool,
}

fn default_max_warnings() -> u32 {
    10
}

impl Default for AggregatorThresholds {
    fn default() -> Self {
        Self {
            max_errors: 0,
            max_warnings: default_max_warnings(),
            require_all_pass: true,
        }
    }
}

/// Aggregator-specific actions configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AggregatorActions {
    /// Action on pass: "proceed" or custom action.
    #[serde(default = "default_on_pass")]
    pub on_pass: String,

    /// Action on fail: "block" or custom action.
    #[serde(default = "default_on_fail")]
    pub on_fail: String,

    /// Whether to generate a report.
    #[serde(default = "default_true")]
    pub generate_report: bool,

    /// Report format: "markdown", "json", "text".
    #[serde(default = "default_report_format")]
    pub report_format: String,
}

fn default_on_pass() -> String {
    "proceed".to_string()
}

fn default_on_fail() -> String {
    "block".to_string()
}

fn default_report_format() -> String {
    "markdown".to_string()
}

impl Default for AggregatorActions {
    fn default() -> Self {
        Self {
            on_pass: default_on_pass(),
            on_fail: default_on_fail(),
            generate_report: true,
            report_format: default_report_format(),
        }
    }
}

/// Complete agent rules file structure.
/// Supports parsing full agent configuration from TOML including:
/// - [agent] section with metadata
/// - [rules.*] sections with dynamic rule definitions
/// - [thresholds] section for aggregators
/// - [actions] section for aggregators
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentRulesFile {
    /// Agent metadata.
    #[serde(default)]
    pub agent: AgentMetadata,

    /// Rules defined as a map from rule_id to rule definition.
    #[serde(default)]
    pub rules: HashMap<String, DynamicRuleDefinition>,

    /// Aggregator thresholds (only applicable to aggregator agents).
    #[serde(default)]
    pub thresholds: Option<AggregatorThresholds>,

    /// Aggregator actions (only applicable to aggregator agents).
    #[serde(default)]
    pub actions: Option<AggregatorActions>,
}

impl AgentRulesFile {
    /// Parse from TOML string.
    pub fn from_toml(content: &str) -> ConfigResult<Self> {
        toml::from_str(content).map_err(|e| ConfigError::ParseError {
            path: PathBuf::from("<string>"),
            source: e,
        })
    }

    /// Convert rules map to Vec<RuleConfig> for backward compatibility.
    pub fn to_rule_configs(&self) -> Vec<RuleConfig> {
        self.rules
            .iter()
            .map(|(id, def)| {
                RuleConfig {
                    id: id.clone(),
                    name: def.description.clone(),
                    enabled: def.enabled,
                    severity: def.severity.clone(),
                    options: HashMap::new(), // Options come from extra fields
                }
            })
            .collect()
    }

    /// Get enabled rules only.
    pub fn enabled_rules(&self) -> impl Iterator<Item = (&String, &DynamicRuleDefinition)> {
        self.rules.iter().filter(|(_, def)| def.enabled)
    }

    /// Get a rule by ID.
    pub fn get_rule(&self, rule_id: &str) -> Option<&DynamicRuleDefinition> {
        self.rules.get(rule_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rule_config_builder() {
        let rule = RuleConfig::new("SEC001")
            .with_name("No hardcoded secrets")
            .with_severity("error");

        assert_eq!(rule.id, "SEC001");
        assert_eq!(rule.name, "No hardcoded secrets");
        assert!(rule.enabled);
        assert_eq!(rule.severity, Some("error".to_string()));
    }

    #[test]
    fn test_agent_config_builder() {
        let agent = AgentConfig::new("security-scanner")
            .with_name("Security Scanner")
            .depends_on("code-quality")
            .with_timeout(120)
            .with_priority(10)
            .with_rule(RuleConfig::new("SEC001"));

        assert_eq!(agent.id, "security-scanner");
        assert_eq!(agent.name, "Security Scanner");
        assert_eq!(agent.depends_on, vec!["code-quality"]);
        assert_eq!(agent.timeout_seconds, 120);
        assert_eq!(agent.priority, 10);
        assert_eq!(agent.rules.len(), 1);
    }

    #[test]
    fn test_forge_config_from_toml() {
        let toml_content = r#"
version = "1"

[global]
max_parallel = 8
fail_fast = true

[defaults]
timeout_seconds = 180

[agents.security]
name = "Security Scanner"
priority = 10
depends_on = ["lint"]

[[agents.security.rules]]
id = "SEC001"
name = "No secrets"
enabled = true

[agents.lint]
name = "Linter"
priority = 5
"#;

        let config = ForgeConfig::from_toml(toml_content).expect("should parse");

        assert_eq!(config.version, "1");
        assert_eq!(config.global.max_parallel, 8);
        assert!(config.global.fail_fast);
        assert_eq!(config.defaults.timeout_seconds, 180);

        let security = config
            .get_agent("security")
            .expect("should have security agent");
        assert_eq!(security.name, "Security Scanner");
        assert_eq!(security.priority, 10);
        assert_eq!(security.depends_on, vec!["lint"]);
        assert_eq!(security.rules.len(), 1);

        let lint = config.get_agent("lint").expect("should have lint agent");
        assert_eq!(lint.name, "Linter");
    }

    #[test]
    fn test_forge_config_validation_circular_deps() {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("a").depends_on("b"));
        config.add_agent(AgentConfig::new("b").depends_on("c"));
        config.add_agent(AgentConfig::new("c").depends_on("a"));

        let result = config.validate();
        assert!(result.is_err());

        if let Err(ConfigError::ValidationError(msg)) = result {
            assert!(msg.contains("Circular dependency"));
        } else {
            panic!("Expected circular dependency error");
        }
    }

    #[test]
    fn test_forge_config_validation_missing_dep() {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("a").depends_on("nonexistent"));

        let result = config.validate();
        assert!(result.is_err());

        if let Err(ConfigError::ValidationError(msg)) = result {
            assert!(msg.contains("unknown agent"));
        } else {
            panic!("Expected unknown agent error");
        }
    }

    #[test]
    fn test_forge_config_merge() {
        let mut base = ForgeConfig::new();
        base.add_agent(AgentConfig::new("a").with_priority(1));
        base.add_agent(AgentConfig::new("b").with_priority(2));

        let mut overlay = ForgeConfig::new();
        overlay.add_agent(AgentConfig::new("a").with_priority(10)); // Override
        overlay.add_agent(AgentConfig::new("c").with_priority(3)); // New

        base.merge(overlay);

        assert_eq!(base.agents.len(), 3);
        assert_eq!(base.get_agent("a").unwrap().priority, 10);
        assert_eq!(base.get_agent("b").unwrap().priority, 2);
        assert_eq!(base.get_agent("c").unwrap().priority, 3);
    }

    #[test]
    fn test_forge_config_enabled_agents() {
        let mut config = ForgeConfig::new();
        config.add_agent(AgentConfig::new("enabled1"));
        config.add_agent(AgentConfig::new("disabled").disabled());
        config.add_agent(AgentConfig::new("enabled2"));

        let enabled: Vec<_> = config.enabled_agents().collect();
        assert_eq!(enabled.len(), 2);
    }

    #[test]
    fn test_config_loader_new() {
        let loader = ConfigLoader::new();
        assert!(loader.global_config_dir.is_some());
    }

    #[test]
    fn test_forge_config_to_toml() {
        let mut config = ForgeConfig::new();
        config.global.max_parallel = 4;
        config.add_agent(AgentConfig::new("test").with_priority(5));

        let toml = config.to_toml().expect("should serialize");
        assert!(toml.contains("max_parallel"));
        assert!(toml.contains("test"));
    }
}
