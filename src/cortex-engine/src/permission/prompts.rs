//! Permission prompts for Cortex CLI.
//!
//! NOTE: This module provides permission prompt types and formatting.
//! Actual TUI rendering should be done in cortex-tui crate.
//! This module only provides the data structures and formatting logic.

use std::fmt;

use super::types::{PermissionContext, PermissionResponse, PermissionScope, RiskLevel};

/// User's response to a permission prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptResponse {
    /// Allow this specific request only (y).
    Yes,
    /// Deny this specific request (n).
    No,
    /// Always allow this pattern (a).
    Always,
    /// Allow for this session only (s).
    Session,
    /// Never allow this pattern (e).
    Never,
}

impl PromptResponse {
    /// Parse a single character response.
    pub fn from_char(c: char) -> Option<Self> {
        match c.to_ascii_lowercase() {
            'y' => Some(Self::Yes),
            'n' => Some(Self::No),
            'a' => Some(Self::Always),
            's' => Some(Self::Session),
            'e' => Some(Self::Never),
            _ => None,
        }
    }

    /// Parse a string response.
    pub fn from_str(s: &str) -> Option<Self> {
        let s = s.trim().to_lowercase();
        match s.as_str() {
            "y" | "yes" => Some(Self::Yes),
            "n" | "no" => Some(Self::No),
            "a" | "always" => Some(Self::Always),
            "s" | "session" => Some(Self::Session),
            "e" | "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Convert to permission response.
    pub fn to_permission_response(&self) -> PermissionResponse {
        match self {
            Self::Yes | Self::Always | Self::Session => PermissionResponse::Allow,
            Self::No | Self::Never => PermissionResponse::Deny,
        }
    }

    /// Convert to permission scope.
    pub fn to_scope(&self) -> PermissionScope {
        match self {
            Self::Yes | Self::No => PermissionScope::Once,
            Self::Session => PermissionScope::Session,
            Self::Always | Self::Never => PermissionScope::Always,
        }
    }
}

/// A permission prompt to display to the user.
#[derive(Debug, Clone)]
pub struct PermissionPrompt {
    /// Tool requesting permission.
    pub tool: String,
    /// Action being performed.
    pub action: String,
    /// Context for the permission request.
    pub context: PermissionContext,
    /// Pattern that will be stored if user chooses always/session.
    pub pattern: String,
}

impl PermissionPrompt {
    /// Create a new permission prompt.
    pub fn new(
        tool: impl Into<String>,
        action: impl Into<String>,
        context: PermissionContext,
        pattern: impl Into<String>,
    ) -> Self {
        Self {
            tool: tool.into(),
            action: action.into(),
            context,
            pattern: pattern.into(),
        }
    }

    /// Create a prompt for a bash command.
    pub fn for_bash(command: &str, risk: RiskLevel) -> Self {
        let context = PermissionContext::for_command(command).with_risk(risk);
        let pattern = crate::permission::patterns::extract_command_prefix(command);

        Self::new(
            "bash",
            format!("execute: {}", truncate_command(command, 60)),
            context,
            pattern,
        )
    }

    /// Create a prompt for a file write.
    pub fn for_file_write(path: &std::path::Path, risk: RiskLevel) -> Self {
        let context = PermissionContext::for_file(path).with_risk(risk);
        let pattern = crate::permission::patterns::extract_path_pattern(path);

        Self::new(
            "write",
            format!("write to: {}", path.display()),
            context,
            pattern,
        )
    }

    /// Create a prompt for a file edit.
    pub fn for_file_edit(path: &std::path::Path, risk: RiskLevel) -> Self {
        let context = PermissionContext::for_file(path).with_risk(risk);
        let pattern = crate::permission::patterns::extract_path_pattern(path);

        Self::new(
            "edit",
            format!("edit: {}", path.display()),
            context,
            pattern,
        )
    }

    /// Format the prompt message.
    pub fn format_message(&self) -> String {
        format!("[{}] wants to {}", self.tool, self.action)
    }

    /// Format the full prompt with options.
    pub fn format_full(&self) -> String {
        let mut output = String::new();

        // Header
        output.push_str("Permission Request\n");
        output.push_str("─────────────────────\n");

        // Tool and action
        output.push_str(&format!("Tool: {}\n", self.tool));
        output.push_str(&format!("Action: {}\n", self.action));

        // Risk level with color hint
        let risk_str = match self.context.risk_level {
            RiskLevel::Low => "Low",
            RiskLevel::Medium => "Medium",
            RiskLevel::High => "High",
            RiskLevel::Critical => "Critical",
        };
        output.push_str(&format!("Risk: {}\n", risk_str));

        // Additional context
        if let Some(ref desc) = self.context.description {
            output.push_str(&format!("Description: {}\n", desc));
        }

        if let Some(ref cmd) = self.context.command {
            output.push_str(&format!("Command: {}\n", cmd));
        }

        if let Some(ref path) = self.context.file_path {
            output.push_str(&format!("Path: {}\n", path.display()));
        }

        output.push_str("\n");
        output.push_str("Options:\n");
        output.push_str("  [y] Yes - allow this once\n");
        output.push_str("  [n] No - deny this once\n");
        output.push_str("  [a] Always - always allow this pattern\n");
        output.push_str("  [s] Session - allow for this session\n");
        output.push_str("  [e] Never - always deny this pattern\n");
        output.push_str("\n");
        output.push_str(&format!("Pattern: {}\n", self.pattern));

        output
    }

    /// Format a short prompt for inline display.
    pub fn format_short(&self) -> String {
        format!(
            "[{}] wants to {}. Allow? [y/n/a(always)/s(session)/e(never)]",
            self.tool, self.action
        )
    }

    /// Get the risk indicator for display.
    pub fn risk_indicator(&self) -> &'static str {
        match self.context.risk_level {
            RiskLevel::Low => "[low]",
            RiskLevel::Medium => "[med]",
            RiskLevel::High => "[HIGH]",
            RiskLevel::Critical => "[CRIT]",
        }
    }
}

impl fmt::Display for PermissionPrompt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.format_short())
    }
}

