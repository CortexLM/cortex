//! Task-related prompts for Cortex CLI.
//!
//! This module contains prompts used for various task-oriented operations
//! like summarization, compaction, and conversation management.

/// System prompt for specialized summarization.
///
/// Used for condensing conversation history while preserving key decisions,
/// state changes, and important context.
///
/// # Key Focus Areas
/// - Key decisions made by user or assistant
/// - Major state changes (files created/modified, commands executed)
/// - Important context needed for future turns
/// - Core topics and goals discussed
pub const SUMMARIZATION_PROMPT: &str = r#"You are a specialized summarization assistant. Your task is to condense the provided conversation history into a concise summary that preserves key decisions, state changes, and important context for maintaining the conversation's continuity.

Focus on:
1. Key decisions made by the user or assistant.
2. Major state changes (e.g., files created/modified, commands executed).
3. Important context that would be needed for future turns.
4. Core topics and goals discussed.

Avoid:
1. Verbatim repetition of long messages.
2. Unnecessary conversational filler.
3. Minor details that don't affect the overall progress.

The summary should be structured and easy to read, ensuring that an AI agent reading it can perfectly understand the current state of the task."#;

/// Default prompt for conversation compaction.
///
/// Used when auto-compacting conversation history to reduce token usage.
pub const COMPACTION_PROMPT: &str = r#"Summarize the conversation history above. Focus on:
1. Key decisions made
2. Important code changes
3. Outstanding tasks or issues
4. Context needed to continue the conversation

Be concise but preserve critical information."#;

/// Prefix added before summaries in compacted conversations.
pub const SUMMARY_PREFIX: &str = "[Previous conversation summary]\n";

/// Prompt for requesting an explicit summary from a subagent.
///
/// Used when a subagent completes work but doesn't provide a structured summary.
/// This follows the pattern of ensuring structured output for the orchestrator.
pub const SUMMARY_REQUEST_PROMPT: &str = r#"You have completed your work but did not provide a summary. Please provide a final summary NOW using EXACTLY this format:

## Summary for Orchestrator

### Tasks Completed
- [List each task you completed with brief outcome]

### Key Findings/Changes
- [Main discoveries or modifications made]

### Files Modified (if any)
- [List of files with type of change]

### Recommendations (if applicable)
- [Any follow-up actions or suggestions]

### Status: COMPLETED

DO NOT use any tools. Just provide the summary based on the work you have already done."#;

/// Markers that indicate a proper summary output from a subagent.
pub const SUMMARY_MARKERS: &[&str] = &[
    "## Summary for Orchestrator",
    "### Tasks Completed",
    "### Key Findings",
    "### Status: COMPLETED",
    "Status: COMPLETED",
    "## Summary",
    "### Summary",
    "## Final Summary",
    "### Final Summary",
];

/// Check if a response contains a proper summary for the orchestrator.
///
/// Returns `true` if any of the summary markers are present, `false` otherwise.
///
/// # Example
///
/// ```rust
/// use cortex_prompt_harness::prompts::tasks::has_summary_output;
///
/// let response = "## Summary for Orchestrator\n### Tasks Completed\n- Analyzed code";
/// assert!(has_summary_output(response));
///
/// let response = "I looked at the code and found some issues.";
/// assert!(!has_summary_output(response));
/// ```
pub fn has_summary_output(response: &str) -> bool {
    if response.trim().is_empty() {
        return false;
    }

    let response_lower = response.to_lowercase();
    SUMMARY_MARKERS
        .iter()
        .any(|marker| response_lower.contains(&marker.to_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarization_prompt_focus_areas() {
        assert!(SUMMARIZATION_PROMPT.contains("Key decisions"));
        assert!(SUMMARIZATION_PROMPT.contains("state changes"));
        assert!(SUMMARIZATION_PROMPT.contains("Important context"));
    }

    #[test]
    fn test_compaction_prompt_focus_areas() {
        assert!(COMPACTION_PROMPT.contains("Key decisions"));
        assert!(COMPACTION_PROMPT.contains("code changes"));
        assert!(COMPACTION_PROMPT.contains("Outstanding tasks"));
    }

    #[test]
    fn test_has_summary_output_with_markers() {
        let response = "## Summary for Orchestrator\n### Tasks Completed\n- Done";
        assert!(has_summary_output(response));

        let response = "Status: COMPLETED";
        assert!(has_summary_output(response));

        let response = "## Summary\nSome findings";
        assert!(has_summary_output(response));
    }

    #[test]
    fn test_has_summary_output_without_markers() {
        let response = "I found some issues in the code.";
        assert!(!has_summary_output(response));

        let response = "";
        assert!(!has_summary_output(response));

        let response = "   ";
        assert!(!has_summary_output(response));
    }

    #[test]
    fn test_has_summary_output_case_insensitive() {
        let response = "## summary for orchestrator";
        assert!(has_summary_output(response));

        let response = "STATUS: COMPLETED";
        assert!(has_summary_output(response));
    }
}
