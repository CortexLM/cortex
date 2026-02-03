//! Minimal base agent prompt with dynamic skill loading.
//!
//! This prompt provides the core agent identity and skill loading mechanism.
//! Detailed instructions for specific operations are loaded dynamically via skills.
//!
//! # Philosophy
//!
//! The base agent prompt is intentionally minimal to:
//! - Reduce token usage when full instructions aren't needed
//! - Enable dynamic loading of only relevant capabilities
//! - Keep the agent focused on the core task
//!
//! # Usage
//!
//! ```rust
//! use cortex_prompt_harness::prompts::base_agent::{
//!     CORTEX_BASE_PROMPT,
//!     get_recommended_skills,
//!     format_skill_loading_prompt,
//! };
//!
//! // Get recommended skills for a task
//! let skills = get_recommended_skills("Create a PR with these changes");
//! // skills = ["git"]
//!
//! // Format the skill loading call
//! let loading_call = format_skill_loading_prompt(&skills);
//! // loading_call = "load_skill([\"git\"])"
//! ```

/// The minimal base prompt for Cortex agent with skill loading.
///
/// This prompt provides:
/// - Core agent identity
/// - Autonomy level and execution mode
/// - Skill loading mechanism
/// - Essential rules that always apply
/// - Output format expectations
///
/// Detailed instructions for specific operations (git, debugging, code quality, etc.)
/// are loaded dynamically via the skill system.
pub const CORTEX_BASE_PROMPT: &str = r#"# CORTEX

You are **Cortex**, an autonomous software engineering intelligence.

## Operating Mode

```
AUTONOMY LEVEL: FULL
INTERACTION MODE: ASYNC
VERIFICATION: MANDATORY
```

- Execute tasks completely without human intervention
- Verify all changes before reporting completion
- Match exactly what was requested

## Skill System

At the start of each task, analyze what capabilities you need and load relevant skills:

```
load_skill([skill1, skill2, ...])
```

### Available Skills

| Skill | Description | When to Load |
|-------|-------------|--------------|
| `git` | Version control operations | Git commits, PRs, branches, merges |
| `code-quality` | Code standards and testing | Writing/reviewing code, linting, tests |
| `file-operations` | File handling best practices | Creating/editing/moving files |
| `debugging` | Failure protocol and error handling | Encountering errors, troubleshooting |
| `security` | Security rules and secrets handling | Handling sensitive data, auth, keys |
| `planning` | Task decomposition and cognitive phases | Complex multi-step tasks |

### Skill Loading Examples

- "Create a PR" → `load_skill(["git"])`
- "Fix this bug" → `load_skill(["debugging", "code-quality"])`
- "Add new feature" → `load_skill(["planning", "code-quality", "file-operations"])`
- "Review code security" → `load_skill(["security", "code-quality"])`
- "Refactor this module" → `load_skill(["code-quality", "file-operations"])`

## Essential Rules

1. **Complete entirely** - Finish the task before stopping
2. **Verify all changes** - Ensure code compiles and works as expected
3. **Never break the codebase** - Rollback if something goes wrong
4. **Stay focused** - Do exactly what was requested, no extras
5. **Research before asking** - Investigate until you understand

## Output

When done, provide a brief summary (1-4 sentences) of what was accomplished.
Any caveats or follow-up items if relevant. No excessive detail.
"#;

/// Alternative prompt for when skills are pre-loaded.
///
/// This prompt assumes skills have already been injected into the context,
/// so it omits the skill loading instructions.
pub const CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED: &str = r#"# CORTEX

You are **Cortex**, an autonomous software engineering intelligence.

## Operating Mode

```
AUTONOMY LEVEL: FULL
INTERACTION MODE: ASYNC
VERIFICATION: MANDATORY
```

- Execute tasks completely without human intervention
- Verify all changes before reporting completion
- Match exactly what was requested

## Essential Rules

1. **Complete entirely** - Finish the task before stopping
2. **Verify all changes** - Ensure code compiles and works as expected
3. **Never break the codebase** - Rollback if something goes wrong
4. **Stay focused** - Do exactly what was requested, no extras
5. **Research before asking** - Investigate until you understand

## Output

When done, provide a brief summary (1-4 sentences) of what was accomplished.
Any caveats or follow-up items if relevant. No excessive detail.
"#;

/// All available built-in skills.
pub const AVAILABLE_SKILLS: &[&str] = &[
    "git",
    "code-quality",
    "file-operations",
    "debugging",
    "security",
    "planning",
];

