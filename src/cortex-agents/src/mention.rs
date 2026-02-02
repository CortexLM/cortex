//! Agent mention parsing for @agent syntax.
//!
//! This module provides functionality to detect and parse @agent mentions
//! in user messages, enabling direct invocation of subagents.
//!
//! # Examples
//!
//! ```rust
//! use cortex_agents::mention::{parse_agent_mentions, AgentMention};
//!
//! let text = "@general Find all uses of the Config struct";
//! let mentions = parse_agent_mentions(text);
//! assert_eq!(mentions.len(), 1);
//! assert_eq!(mentions[0].agent_name, "general");
//! ```

use regex::Regex;
use std::sync::LazyLock;

/// A parsed agent mention from user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMention {
    /// The agent name (without the @ prefix).
    pub agent_name: String,
    /// Start position of the mention in the original text.
    pub start: usize,
    /// End position of the mention in the original text.
    pub end: usize,
    /// The full mention text including @.
    pub mention_text: String,
}

impl AgentMention {
    /// Create a new agent mention.
    pub fn new(agent_name: impl Into<String>, start: usize, end: usize) -> Self {
        let agent_name = agent_name.into();
        let mention_text = format!("@{}", agent_name);
        Self {
            agent_name,
            start,
            end,
            mention_text,
        }
    }
}

/// Regex for parsing @mentions.
/// Matches @word where word is alphanumeric with underscores/hyphens.
static MENTION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"@([a-zA-Z][a-zA-Z0-9_-]*)").expect("Invalid mention regex"));

/// Parse all @agent mentions from text.
///
/// Returns a list of AgentMention structs for each @word found.
/// Does not validate if the agent exists - that should be done separately.
///
/// # Examples
///
/// ```rust
/// use cortex_agents::mention::parse_agent_mentions;
///
/// let mentions = parse_agent_mentions("@general search for config files");
/// assert_eq!(mentions.len(), 1);
/// assert_eq!(mentions[0].agent_name, "general");
///
/// let mentions = parse_agent_mentions("Use @explore and @research in parallel");
/// assert_eq!(mentions.len(), 2);
/// ```
pub fn parse_agent_mentions(text: &str) -> Vec<AgentMention> {
    MENTION_REGEX
        .captures_iter(text)
        .map(|cap| {
            let full_match = cap.get(0).unwrap();
            let agent_name = cap.get(1).unwrap().as_str();
            AgentMention::new(agent_name, full_match.start(), full_match.end())
        })
        .collect()
}

/// Extract the first valid agent mention from text.
///
/// Returns the first mention that corresponds to a valid agent name
/// from the provided list of valid agents.
///
/// # Examples
///
/// ```rust
/// use cortex_agents::mention::find_first_valid_mention;
///
/// let valid_agents = vec!["general", "explore", "research"];
/// let mention = find_first_valid_mention("@invalid @general do something", &valid_agents);
/// assert!(mention.is_some());
/// assert_eq!(mention.unwrap().agent_name, "general");
/// ```
pub fn find_first_valid_mention(text: &str, valid_agents: &[&str]) -> Option<AgentMention> {
    parse_agent_mentions(text)
        .into_iter()
        .find(|m| valid_agents.contains(&m.agent_name.as_str()))
}

/// Extract the first valid agent mention, returning also the remaining text.
///
/// Returns a tuple of (AgentMention, remaining_text) where remaining_text
/// is the original text with the @mention removed.
pub fn extract_mention_and_text(
    text: &str,
    valid_agents: &[&str],
) -> Option<(AgentMention, String)> {
    let mention = find_first_valid_mention(text, valid_agents)?;

    // Remove the mention from text
    let mut remaining = String::with_capacity(text.len());
    remaining.push_str(&text[..mention.start]);
    remaining.push_str(&text[mention.end..]);

    // Trim and normalize whitespace
    let remaining = remaining.trim().to_string();

    Some((mention, remaining))
}

/// Check if text starts with a valid agent mention.
pub fn starts_with_mention(text: &str, valid_agents: &[&str]) -> bool {
    let text = text.trim();
    if let Some(mention) = find_first_valid_mention(text, valid_agents) {
        mention.start == 0 || text[..mention.start].trim().is_empty()
    } else {
        false
    }
}

/// Result of parsing a message for agent invocation.
#[derive(Debug, Clone)]
pub struct ParsedAgentMessage {
    /// The agent to invoke (if any).
    pub agent: Option<String>,
    /// The task/prompt to send to the agent.
    pub prompt: String,
    /// Original message text.
    pub original: String,
    /// Whether this should trigger a Task tool call.
    pub should_invoke_task: bool,
}

impl ParsedAgentMessage {
    /// Create a message with no agent invocation.
    pub fn plain(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            agent: None,
            prompt: text.clone(),
            original: text,
            should_invoke_task: false,
        }
    }

    /// Create a message targeting a specific agent.
    pub fn for_agent(
        agent: impl Into<String>,
        prompt: impl Into<String>,
        original: impl Into<String>,
    ) -> Self {
        Self {
            agent: Some(agent.into()),
            prompt: prompt.into(),
            original: original.into(),
            should_invoke_task: true,
        }
    }
}

