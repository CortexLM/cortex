//! System prompt building and AGENTS.md loading.
//!
//! This module provides system prompt construction with support for:
//! - Monolithic prompt mode (full `CORTEX_MAIN_PROMPT`)
//! - Skill-based prompt mode (minimal base + on-demand skills)
//!
//! The skill-based mode reduces token usage by only including instructions
//! relevant to the current task.

use std::path::PathBuf;

use crate::config::Config;

/// System prompt for the Cortex Agent - loaded from cortex-prompt-harness
pub(crate) const SYSTEM_PROMPT: &str = cortex_prompt_harness::prompts::CORTEX_MAIN_PROMPT;

/// Base prompt for skill-based mode - minimal core with skill loading capability
pub(crate) const BASE_PROMPT: &str = cortex_prompt_harness::prompts::CORTEX_BASE_PROMPT;

/// Base prompt for when skills are pre-loaded (no skill loading instructions)
pub(crate) const BASE_PROMPT_WITH_SKILLS: &str =
    cortex_prompt_harness::prompts::CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED;

/// Whether to use skill-based prompts by default.
///
/// When `true`, the system uses a minimal base prompt with skills loaded on-demand.
/// When `false`, the full monolithic prompt is used.
pub const USE_SKILL_BASED_PROMPT: bool = true;

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

// =============================================================================
// Skill-Based Prompt System
// =============================================================================

/// Build a system prompt with specific skills pre-loaded.
///
/// This function constructs a prompt using the minimal base prompt and injects
/// the requested skills. If no skills are specified and `USE_SKILL_BASED_PROMPT`
/// is false, it falls back to the full monolithic prompt.
///
/// # Arguments
///
/// * `config` - The configuration containing environment settings
/// * `skills` - Slice of skill names to include (e.g., `["git", "debugging"]`)
///
/// # Returns
///
/// A complete system prompt with the requested skills injected.
///
/// # Examples
///
/// ```ignore
/// // Build prompt with specific skills
/// let prompt = build_system_prompt_with_skills(&config, &["git", "debugging"]);
///
/// // Build prompt with auto-detected skills
/// let skills = auto_detect_skills_from_message("Fix this bug and create a PR");
/// let prompt = build_system_prompt_with_skills(&config, &skills);
/// ```
pub fn build_system_prompt_with_skills(config: &Config, skills: &[&str]) -> String {
    // If skills mode is disabled and no skills specified, use monolithic prompt
    if !USE_SKILL_BASED_PROMPT && skills.is_empty() {
        return build_system_prompt(config);
    }

    let cwd = config.cwd.display().to_string();
    let user_instructions = config.user_instructions.as_deref().unwrap_or("");

    // Get system info
    let system_info = get_system_info();
    let current_date = chrono::Utc::now().format("%Y-%m-%d").to_string();

    // Build environment context
    let env_context = "# The commands below were executed at the start of all sessions to gather context about the environment.\n\
         # You do not need to repeat them, unless you think the environment has changed.\n\
         # Remember: They are not necessarily related to the current conversation, but may be useful for context.".to_string();

    // Choose base prompt based on whether skills are pre-loaded
    let base = if skills.is_empty() {
        BASE_PROMPT
    } else {
        BASE_PROMPT_WITH_SKILLS
    };

    // Inject skills into the base prompt
    let mut prompt = inject_skills(base, skills);

    // Handle agent-specific prompts
    if let Some(agent_name) = &config.current_agent {
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
                        prompt = agent_prompt;
                    }
                } else {
                    prompt = content;
                }
            }
        } else {
            // Prepend agent name to prompt if no custom prompt found
            prompt = format!("You are the {} agent.\n\n{}", agent_name, prompt);
        }
    }

    // Replace template variables (if present in the prompt)
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

    // If template variable wasn't present, append additional context
    if !additional.is_empty() && !prompt.contains(&additional) {
        prompt.push_str("\n\n");
        prompt.push_str(&additional);
    }

    prompt
}

