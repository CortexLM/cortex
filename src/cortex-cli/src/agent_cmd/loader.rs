//! Agent loading and parsing functionality.
//!
//! Contains functions for loading agents from various sources.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::prompts::{ARCHITECT_PROMPT, CODE_EXPLORER_PROMPT, CODE_REVIEWER_PROMPT};
use super::types::{AgentFrontmatter, AgentInfo, AgentMode, AgentSource};

/// Get the agents directory path.
pub fn get_agents_dir() -> Result<PathBuf> {
    let cortex_home = dirs::home_dir()
        .map(|h| h.join(".cortex"))
        .ok_or_else(|| anyhow::anyhow!("Could not determine home directory"))?;
    Ok(cortex_home.join("agents"))
}

/// Get all project agents directories.
///
/// Returns directories in priority order:
/// 1. `.agents/` (https://agent.md/ compatible)
/// 2. `.agent/` (https://agent.md/ compatible)
/// 3. `.cortex/agents/` (traditional format)
pub fn get_project_agents_dirs() -> Vec<PathBuf> {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let candidates = [
        cwd.join(".agents"),                // agent.md format
        cwd.join(".agent"),                 // agent.md format (singular)
        cwd.join(".cortex").join("agents"), // traditional format
    ];

    candidates.into_iter().filter(|p| p.exists()).collect()
}

/// Load all agents from various sources.
pub fn load_all_agents() -> Result<Vec<AgentInfo>> {
    let mut agents = Vec::new();
    let mut seen_names = std::collections::HashSet::new();

    // Load built-in agents first
    for agent in load_builtin_agents() {
        seen_names.insert(agent.name.clone());
        agents.push(agent);
    }

    // Load project agents from multiple directories (.agents/, .agent/, .cortex/agents/)
    // Project agents take precedence over personal agents
    for project_dir in get_project_agents_dirs() {
        if let Ok(project_agents) = load_agents_from_dir(&project_dir, AgentSource::Project) {
            for agent in project_agents {
                if !seen_names.contains(&agent.name) {
                    seen_names.insert(agent.name.clone());
                    agents.push(agent);
                }
            }
        }
    }

    // Load personal agents from ~/.cortex/agents/
    let personal_dir = get_agents_dir()?;
    if personal_dir.exists()
        && let Ok(personal_agents) = load_agents_from_dir(&personal_dir, AgentSource::Personal)
    {
        for agent in personal_agents {
            if !seen_names.contains(&agent.name) {
                seen_names.insert(agent.name.clone());
                agents.push(agent);
            }
        }
    }

    Ok(agents)
}

