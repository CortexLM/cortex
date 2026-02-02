//! Custom agent loader for reading agents from filesystem.

use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use super::config::{CustomAgentConfig, CustomAgentError};

/// Loader for custom agents from filesystem.
#[derive(Debug, Clone)]
pub struct CustomAgentLoader {
    /// Search paths in order of priority.
    search_paths: Vec<PathBuf>,
}

impl Default for CustomAgentLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl CustomAgentLoader {
    /// Create a new loader with no paths.
    pub fn new() -> Self {
        Self {
            search_paths: Vec::new(),
        }
    }

    /// Create a loader with default paths.
    ///
    /// Default paths (in priority order):
    /// 1. `.agents/` (project-local, https://agent.md/ compatible)
    /// 2. `.agent/` (project-local, https://agent.md/ compatible)
    /// 3. `.cortex/agents/` (project-local)
    /// 4. `~/.cortex/agents/` (global personal)
    /// 5. `~/.config/cortex/agents/` (global config)
    pub fn with_default_paths(project_root: Option<&Path>) -> Self {
        let mut loader = Self::new();

        // Project-local agents (multiple formats supported)
        if let Some(root) = project_root {
            // Support https://agent.md/ format (.agents/ and .agent/)
            loader.search_paths.push(root.join(".agents"));
            loader.search_paths.push(root.join(".agent"));
            // Traditional .cortex/agents format
            loader.search_paths.push(root.join(".cortex/agents"));
        }

        // Global personal agents from ~/.cortex/agents/
        if let Some(home) = dirs::home_dir() {
            loader.search_paths.push(home.join(".cortex/agents"));
        }

        // Global agents from config directory
        if let Some(config) = dirs::config_dir() {
            loader.search_paths.push(config.join("cortex/agents"));
        }

        loader
    }

