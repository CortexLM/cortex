//! System prompt building and AGENTS.md loading.

use std::path::PathBuf;

use crate::config::Config;

/// System prompt for the Cortex Agent - loaded from cortex_prompt.txt
pub(crate) const SYSTEM_PROMPT: &str = include_str!("../../../../cortex_prompt.txt");

/// Build the system prompt for the agent.
pub fn build_system_prompt(config: &Config) -> String {
    let cwd = config.cwd.display().to_string();
    let user_instructions = config.user_instructions.as_deref().unwrap_or("");

    // Get system info
    let system_info = get_system_info();
    let current_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Build environment context
    let env_context = "# The commands below were executed at the start of all sessions to gather context about the environment.\n\
         # You do not need to repeat them, unless you think the environment has changed.\n\
         # Remember: They are not necessarily related to the current conversation, but may be useful for context.".to_string();

    // Replace template variables
    let mut prompt = if let Some(agent_name) = &config.current_agent {
        // Try to load the agent to get its custom prompt
        let mut p = format!("You are the {} agent. ", agent_name) + SYSTEM_PROMPT;

        // Try project-level agent first
        let project_agent_path = config
            .cwd
            .join(".cortex")
            .join("agents")
            .join(format!("{}.md", agent_name));
        let user_agent_path = config
            .cortex_home
            .join("agents")
            .join(format!("{}.md", agent_name));

        let path_to_try = if project_agent_path.exists() {
            Some(project_agent_path)
        } else if user_agent_path.exists() {
            Some(user_agent_path)
        } else {
            None
        };

        if let Some(path) = path_to_try {
            if let Ok(content) = std::fs::read_to_string(path) {
                // If it starts with frontmatter, try to parse it
                if content.starts_with("---") {
                    if let Ok((_meta, agent_prompt)) = crate::agents::parse_agent_md(&content) {
                        p = agent_prompt;
                    }
                } else {
                    p = content;
                }
            }
        }
        p
    } else {
        SYSTEM_PROMPT.to_string()
    };

    prompt = prompt.replace("{{SYSTEM_INFO}}", &system_info);
    prompt = prompt.replace("{{MODEL_NAME}}", &config.model);
    prompt = prompt.replace("{{CURRENT_DATE}}", &current_date);
    prompt = prompt.replace("{{CWD}}", &cwd);
    prompt = prompt.replace("{{ENVIRONMENT_CONTEXT}}", &env_context);

    // Load AGENTS.md instructions
    let agents_instructions = load_agents_md(config);

    // Additional context (user instructions + AGENTS.md)
    let mut additional = String::new();

    if !agents_instructions.is_empty() {
        additional.push_str("## Project Instructions (from AGENTS.md)\n");
        additional.push_str(&agents_instructions);
        additional.push_str("\n\n");
    }

    if !user_instructions.is_empty() {
        additional.push_str("## User Instructions\n");
        additional.push_str(user_instructions);
        additional.push('\n');
    }

    prompt = prompt.replace("{{ADDITIONAL_CONTEXT}}", &additional);

    prompt
}

/// Load and merge AGENTS.md files.
/// Order: ~/.cortex/AGENTS.md -> repo root -> directories down to CWD
/// AGENTS.override.md replaces instead of merging.
fn load_agents_md(config: &Config) -> String {
    let mut instructions = Vec::new();

    // 1. Global AGENTS.md from ~/.cortex/
    let global_path = config.cortex_home.join("AGENTS.md");
    if let Ok(content) = std::fs::read_to_string(&global_path) {
        instructions.push(content);
    }

    // 2. Find git root or use cwd
    let repo_root = find_git_root(&config.cwd).unwrap_or_else(|| config.cwd.clone());

    // 3. Walk from repo root to cwd, collecting AGENTS.md files
    let _current = repo_root.clone();
    let cwd = &config.cwd;

    // Collect all directories from root to cwd
    let mut dirs_to_check = vec![repo_root.clone()];
    if let Ok(relative) = cwd.strip_prefix(&repo_root) {
        let mut path = repo_root.clone();
        for component in relative.components() {
            path = path.join(component);
            dirs_to_check.push(path.clone());
        }
    }

    for dir in dirs_to_check {
        // Check for AGENTS.override.md first (replaces all previous)
        let override_path = dir.join("AGENTS.override.md");
        if let Ok(content) = std::fs::read_to_string(&override_path) {
            instructions.clear();
            instructions.push(content);
            continue;
        }

        // Regular AGENTS.md (merges)
        let agents_path = dir.join("AGENTS.md");
        if let Ok(content) = std::fs::read_to_string(&agents_path) {
            instructions.push(content);
        }
    }

    instructions.join("\n\n---\n\n")
}

/// Find git repository root.
pub(crate) fn find_git_root(start: &PathBuf) -> Option<PathBuf> {
    let mut current = start.clone();
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Get system information string.
fn get_system_info() -> String {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;

    #[cfg(target_os = "linux")]
    let kernel = std::process::Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    #[cfg(not(target_os = "linux"))]
    let kernel = String::new();

    if kernel.is_empty() {
        format!("{os} {arch}")
    } else {
        format!("{os} {arch} ({kernel})")
    }
}