/// Skill metadata for display and recommendation.
#[derive(Debug, Clone)]
pub struct SkillInfo {
    /// Skill identifier.
    pub name: &'static str,
    /// Brief description of the skill.
    pub description: &'static str,
    /// Keywords that trigger this skill recommendation.
    pub keywords: &'static [&'static str],
}

/// Metadata for all available skills.
pub const SKILL_METADATA: &[SkillInfo] = &[
    SkillInfo {
        name: "git",
        description: "Version control operations",
        keywords: &[
            "git",
            "commit",
            "push",
            "pull",
            "merge",
            "branch",
            "pr",
            "pull request",
            "rebase",
            "cherry-pick",
            "checkout",
            "stash",
            "diff",
            "log",
            "blame",
        ],
    },
    SkillInfo {
        name: "code-quality",
        description: "Code standards and testing",
        keywords: &[
            "lint",
            "test",
            "format",
            "style",
            "convention",
            "review",
            "refactor",
            "clean",
            "quality",
            "coverage",
            "eslint",
            "pylint",
            "clippy",
            "prettier",
            "jest",
            "pytest",
            "cargo test",
        ],
    },
    SkillInfo {
        name: "file-operations",
        description: "File handling best practices",
        keywords: &[
            "create",
            "file",
            "write",
            "edit",
            "move",
            "rename",
            "delete",
            "copy",
            "directory",
            "folder",
            "path",
            "backup",
        ],
    },
    SkillInfo {
        name: "debugging",
        description: "Failure protocol and error handling",
        keywords: &[
            "debug",
            "error",
            "fix",
            "bug",
            "crash",
            "exception",
            "trace",
            "stack",
            "breakpoint",
            "investigate",
            "troubleshoot",
            "diagnose",
            "failing",
            "broken",
        ],
    },
    SkillInfo {
        name: "security",
        description: "Security rules and secrets handling",
        keywords: &[
            "security",
            "secret",
            "key",
            "token",
            "password",
            "credential",
            "auth",
            "authentication",
            "authorization",
            "encrypt",
            "hash",
            "vulnerability",
            "audit",
            "sensitive",
            "env",
            "environment variable",
        ],
    },
    SkillInfo {
        name: "planning",
        description: "Task decomposition and cognitive phases",
        keywords: &[
            "plan",
            "design",
            "architect",
            "complex",
            "multi-step",
            "breakdown",
            "decompose",
            "strategy",
            "roadmap",
            "milestone",
            "phase",
            "implement feature",
        ],
    },
];

/// Get recommended skills based on task keywords.
///
/// This function analyzes the task description and returns a list of
/// recommended skills based on keyword matching.
///
/// # Arguments
///
/// * `task` - The task description to analyze
///
/// # Returns
///
/// A vector of skill names that are relevant to the task.
///
/// # Examples
///
/// ```rust
/// use cortex_prompt_harness::prompts::base_agent::get_recommended_skills;
///
/// let skills = get_recommended_skills("Create a PR with bug fixes");
/// assert!(skills.contains(&"git"));
/// assert!(skills.contains(&"debugging"));
///
/// let skills = get_recommended_skills("Design and implement a new feature with tests");
/// assert!(skills.contains(&"code-quality"));
/// assert!(skills.contains(&"planning"));
/// ```
#[must_use]
pub fn get_recommended_skills(task: &str) -> Vec<&'static str> {
    let task_lower = task.to_lowercase();
    let mut recommended: Vec<&'static str> = Vec::new();

    for skill in SKILL_METADATA {
        for keyword in skill.keywords {
            if task_lower.contains(keyword) {
                if !recommended.contains(&skill.name) {
                    recommended.push(skill.name);
                }
                break;
            }
        }
    }

    // Default to planning for complex-sounding tasks with no specific matches
    if recommended.is_empty() && task.len() > 100 {
        recommended.push("planning");
    }

    recommended
}

/// Format a skill loading prompt call.
///
/// # Arguments
///
/// * `skills` - The list of skills to load
///
/// # Returns
///
/// A formatted string representing the skill loading call.
///
/// # Examples
///
/// ```rust
/// use cortex_prompt_harness::prompts::base_agent::format_skill_loading_prompt;
///
/// let call = format_skill_loading_prompt(&["git"]);
/// assert_eq!(call, "load_skill([\"git\"])");
///
/// let call = format_skill_loading_prompt(&["debugging", "code-quality"]);
/// assert_eq!(call, "load_skill([\"debugging\", \"code-quality\"])");
/// ```
#[must_use]
pub fn format_skill_loading_prompt(skills: &[&str]) -> String {
    if skills.is_empty() {
        return String::from("load_skill([])");
    }

    let quoted: Vec<String> = skills.iter().map(|s| format!("\"{}\"", s)).collect();
    format!("load_skill([{}])", quoted.join(", "))
}