/// Inject skill content into a base prompt.
///
/// This function retrieves the content for each requested skill and appends
/// it to the base prompt with clear section separators. Invalid or missing
/// skills are silently skipped.
///
/// # Arguments
///
/// * `base_prompt` - The base prompt to build upon
/// * `skills` - Slice of skill names to inject
///
/// # Returns
///
/// The base prompt with skill content appended.
///
/// # Examples
///
/// ```ignore
/// let prompt = inject_skills(BASE_PROMPT, &["git", "debugging"]);
/// assert!(prompt.contains("Git Operations Skill"));
/// assert!(prompt.contains("Debugging Skill"));
/// ```
pub fn inject_skills(base_prompt: &str, skills: &[&str]) -> String {
    if skills.is_empty() {
        return base_prompt.to_string();
    }

    let mut result = base_prompt.to_string();
    let mut injected_skills = Vec::new();

    for skill_name in skills {
        if let Some(skill_content) = cortex_prompt_harness::prompts::get_builtin_skill(skill_name) {
            injected_skills.push((*skill_name, skill_content));
        }
        // Silently skip invalid/missing skills for graceful handling
    }

    if !injected_skills.is_empty() {
        result.push_str("\n\n---\n\n# Loaded Skills\n\n");
        result.push_str("The following skills have been loaded for this task:\n\n");

        for (name, content) in &injected_skills {
            result.push_str(&format!("## Skill: {}\n\n", name));
            // Skip YAML frontmatter if present
            let content_without_frontmatter = strip_yaml_frontmatter(content);
            result.push_str(content_without_frontmatter);
            result.push_str("\n\n---\n\n");
        }
    }

    result
}

/// Strip YAML frontmatter from skill content.
///
/// Skills include YAML frontmatter for metadata, but we don't need it
/// in the injected prompt.
fn strip_yaml_frontmatter(content: &str) -> &str {
    if !content.starts_with("---\n") {
        return content;
    }

    // Find the closing ---
    if let Some(end_pos) = content[4..].find("\n---\n") {
        // Skip past the closing --- and newline
        let skip_to = 4 + end_pos + 5;
        if skip_to < content.len() {
            return &content[skip_to..];
        }
    }

    content
}

/// Auto-detect skills from a user message.
///
/// This function analyzes the user's message and returns a list of skills
/// that are likely relevant to the task. Uses keyword matching from
/// `cortex_prompt_harness::prompts::get_recommended_skills`.
///
/// # Arguments
///
/// * `message` - The user's message to analyze
///
/// # Returns
///
/// A vector of skill names that are relevant to the message.
///
/// # Examples
///
/// ```ignore
/// let skills = auto_detect_skills_from_message("Fix this bug and create a PR");
/// assert!(skills.contains(&"git"));
/// assert!(skills.contains(&"debugging"));
///
/// let skills = auto_detect_skills_from_message("Create a new file with tests");
/// assert!(skills.contains(&"file-operations"));
/// assert!(skills.contains(&"code-quality"));
/// ```
pub fn auto_detect_skills_from_message(message: &str) -> Vec<&'static str> {
    cortex_prompt_harness::prompts::get_recommended_skills(message)
}

/// Get the list of all available built-in skills.
///
/// # Returns
///
/// A slice of all available skill names.
pub fn available_skills() -> &'static [&'static str] {
    cortex_prompt_harness::prompts::AVAILABLE_SKILLS
}