/// Load built-in agents.
pub fn load_builtin_agents() -> Vec<AgentInfo> {
    vec![
        AgentInfo {
            name: "build".to_string(),
            display_name: Some("Build".to_string()),
            description: Some("Full access agent for development work".to_string()),
            mode: AgentMode::Primary,
            native: true,
            hidden: false,
            prompt: None,
            temperature: None,
            top_p: None,
            color: Some("#22c55e".to_string()),
            model: None,
            tools: HashMap::new(),
            allowed_tools: None,
            denied_tools: Vec::new(),
            max_turns: None,
            can_delegate: true,
            tags: vec!["development".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
        AgentInfo {
            name: "plan".to_string(),
            display_name: Some("Plan".to_string()),
            description: Some("Read-only agent for analysis and code exploration".to_string()),
            mode: AgentMode::Primary,
            native: true,
            hidden: false,
            prompt: None,
            temperature: None,
            top_p: None,
            color: Some("#3b82f6".to_string()),
            model: None,
            tools: HashMap::new(),
            allowed_tools: Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
                "LS".to_string(),
            ]),
            denied_tools: vec!["Execute".to_string(), "Create".to_string(), "Edit".to_string()],
            max_turns: None,
            can_delegate: false,
            tags: vec!["analysis".to_string(), "read-only".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
        AgentInfo {
            name: "explore".to_string(),
            display_name: Some("Explore".to_string()),
            description: Some("Fast agent specialized for exploring codebases".to_string()),
            mode: AgentMode::Subagent,
            native: true,
            hidden: false,
            prompt: Some("You are a fast, focused agent specialized in exploring codebases. Your goal is to quickly find relevant information.".to_string()),
            temperature: Some(0.3),
            top_p: None,
            color: Some("#f59e0b".to_string()),
            model: None,
            tools: [
                ("edit".to_string(), false),
                ("write".to_string(), false),
                ("todoread".to_string(), false),
                ("todowrite".to_string(), false),
            ].into_iter().collect(),
            allowed_tools: Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
                "LS".to_string(),
            ]),
            denied_tools: Vec::new(),
            max_turns: Some(10),
            can_delegate: false,
            tags: vec!["code".to_string(), "analysis".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
        AgentInfo {
            name: "general".to_string(),
            display_name: Some("General".to_string()),
            description: Some("General-purpose agent for researching complex questions and executing multi-step tasks".to_string()),
            mode: AgentMode::Subagent,
            native: true,
            hidden: true,
            prompt: None,
            temperature: None,
            top_p: None,
            color: Some("#8b5cf6".to_string()),
            model: None,
            tools: [
                ("todoread".to_string(), false),
                ("todowrite".to_string(), false),
            ].into_iter().collect(),
            allowed_tools: None,
            denied_tools: Vec::new(),
            max_turns: None,
            can_delegate: true,
            tags: vec!["general".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
        AgentInfo {
            name: "code-explorer".to_string(),
            display_name: Some("Code Explorer".to_string()),
            description: Some("Explore and understand codebases. Use for analyzing code structure, finding patterns, and understanding implementations.".to_string()),
            mode: AgentMode::Subagent,
            native: true,
            hidden: false,
            prompt: Some(CODE_EXPLORER_PROMPT.to_string()),
            temperature: Some(0.3),
            top_p: None,
            color: Some("#06b6d4".to_string()),
            model: None,
            tools: HashMap::new(),
            allowed_tools: Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
                "LS".to_string(),
            ]),
            denied_tools: Vec::new(),
            max_turns: Some(10),
            can_delegate: false,
            tags: vec!["code".to_string(), "analysis".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
        AgentInfo {
            name: "code-reviewer".to_string(),
            display_name: Some("Code Reviewer".to_string()),
            description: Some("Review code for quality, bugs, and best practices".to_string()),
            mode: AgentMode::Subagent,
            native: true,
            hidden: false,
            prompt: Some(CODE_REVIEWER_PROMPT.to_string()),
            temperature: Some(0.2),
            top_p: None,
            color: Some("#ef4444".to_string()),
            model: None,
            tools: HashMap::new(),
            allowed_tools: Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
            ]),
            denied_tools: vec!["Execute".to_string()],
            max_turns: Some(5),
            can_delegate: false,
            tags: vec!["review".to_string(), "quality".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
        AgentInfo {
            name: "architect".to_string(),
            display_name: Some("Architect".to_string()),
            description: Some("Design software architecture and make high-level technical decisions".to_string()),
            mode: AgentMode::Subagent,
            native: true,
            hidden: false,
            prompt: Some(ARCHITECT_PROMPT.to_string()),
            temperature: Some(0.5),
            top_p: None,
            color: Some("#a855f7".to_string()),
            model: None,
            tools: HashMap::new(),
            allowed_tools: Some(vec![
                "Read".to_string(),
                "Grep".to_string(),
                "Glob".to_string(),
                "LS".to_string(),
            ]),
            denied_tools: vec!["Execute".to_string()],
            max_turns: Some(15),
            can_delegate: true,
            tags: vec!["architecture".to_string(), "design".to_string()],
            source: AgentSource::Builtin,
            path: None,
        },
    ]
}

/// Load agents from a directory.
pub fn load_agents_from_dir(dir: &Path, source: AgentSource) -> Result<Vec<AgentInfo>> {
    let mut agents = Vec::new();

    if !dir.exists() {
        return Ok(agents);
    }

    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        // Check for .md files directly in the agents directory
        if path.is_file() && path.extension().map(|e| e == "md").unwrap_or(false) {
            match load_agent_from_md(&path, source) {
                Ok(agent) => agents.push(agent),
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to load agent from {}: {}",
                        path.display(),
                        e
                    );
                }
            }
        }

        // Check for directories with AGENT.md or agent.json
        if path.is_dir() {
            let agent_md = path.join("AGENT.md");
            let agent_json = path.join("agent.json");

            if agent_md.exists() {
                match load_agent_from_md(&agent_md, source) {
                    Ok(mut agent) => {
                        agent.path = Some(path.clone());
                        agents.push(agent);
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to load agent from {}: {}",
                            agent_md.display(),
                            e
                        );
                    }
                }
            } else if agent_json.exists() {
                match load_agent_from_json(&agent_json, source) {
                    Ok(mut agent) => {
                        agent.path = Some(path.clone());
                        agents.push(agent);
                    }
                    Err(e) => {
                        eprintln!(
                            "Warning: Failed to load agent from {}: {}",
                            agent_json.display(),
                            e
                        );
                    }
                }
            }
        }
    }

    Ok(agents)
}

/// Normalize line endings by converting CRLF to LF.
/// This handles files with mixed line endings (common when editing on different OSes).
fn normalize_line_endings(content: String) -> String {
    // Replace all CRLF with LF to handle mixed line endings
    content.replace("\r\n", "\n").replace('\r', "\n")
}