/// Truncate a command for display.
fn truncate_command(command: &str, max_len: usize) -> String {
    cortex_common::truncate_command(command, max_len).into_owned()
}

/// Format a permission list for display.
pub fn format_permission_list(permissions: &[super::types::Permission]) -> String {
    if permissions.is_empty() {
        return "No permissions stored.".to_string();
    }

    let mut output = String::new();
    output.push_str("Stored Permissions:\n");
    output.push_str("═══════════════════\n\n");

    for perm in permissions {
        let status = if perm.allows() {
            "✓ Allow"
        } else {
            "✗ Deny"
        };
        let scope = match perm.scope {
            PermissionScope::Once => "once",
            PermissionScope::Session => "session",
            PermissionScope::Always => "always",
        };

        output.push_str(&format!(
            "{} [{}] {}: {}\n",
            status, scope, perm.tool, perm.pattern
        ));

        if let Some(ref reason) = perm.reason {
            output.push_str(&format!("  Reason: {}\n", reason));
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prompt_response_from_char() {
        assert_eq!(PromptResponse::from_char('y'), Some(PromptResponse::Yes));
        assert_eq!(PromptResponse::from_char('Y'), Some(PromptResponse::Yes));
        assert_eq!(PromptResponse::from_char('n'), Some(PromptResponse::No));
        assert_eq!(PromptResponse::from_char('a'), Some(PromptResponse::Always));
        assert_eq!(
            PromptResponse::from_char('s'),
            Some(PromptResponse::Session)
        );
        assert_eq!(PromptResponse::from_char('e'), Some(PromptResponse::Never));
        assert_eq!(PromptResponse::from_char('x'), None);
    }

    #[test]
    fn test_prompt_response_from_str() {
        assert_eq!(PromptResponse::from_str("yes"), Some(PromptResponse::Yes));
        assert_eq!(PromptResponse::from_str("NO"), Some(PromptResponse::No));
        assert_eq!(
            PromptResponse::from_str("always"),
            Some(PromptResponse::Always)
        );
    }

    #[test]
    fn test_prompt_response_conversion() {
        let resp = PromptResponse::Yes;
        assert_eq!(resp.to_permission_response(), PermissionResponse::Allow);
        assert_eq!(resp.to_scope(), PermissionScope::Once);

        let resp = PromptResponse::Always;
        assert_eq!(resp.to_permission_response(), PermissionResponse::Allow);
        assert_eq!(resp.to_scope(), PermissionScope::Always);

        let resp = PromptResponse::Never;
        assert_eq!(resp.to_permission_response(), PermissionResponse::Deny);
        assert_eq!(resp.to_scope(), PermissionScope::Always);
    }

    #[test]
    fn test_permission_prompt_bash() {
        let prompt = PermissionPrompt::for_bash("git push origin main", RiskLevel::Medium);

        assert_eq!(prompt.tool, "bash");
        assert!(prompt.action.contains("git push"));
        assert_eq!(prompt.context.risk_level, RiskLevel::Medium);
    }

    #[test]
    fn test_permission_prompt_format() {
        let prompt = PermissionPrompt::for_bash("git status", RiskLevel::Low);

        let short = prompt.format_short();
        assert!(short.contains("bash"));
        assert!(short.contains("git status"));
        assert!(short.contains("[y/n/a"));

        let full = prompt.format_full();
        assert!(full.contains("Permission Request"));
        assert!(full.contains("Tool: bash"));
    }

    #[test]
    fn test_truncate_command() {
        assert_eq!(truncate_command("short", 10), "short");
        // truncate_command tries to cut on word boundaries, so "this is a..." (12 chars)
        // instead of "this is a ve..." (14 chars) because we look for last space
        assert_eq!(
            truncate_command("this is a very long command", 15),
            "this is a..."
        );
    }
}