/// Check if a skill name is valid.
///
/// # Arguments
///
/// * `skill` - The skill name to check
///
/// # Returns
///
/// `true` if the skill exists, `false` otherwise.
pub fn is_valid_skill(skill: &str) -> bool {
    cortex_prompt_harness::prompts::is_builtin_skill(skill)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Skill Injection Tests
    // =========================================================================

    #[test]
    fn test_inject_skills_empty() {
        let base = "Base prompt content";
        let result = inject_skills(base, &[]);
        assert_eq!(result, base);
    }

    #[test]
    fn test_inject_skills_single() {
        let base = "Base prompt";
        let result = inject_skills(base, &["git"]);

        assert!(result.starts_with("Base prompt"));
        assert!(result.contains("# Loaded Skills"));
        assert!(result.contains("## Skill: git"));
        assert!(result.contains("Git Operations Skill"));
    }

    #[test]
    fn test_inject_skills_multiple() {
        let base = "Base prompt";
        let result = inject_skills(base, &["git", "debugging"]);

        assert!(result.contains("## Skill: git"));
        assert!(result.contains("## Skill: debugging"));
        assert!(result.contains("Git Operations Skill"));
        assert!(result.contains("Debugging Skill"));
    }

    #[test]
    fn test_inject_skills_invalid_skill_skipped() {
        let base = "Base prompt";
        let result = inject_skills(base, &["git", "nonexistent-skill", "debugging"]);

        assert!(result.contains("## Skill: git"));
        assert!(result.contains("## Skill: debugging"));
        assert!(!result.contains("nonexistent-skill"));
    }

    #[test]
    fn test_inject_skills_all_invalid() {
        let base = "Base prompt";
        let result = inject_skills(base, &["invalid1", "invalid2"]);

        // Should still have base prompt but no skills section
        assert!(result.contains("Base prompt"));
        assert!(!result.contains("# Loaded Skills"));
    }

    // =========================================================================
    // Auto-Detection Tests
    // =========================================================================

    #[test]
    fn test_auto_detect_git_operations() {
        let skills = auto_detect_skills_from_message("Create a PR with these changes");
        assert!(skills.contains(&"git"));

        let skills = auto_detect_skills_from_message("Commit the fix");
        assert!(skills.contains(&"git"));
    }

    #[test]
    fn test_auto_detect_debugging() {
        let skills = auto_detect_skills_from_message("Fix this bug");
        assert!(skills.contains(&"debugging"));

        let skills = auto_detect_skills_from_message("Debug the failing test");
        assert!(skills.contains(&"debugging"));
    }

    #[test]
    fn test_auto_detect_multiple_skills() {
        let skills = auto_detect_skills_from_message("Fix the bug and create a PR");
        assert!(skills.contains(&"git"));
        assert!(skills.contains(&"debugging"));
    }

    #[test]
    fn test_auto_detect_code_quality() {
        let skills = auto_detect_skills_from_message("Run the tests and lint");
        assert!(skills.contains(&"code-quality"));
    }

    #[test]
    fn test_auto_detect_file_operations() {
        let skills = auto_detect_skills_from_message("Create a new file");
        assert!(skills.contains(&"file-operations"));
    }

    #[test]
    fn test_auto_detect_security() {
        let skills = auto_detect_skills_from_message("Handle the API key securely");
        assert!(skills.contains(&"security"));
    }

    #[test]
    fn test_auto_detect_planning() {
        let skills = auto_detect_skills_from_message("Design the new architecture");
        assert!(skills.contains(&"planning"));
    }

    #[test]
    fn test_auto_detect_empty_for_unmatched() {
        let skills = auto_detect_skills_from_message("hello");
        assert!(skills.is_empty());
    }

    // =========================================================================
    // Utility Tests
    // =========================================================================

    #[test]
    fn test_available_skills() {
        let skills = available_skills();
        assert!(skills.contains(&"git"));
        assert!(skills.contains(&"code-quality"));
        assert!(skills.contains(&"file-operations"));
        assert!(skills.contains(&"debugging"));
        assert!(skills.contains(&"security"));
        assert!(skills.contains(&"planning"));
        assert_eq!(skills.len(), 6);
    }

    #[test]
    fn test_is_valid_skill() {
        assert!(is_valid_skill("git"));
        assert!(is_valid_skill("debugging"));
        assert!(!is_valid_skill("nonexistent"));
        assert!(!is_valid_skill(""));
    }

    #[test]
    fn test_strip_yaml_frontmatter() {
        let content = "---\nname: test\n---\n\n# Actual Content";
        let stripped = strip_yaml_frontmatter(content);
        assert_eq!(stripped, "\n# Actual Content");

        let no_frontmatter = "# Just Content";
        assert_eq!(strip_yaml_frontmatter(no_frontmatter), "# Just Content");
    }

    #[test]
    fn test_strip_yaml_frontmatter_no_content_after() {
        let content = "---\nname: test\n---\n";
        let stripped = strip_yaml_frontmatter(content);
        // Should return original if nothing after frontmatter
        assert_eq!(stripped, content);
    }

    // =========================================================================
    // Constant Tests
    // =========================================================================

    #[test]
    fn test_use_skill_based_prompt_default() {
        // Verify the default value
        assert!(USE_SKILL_BASED_PROMPT);
    }

    #[test]
    fn test_base_prompts_exist() {
        assert!(!BASE_PROMPT.is_empty());
        assert!(!BASE_PROMPT_WITH_SKILLS.is_empty());
        assert!(!SYSTEM_PROMPT.is_empty());
    }

    #[test]
    fn test_base_prompt_contains_skill_loading() {
        assert!(BASE_PROMPT.contains("load_skill"));
    }

    #[test]
    fn test_base_prompt_with_skills_no_loading_instructions() {
        assert!(!BASE_PROMPT_WITH_SKILLS.contains("load_skill"));
    }
}