/// Read a file with automatic encoding detection (UTF-8 and UTF-16).
/// Also normalizes line endings (CRLF -> LF) to handle mixed line endings.
pub fn read_file_with_encoding(path: &Path) -> Result<String> {
    let bytes =
        std::fs::read(path).with_context(|| format!("Failed to read {}", path.display()))?;

    // Check for UTF-16 BOM and convert if needed
    let content = if bytes.starts_with(&[0xFF, 0xFE]) {
        // UTF-16 LE BOM
        let u16_chars: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&u16_chars)
            .with_context(|| format!("Invalid UTF-16 LE content in {}", path.display()))?
    } else if bytes.starts_with(&[0xFE, 0xFF]) {
        // UTF-16 BE BOM
        let u16_chars: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|chunk| u16::from_be_bytes([chunk[0], chunk[1]]))
            .collect();
        String::from_utf16(&u16_chars)
            .with_context(|| format!("Invalid UTF-16 BE content in {}", path.display()))?
    } else if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        // UTF-8 BOM - skip it
        String::from_utf8(bytes[3..].to_vec())
            .with_context(|| format!("Invalid UTF-8 content in {}", path.display()))?
    } else {
        // Assume UTF-8
        String::from_utf8(bytes)
            .with_context(|| format!("Invalid UTF-8 content in {}", path.display()))?
    };

    // Normalize line endings to handle mixed CRLF/LF
    Ok(normalize_line_endings(content))
}

/// Load an agent from a markdown file with YAML frontmatter.
pub fn load_agent_from_md(path: &Path, source: AgentSource) -> Result<AgentInfo> {
    let content = read_file_with_encoding(path)?;

    let (frontmatter, body) = parse_frontmatter(&content)?;

    Ok(AgentInfo {
        name: frontmatter.name,
        display_name: frontmatter.display_name,
        description: frontmatter.description,
        mode: frontmatter.mode,
        native: false,
        hidden: frontmatter.hidden,
        prompt: if body.is_empty() { None } else { Some(body) },
        temperature: frontmatter.temperature,
        top_p: frontmatter.top_p,
        color: frontmatter.color,
        model: frontmatter.model,
        tools: frontmatter.tools,
        allowed_tools: frontmatter.allowed_tools,
        denied_tools: frontmatter.denied_tools,
        max_turns: frontmatter.max_turns,
        can_delegate: frontmatter.can_delegate,
        tags: frontmatter.tags,
        source,
        path: Some(path.to_path_buf()),
    })
}

/// Load an agent from a JSON file.
pub fn load_agent_from_json(path: &Path, source: AgentSource) -> Result<AgentInfo> {
    let content = read_file_with_encoding(path)?;

    let frontmatter: AgentFrontmatter = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse {}", path.display()))?;

    // Check for a separate prompt file
    let prompt = if let Some(parent) = path.parent() {
        let prompt_file = parent.join("prompt.md");
        if prompt_file.exists() {
            Some(std::fs::read_to_string(&prompt_file)?)
        } else {
            None
        }
    } else {
        None
    };

    Ok(AgentInfo {
        name: frontmatter.name,
        display_name: frontmatter.display_name,
        description: frontmatter.description,
        mode: frontmatter.mode,
        native: false,
        hidden: frontmatter.hidden,
        prompt,
        temperature: frontmatter.temperature,
        top_p: frontmatter.top_p,
        color: frontmatter.color,
        model: frontmatter.model,
        tools: frontmatter.tools,
        allowed_tools: frontmatter.allowed_tools,
        denied_tools: frontmatter.denied_tools,
        max_turns: frontmatter.max_turns,
        can_delegate: frontmatter.can_delegate,
        tags: frontmatter.tags,
        source,
        path: Some(path.to_path_buf()),
    })
}

/// Parse YAML frontmatter from markdown content.
pub fn parse_frontmatter(content: &str) -> Result<(AgentFrontmatter, String)> {
    let content = content.trim();

    if !content.starts_with("---") {
        anyhow::bail!("File must start with YAML frontmatter (---)")
    }

    let rest = &content[3..];
    let end_idx = rest
        .find("\n---")
        .ok_or_else(|| anyhow::anyhow!("Missing closing --- for YAML frontmatter"))?;

    let yaml_content = &rest[..end_idx].trim();
    let markdown_content = rest[end_idx + 4..].trim();

    let frontmatter: AgentFrontmatter =
        serde_yaml::from_str(yaml_content).with_context(|| "Invalid YAML in frontmatter")?;

    Ok((frontmatter, markdown_content.to_string()))
}