    /// Add a search path.
    pub fn add_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.search_paths.contains(&path) {
            self.search_paths.push(path);
        }
    }

    /// Prepend a search path (highest priority).
    pub fn prepend_path(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if !self.search_paths.contains(&path) {
            self.search_paths.insert(0, path);
        }
    }

    /// Get the search paths.
    pub fn search_paths(&self) -> &[PathBuf] {
        &self.search_paths
    }

    /// Load all custom agents from all search paths.
    pub async fn load_all(&self) -> Result<Vec<CustomAgentConfig>, CustomAgentError> {
        let mut agents = Vec::new();
        let mut seen_names = std::collections::HashSet::new();

        for path in &self.search_paths {
            if !path.exists() {
                debug!("Agent directory {:?} does not exist, skipping", path);
                continue;
            }

            match self.load_from_dir(path).await {
                Ok(dir_agents) => {
                    for agent in dir_agents {
                        if !seen_names.contains(&agent.name) {
                            seen_names.insert(agent.name.clone());
                            agents.push(agent);
                        } else {
                            debug!("Skipping duplicate agent '{}' from {:?}", agent.name, path);
                        }
                    }
                }
                Err(e) => {
                    warn!("Error loading agents from {:?}: {}", path, e);
                }
            }
        }

        Ok(agents)
    }

    /// Load custom agents from a specific directory.
    async fn load_from_dir(&self, dir: &Path) -> Result<Vec<CustomAgentConfig>, CustomAgentError> {
        let mut agents = Vec::new();

        let mut entries = tokio::fs::read_dir(dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();

            // Only process .md files
            if path.extension().is_some_and(|e| e == "md") {
                match self.load_agent(&path).await {
                    Ok(agent) => {
                        debug!("Loaded agent '{}' from {:?}", agent.name, path);
                        agents.push(agent);
                    }
                    Err(e) => {
                        warn!("Failed to load agent from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(agents)
    }

    /// Load a single custom agent from a file.
    async fn load_agent(&self, path: &Path) -> Result<CustomAgentConfig, CustomAgentError> {
        let content = tokio::fs::read_to_string(path).await?;

        // Parse frontmatter YAML
        let (frontmatter, body) = parse_frontmatter(&content)?;

        let mut config: CustomAgentConfig = serde_yaml::from_value(frontmatter)?;
        config.prompt = body;

        // Use filename if name not set
        if config.name.is_empty() {
            config.name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed")
                .to_string();
        }

        Ok(config)
    }

    /// Check if a custom agent exists by name.
    pub async fn exists(&self, name: &str) -> bool {
        for dir in &self.search_paths {
            let path = dir.join(format!("{}.md", name));
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return true;
            }
        }
        false
    }

    /// Get the path where a custom agent would be saved.
    pub fn agent_path(&self, name: &str) -> Option<PathBuf> {
        self.search_paths
            .first()
            .map(|dir| dir.join(format!("{}.md", name)))
    }
}

/// Parse YAML frontmatter from content.
///
/// Supports YAML anchors (`&name`), aliases (`*name`), and merge keys (`<<: *name`)
/// which are resolved before returning the parsed value (#2199).
fn parse_frontmatter(content: &str) -> Result<(serde_yaml::Value, String), CustomAgentError> {
    let content = content.trim();

    if !content.starts_with("---") {
        return Ok((
            serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
            content.to_string(),
        ));
    }

    let end = content[3..]
        .find("\n---")
        .ok_or_else(|| CustomAgentError::InvalidFrontmatter("Missing closing '---'".to_string()))?;

    let yaml_str = &content[3..end + 3];
    let body = content[end + 7..].trim();

    // Parse YAML with anchor/alias support
    let frontmatter: serde_yaml::Value = serde_yaml::from_str(yaml_str)?;

    // Resolve merge keys (`<<: *alias`) in the parsed YAML (#2199)
    let resolved = resolve_yaml_merge_keys(frontmatter);

    Ok((resolved, body.to_string()))
}

/// Recursively resolve YAML merge keys (`<<: *alias`) in a parsed YAML value.
/// This ensures that anchor-referenced values are properly merged into the parent mapping (#2199).
fn resolve_yaml_merge_keys(value: serde_yaml::Value) -> serde_yaml::Value {
    match value {
        serde_yaml::Value::Mapping(mut map) => {
            // Check for merge key (`<<`)
            let merge_key = serde_yaml::Value::String("<<".to_string());

            if let Some(merge_value) = map.remove(&merge_key) {
                // The merge value can be a single mapping or a sequence of mappings
                let merged_values: Vec<serde_yaml::Value> = match merge_value {
                    serde_yaml::Value::Sequence(seq) => seq,
                    other => vec![other],
                };

                // Create a new mapping with merged values first, then overwrite with local values
                let mut result = serde_yaml::Mapping::new();

                // Apply merged values (in order, later ones take precedence)
                for merge_source in merged_values {
                    if let serde_yaml::Value::Mapping(source_map) = merge_source {
                        for (k, v) in source_map {
                            result.insert(k, v);
                        }
                    }
                }

                // Apply local values (these take precedence over merged values)
                for (k, v) in map {
                    result.insert(k, resolve_yaml_merge_keys(v));
                }

                serde_yaml::Value::Mapping(result)
            } else {
                // No merge key, just recursively process children
                let resolved: serde_yaml::Mapping = map
                    .into_iter()
                    .map(|(k, v)| (k, resolve_yaml_merge_keys(v)))
                    .collect();
                serde_yaml::Value::Mapping(resolved)
            }
        }
        serde_yaml::Value::Sequence(seq) => {
            serde_yaml::Value::Sequence(seq.into_iter().map(resolve_yaml_merge_keys).collect())
        }
        other => other,
    }
}

/// Synchronous version of the loader.
pub mod sync {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::CustomAgentError;
    use crate::custom::CustomAgentConfig;

    /// Load custom agents from a directory synchronously.
    pub fn load_from_dir(dir: &Path) -> Result<Vec<CustomAgentConfig>, CustomAgentError> {
        let mut agents = Vec::new();

        if !dir.exists() {
            return Ok(agents);
        }

        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.extension().is_some_and(|e| e == "md") {
                match load_from_file(&path) {
                    Ok(agent) => agents.push(agent),
                    Err(e) => {
                        tracing::warn!("Failed to load agent from {:?}: {}", path, e);
                    }
                }
            }
        }

        Ok(agents)
    }

    /// Load a custom agent from a file synchronously.
    pub fn load_from_file(path: &Path) -> Result<CustomAgentConfig, CustomAgentError> {
        let content = fs::read_to_string(path)?;

        let (frontmatter, body) = super::parse_frontmatter(&content)?;

        let mut config: CustomAgentConfig = serde_yaml::from_value(frontmatter)?;
        config.prompt = body;

        if config.name.is_empty() {
            config.name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unnamed")
                .to_string();
        }

        Ok(config)
    }

    /// Get default search directories.
    ///
    /// Returns paths in priority order:
    /// 1. `.agents/` (project-local, https://agent.md/ compatible)
    /// 2. `.agent/` (project-local, https://agent.md/ compatible)
    /// 3. `.cortex/agents/` (project-local)
    /// 4. `~/.cortex/agents/` (global personal)
    /// 5. `~/.config/cortex/agents/` (global config)
    pub fn default_search_dirs(project_root: Option<&Path>) -> Vec<PathBuf> {
        let mut dirs = Vec::new();

        // Project-local agents (multiple formats supported)
        if let Some(root) = project_root {
            // Support https://agent.md/ format (.agents/ and .agent/)
            dirs.push(root.join(".agents"));
            dirs.push(root.join(".agent"));
            // Traditional .cortex/agents format
            dirs.push(root.join(".cortex/agents"));
        }

        // Global personal agents from ~/.cortex/agents/
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join(".cortex/agents"));
        }

        // Global agents from config directory
        if let Some(config) = dirs::config_dir() {
            dirs.push(config.join("cortex/agents"));
        }

        dirs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_frontmatter() {
        let content = r#"---
name: test-agent
description: A test agent
---

This is the prompt."#;

        let (fm, body) = parse_frontmatter(content).unwrap();

        assert_eq!(fm["name"].as_str(), Some("test-agent"));
        assert_eq!(fm["description"].as_str(), Some("A test agent"));
        assert_eq!(body, "This is the prompt.");
    }

    #[test]
    fn test_parse_frontmatter_no_yaml() {
        let content = "Just a prompt without frontmatter.";
        let (fm, body) = parse_frontmatter(content).unwrap();

        assert!(fm.as_mapping().unwrap().is_empty());
        assert_eq!(body, content);
    }

    #[test]
    fn test_parse_frontmatter_missing_closing() {
        let content = r#"---
name: test
No closing delimiter"#;

        let result = parse_frontmatter(content);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_loader_with_default_paths() {
        let temp = TempDir::new().unwrap();
        let loader = CustomAgentLoader::with_default_paths(Some(temp.path()));

        let paths = loader.search_paths();
        // Should have at least 3 paths: .agents/, .agent/, .cortex/agents/
        assert!(paths.len() >= 3);
        // First path should be .agents/ (agent.md format)
        assert!(paths[0].ends_with(".agents"));
        // Second path should be .agent/ (agent.md format singular)
        assert!(paths[1].ends_with(".agent"));
        // Third path should be .cortex/agents/ (traditional format)
        assert!(paths[2].ends_with(".cortex/agents"));
    }

    #[tokio::test]
    async fn test_load_agent() {
        let temp = TempDir::new().unwrap();

        std::fs::write(
            temp.path().join("test-agent.md"),
            r#"---
name: test-agent
description: A test agent
model: gpt-4
reasoning_effort: high
tools: read-only
---

You are a helpful test agent."#,
        )
        .unwrap();

        let mut loader = CustomAgentLoader::new();
        loader.add_path(temp.path().to_path_buf());

        let agents = loader.load_all().await.unwrap();

        assert_eq!(agents.len(), 1);
        let agent = &agents[0];
        assert_eq!(agent.name, "test-agent");
        assert_eq!(agent.description, "A test agent");
        assert_eq!(agent.model, "gpt-4");
        assert_eq!(agent.prompt, "You are a helpful test agent.");
    }

    #[tokio::test]
    async fn test_load_agent_name_from_filename() {
        let temp = TempDir::new().unwrap();

        // No name in frontmatter
        std::fs::write(
            temp.path().join("my-agent.md"),
            r#"---
description: An agent
---

Prompt content."#,
        )
        .unwrap();

        let mut loader = CustomAgentLoader::new();
        loader.add_path(temp.path().to_path_buf());

        let agents = loader.load_all().await.unwrap();

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "my-agent");
    }

    #[tokio::test]
    async fn test_agent_precedence() {
        let temp1 = TempDir::new().unwrap();
        let temp2 = TempDir::new().unwrap();

        // Same name in both directories
        std::fs::write(
            temp1.path().join("dupe.md"),
            "---\nname: dupe\ndescription: First\n---\nFirst",
        )
        .unwrap();

        std::fs::write(
            temp2.path().join("dupe.md"),
            "---\nname: dupe\ndescription: Second\n---\nSecond",
        )
        .unwrap();

        let mut loader = CustomAgentLoader::new();
        loader.add_path(temp1.path().to_path_buf());
        loader.add_path(temp2.path().to_path_buf());

        let agents = loader.load_all().await.unwrap();

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].description, "First"); // First takes precedence
    }

    #[test]
    fn test_sync_load() {
        let temp = TempDir::new().unwrap();

        std::fs::write(
            temp.path().join("sync-agent.md"),
            "---\nname: sync-agent\n---\nSync prompt",
        )
        .unwrap();

        let agents = sync::load_from_dir(temp.path()).unwrap();

        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].name, "sync-agent");
    }
}