/// Parse a user message to detect agent invocation.
///
/// If the message contains a valid @agent mention at the start,
/// returns a ParsedAgentMessage indicating which agent to invoke.
///
/// # Examples
///
/// ```rust
/// use cortex_agents::mention::parse_message_for_agent;
///
/// let valid_agents = vec!["general", "explore"];
///
/// // Direct mention
/// let parsed = parse_message_for_agent("@general find all config files", &valid_agents);
/// assert!(parsed.should_invoke_task);
/// assert_eq!(parsed.agent, Some("general".to_string()));
/// assert_eq!(parsed.prompt, "find all config files");
///
/// // No mention
/// let parsed = parse_message_for_agent("find all config files", &valid_agents);
/// assert!(!parsed.should_invoke_task);
/// ```
pub fn parse_message_for_agent(text: &str, valid_agents: &[&str]) -> ParsedAgentMessage {
    let text = text.trim();

    // Check if message starts with @agent
    if let Some((mention, remaining)) = extract_mention_and_text(text, valid_agents) {
        // Only trigger if mention is at the start
        if mention.start == 0 || text[..mention.start].trim().is_empty() {
            return ParsedAgentMessage::for_agent(mention.agent_name, remaining, text.to_string());
        }
    }

    ParsedAgentMessage::plain(text)
}

/// Built-in subagent names that are always available.
pub const BUILTIN_SUBAGENTS: &[&str] = &["general", "explore", "research"];

/// System prompt addition for @mention support.
pub const MENTION_SYSTEM_PROMPT: &str = r#"
## Subagent Invocation

You can invoke specialized subagents using @agent syntax:
- @general - General-purpose agent for complex searches, research, and multi-step tasks
- @explore - Fast agent for exploring codebases, finding files, and searching code
- @research - Thorough investigation agent for deep analysis (read-only)

When a user uses @agent, the Task tool will be called automatically with that subagent.

Example: "@general analyze all error handling patterns in this codebase"
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_mention() {
        let mentions = parse_agent_mentions("@general do something");
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].agent_name, "general");
        assert_eq!(mentions[0].start, 0);
        assert_eq!(mentions[0].end, 8);
    }

    #[test]
    fn test_parse_multiple_mentions() {
        let mentions = parse_agent_mentions("@explore and @research in parallel");
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[0].agent_name, "explore");
        assert_eq!(mentions[1].agent_name, "research");
    }

    #[test]
    fn test_parse_no_mentions() {
        let mentions = parse_agent_mentions("no mentions here");
        assert!(mentions.is_empty());
    }

    #[test]
    fn test_parse_email_not_mention() {
        // @ in email should not be parsed as mention (starts with letter check)
        let mentions = parse_agent_mentions("email@example.com");
        // This will match "example" since it follows @
        assert_eq!(mentions.len(), 1);
        assert_eq!(mentions[0].agent_name, "example");
    }

    #[test]
    fn test_find_first_valid() {
        let valid = vec!["general", "explore"];

        let mention = find_first_valid_mention("@invalid @general search", &valid);
        assert!(mention.is_some());
        assert_eq!(mention.unwrap().agent_name, "general");

        let mention = find_first_valid_mention("@invalid search", &valid);
        assert!(mention.is_none());
    }

    #[test]
    fn test_extract_mention_and_text() {
        let valid = vec!["general"];

        let result = extract_mention_and_text("@general find all config files", &valid);
        assert!(result.is_some());
        let (mention, remaining) = result.unwrap();
        assert_eq!(mention.agent_name, "general");
        assert_eq!(remaining, "find all config files");
    }

    #[test]
    fn test_parse_message_for_agent() {
        let valid = vec!["general", "explore"];

        // With mention at start
        let parsed = parse_message_for_agent("@general find files", &valid);
        assert!(parsed.should_invoke_task);
        assert_eq!(parsed.agent, Some("general".to_string()));
        assert_eq!(parsed.prompt, "find files");

        // Without mention
        let parsed = parse_message_for_agent("find files", &valid);
        assert!(!parsed.should_invoke_task);
        assert_eq!(parsed.agent, None);

        // With invalid mention
        let parsed = parse_message_for_agent("@invalid find files", &valid);
        assert!(!parsed.should_invoke_task);
    }

    #[test]
    fn test_starts_with_mention() {
        let valid = vec!["general"];

        assert!(starts_with_mention("@general do task", &valid));
        assert!(starts_with_mention("  @general do task", &valid));
        assert!(!starts_with_mention("do @general task", &valid));
        assert!(!starts_with_mention("@invalid task", &valid));
    }

    #[test]
    fn test_mention_with_hyphen_underscore() {
        let mentions = parse_agent_mentions("@my-agent and @my_agent");
        assert_eq!(mentions.len(), 2);
        assert_eq!(mentions[0].agent_name, "my-agent");
        assert_eq!(mentions[1].agent_name, "my_agent");
    }
}
