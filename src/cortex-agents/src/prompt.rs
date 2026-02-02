//! Agent prompt templates.

use crate::mention::MENTION_SYSTEM_PROMPT;
use crate::AgentInfo;

/// Build the system prompt for an agent.
pub fn build_system_prompt(agent: &AgentInfo, base_prompt: &str) -> String {
    let mut prompt = String::new();

    // Add base prompt
    prompt.push_str(base_prompt);
    prompt.push_str("\n\n");

    // Add agent-specific prompt if provided
    if let Some(ref agent_prompt) = agent.prompt {
        prompt.push_str("## Agent Instructions\n\n");
        prompt.push_str(agent_prompt);
        prompt.push_str("\n\n");
    }

    // Add permission context
    prompt.push_str(&build_permission_context(agent));

    prompt
}

/// Build the system prompt with @mention support for subagents.
pub fn build_system_prompt_with_mentions(agent: &AgentInfo, base_prompt: &str) -> String {
    let mut prompt = build_system_prompt(agent, base_prompt);

    // Add @mention support section
    prompt.push_str(MENTION_SYSTEM_PROMPT);
    prompt.push('\n');

    prompt
}

/// Build permission context for the prompt.
fn build_permission_context(agent: &AgentInfo) -> String {
    let mut context = String::new();

    context.push_str("## Agent Permissions\n\n");
    context.push_str(&format!("Agent: **{}**\n\n", agent.name));

    // Edit permission
    let edit_status = if agent.permission.edit.is_allowed() {
        "ALLOWED"
    } else if agent.permission.edit.is_denied() {
        "DENIED"
    } else {
        "REQUIRES APPROVAL"
    };
    context.push_str(&format!("- File editing: {}\n", edit_status));

    // Web fetch permission
    let webfetch_status = if agent.permission.webfetch.is_allowed() {
        "ALLOWED"
    } else if agent.permission.webfetch.is_denied() {
        "DENIED"
    } else {
        "REQUIRES APPROVAL"
    };
    context.push_str(&format!("- Web fetch: {}\n", webfetch_status));

    // Command execution
    context.push_str("- Command execution: See bash permissions below\n\n");

    // Add guidance based on agent type
    if agent.permission.edit.is_denied() {
        context.push_str("**Note**: This is a read-only agent. Do not attempt to edit files.\n");
        context.push_str("Focus on analysis, exploration, and providing information.\n\n");
    }

    context
}

/// Build tool availability context.
pub fn build_tool_context(agent: &AgentInfo, available_tools: &[&str]) -> String {
    let mut context = String::new();

    context.push_str("## Available Tools\n\n");

    for tool in available_tools {
        let enabled = agent.is_tool_enabled(tool);
        let status = if enabled { "✓" } else { "✗" };
        context.push_str(&format!("- {} {}\n", status, tool));
    }

    context
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PermissionConfig;

    #[test]
    fn test_build_system_prompt() {
        let agent = AgentInfo::new("test").with_permission(PermissionConfig::read_only());

        let prompt = build_system_prompt(&agent, "Base prompt");

        assert!(prompt.contains("Base prompt"));
        assert!(prompt.contains("DENIED"));
        assert!(prompt.contains("read-only"));
    }
}