/// Check if a skill name is valid.
///
/// # Arguments
///
/// * `skill` - The skill name to validate
///
/// # Returns
///
/// `true` if the skill is a valid built-in skill, `false` otherwise.
#[must_use]
pub fn is_valid_skill(skill: &str) -> bool {
    AVAILABLE_SKILLS.contains(&skill)
}

/// Get the description for a skill.
///
/// # Arguments
///
/// * `skill` - The skill name
///
/// # Returns
///
/// The skill description if found, `None` otherwise.
#[must_use]
pub fn get_skill_description(skill: &str) -> Option<&'static str> {
    SKILL_METADATA
        .iter()
        .find(|s| s.name == skill)
        .map(|s| s.description)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // Prompt Content Tests
    // =========================================================================

    #[test]
    fn test_base_prompt_contains_essential_sections() {
        assert!(CORTEX_BASE_PROMPT.contains("# CORTEX"));
        assert!(CORTEX_BASE_PROMPT.contains("Operating Mode"));
        assert!(CORTEX_BASE_PROMPT.contains("Skill System"));
        assert!(CORTEX_BASE_PROMPT.contains("Available Skills"));
        assert!(CORTEX_BASE_PROMPT.contains("Essential Rules"));
        assert!(CORTEX_BASE_PROMPT.contains("Output"));
    }

    #[test]
    fn test_base_prompt_contains_all_skills() {
        for skill in AVAILABLE_SKILLS {
            assert!(
                CORTEX_BASE_PROMPT.contains(&format!("`{}`", skill)),
                "Base prompt should contain skill: {}",
                skill
            );
        }
    }

    #[test]
    fn test_base_prompt_contains_load_skill_syntax() {
        assert!(CORTEX_BASE_PROMPT.contains("load_skill(["));
    }

    #[test]
    fn test_base_prompt_contains_autonomy_level() {
        assert!(CORTEX_BASE_PROMPT.contains("AUTONOMY LEVEL: FULL"));
    }

    #[test]
    fn test_preloaded_prompt_no_skill_instructions() {
        assert!(!CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED.contains("load_skill"));
        assert!(!CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED.contains("Available Skills"));
    }

    #[test]
    fn test_preloaded_prompt_contains_essentials() {
        assert!(CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED.contains("# CORTEX"));
        assert!(CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED.contains("Operating Mode"));
        assert!(CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED.contains("Essential Rules"));
        assert!(CORTEX_BASE_PROMPT_WITH_SKILLS_PRELOADED.contains("Output"));
    }

    // =========================================================================
    // Skill Recommendation Tests
    // =========================================================================

    #[test]
    fn test_get_recommended_skills_git() {
        let skills = get_recommended_skills("Create a PR with these changes");
        assert!(skills.contains(&"git"));

        let skills = get_recommended_skills("Commit the fix");
        assert!(skills.contains(&"git"));

        let skills = get_recommended_skills("merge the feature branch");
        assert!(skills.contains(&"git"));
    }

    #[test]
    fn test_get_recommended_skills_debugging() {
        let skills = get_recommended_skills("Fix this bug");
        assert!(skills.contains(&"debugging"));

        let skills = get_recommended_skills("Debug the failing test");
        assert!(skills.contains(&"debugging"));

        let skills = get_recommended_skills("Investigate the error");
        assert!(skills.contains(&"debugging"));
    }

    #[test]
    fn test_get_recommended_skills_code_quality() {
        let skills = get_recommended_skills("Run the tests");
        assert!(skills.contains(&"code-quality"));

        let skills = get_recommended_skills("lint the code");
        assert!(skills.contains(&"code-quality"));

        let skills = get_recommended_skills("Refactor this function");
        assert!(skills.contains(&"code-quality"));
    }

    #[test]
    fn test_get_recommended_skills_file_operations() {
        let skills = get_recommended_skills("Create a new file");
        assert!(skills.contains(&"file-operations"));

        let skills = get_recommended_skills("Move the config to a different directory");
        assert!(skills.contains(&"file-operations"));
    }

    #[test]
    fn test_get_recommended_skills_security() {
        let skills = get_recommended_skills("Handle the API key securely");
        assert!(skills.contains(&"security"));

        let skills = get_recommended_skills("Add authentication");
        assert!(skills.contains(&"security"));
    }

    #[test]
    fn test_get_recommended_skills_planning() {
        let skills = get_recommended_skills("Design the new architecture");
        assert!(skills.contains(&"planning"));

        let skills = get_recommended_skills("implement feature with multiple components");
        assert!(skills.contains(&"planning"));
    }

    #[test]
    fn test_get_recommended_skills_multiple() {
        let skills = get_recommended_skills("Fix the bug and create a PR");
        assert!(skills.contains(&"debugging"));
        assert!(skills.contains(&"git"));

        let skills = get_recommended_skills("Create a new file with tests");
        assert!(skills.contains(&"file-operations"));
        assert!(skills.contains(&"code-quality"));
    }

    #[test]
    fn test_get_recommended_skills_case_insensitive() {
        let skills = get_recommended_skills("CREATE A PR");
        assert!(skills.contains(&"git"));

        let skills = get_recommended_skills("FIX THE BUG");
        assert!(skills.contains(&"debugging"));
    }

    #[test]
    fn test_get_recommended_skills_empty_for_unmatched() {
        let skills = get_recommended_skills("hello");
        assert!(skills.is_empty());
    }

    #[test]
    fn test_get_recommended_skills_planning_for_long_task() {
        // Tasks over 100 characters with no matches should get planning
        let long_task = "This is a very long task description that doesn't contain any specific keywords but is clearly complex enough to require some form of organized approach to complete successfully";
        let skills = get_recommended_skills(long_task);
        assert!(skills.contains(&"planning"));
    }

    // =========================================================================
    // Format Skill Loading Tests
    // =========================================================================

    #[test]
    fn test_format_skill_loading_prompt_single() {
        let result = format_skill_loading_prompt(&["git"]);
        assert_eq!(result, "load_skill([\"git\"])");
    }

    #[test]
    fn test_format_skill_loading_prompt_multiple() {
        let result = format_skill_loading_prompt(&["debugging", "code-quality"]);
        assert_eq!(result, "load_skill([\"debugging\", \"code-quality\"])");
    }

    #[test]
    fn test_format_skill_loading_prompt_empty() {
        let result = format_skill_loading_prompt(&[]);
        assert_eq!(result, "load_skill([])");
    }

    #[test]
    fn test_format_skill_loading_prompt_all_skills() {
        let result = format_skill_loading_prompt(AVAILABLE_SKILLS);
        assert!(result.starts_with("load_skill(["));
        assert!(result.ends_with("])"));
        for skill in AVAILABLE_SKILLS {
            assert!(result.contains(&format!("\"{}\"", skill)));
        }
    }

    // =========================================================================
    // Validation Tests
    // =========================================================================

    #[test]
    fn test_is_valid_skill() {
        assert!(is_valid_skill("git"));
        assert!(is_valid_skill("code-quality"));
        assert!(is_valid_skill("file-operations"));
        assert!(is_valid_skill("debugging"));
        assert!(is_valid_skill("security"));
        assert!(is_valid_skill("planning"));
    }

    #[test]
    fn test_is_valid_skill_invalid() {
        assert!(!is_valid_skill("invalid-skill"));
        assert!(!is_valid_skill(""));
        assert!(!is_valid_skill("GIT")); // case-sensitive
    }

    #[test]
    fn test_get_skill_description() {
        assert_eq!(
            get_skill_description("git"),
            Some("Version control operations")
        );
        assert_eq!(
            get_skill_description("debugging"),
            Some("Failure protocol and error handling")
        );
    }

    #[test]
    fn test_get_skill_description_invalid() {
        assert_eq!(get_skill_description("invalid"), None);
    }

    // =========================================================================
    // Skill Metadata Tests
    // =========================================================================

    #[test]
    fn test_available_skills_count() {
        assert_eq!(AVAILABLE_SKILLS.len(), 6);
    }

    #[test]
    fn test_skill_metadata_matches_available() {
        assert_eq!(SKILL_METADATA.len(), AVAILABLE_SKILLS.len());
        for skill in AVAILABLE_SKILLS {
            assert!(
                SKILL_METADATA.iter().any(|s| s.name == *skill),
                "Skill metadata missing for: {}",
                skill
            );
        }
    }

    #[test]
    fn test_skill_metadata_has_keywords() {
        for skill in SKILL_METADATA {
            assert!(
                !skill.keywords.is_empty(),
                "Skill {} has no keywords",
                skill.name
            );
        }
    }
}
