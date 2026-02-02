//! Wildcard pattern matching for permissions.
//!
//! Supports glob-style patterns for command permissions:
//! - `*` matches any characters
//! - `?` matches single character
//! - Patterns like "git diff*", "rm -rf*", etc.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Permission level for a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    /// Always allow without asking.
    Allow,
    /// Always ask for confirmation.
    Ask,
    /// Always deny.
    Deny,
}

impl Default for Permission {
    fn default() -> Self {
        Permission::Ask
    }
}

/// A permission pattern with associated permission level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionPattern {
    /// The pattern to match (supports * and ?).
    pub pattern: String,
    /// The permission level.
    pub permission: Permission,
}

/// Wildcard pattern matcher.
#[derive(Debug, Clone, Default)]
pub struct WildcardMatcher {
    patterns: Vec<PermissionPattern>,
}

impl WildcardMatcher {
    /// Create a new matcher.
    pub fn new() -> Self {
        Self::default()
    }

    /// Create from a map of pattern -> permission.
    pub fn from_map(map: HashMap<String, String>) -> Self {
        let patterns = map
            .into_iter()
            .map(|(pattern, perm_str)| {
                let permission = match perm_str.to_lowercase().as_str() {
                    "allow" => Permission::Allow,
                    "deny" => Permission::Deny,
                    _ => Permission::Ask,
                };
                PermissionPattern {
                    pattern,
                    permission,
                }
            })
            .collect();
        Self { patterns }
    }

    /// Add a pattern.
    pub fn add_pattern(&mut self, pattern: &str, permission: Permission) {
        self.patterns.push(PermissionPattern {
            pattern: pattern.to_string(),
            permission,
        });
    }

    /// Match a command against patterns and return the permission.
    /// Returns None if no pattern matches.
    pub fn match_command(&self, command: &str) -> Option<Permission> {
        // Check patterns in order, more specific patterns first
        // Sort by specificity (fewer wildcards = more specific)
        let mut sorted: Vec<_> = self.patterns.iter().collect();
        sorted.sort_by(|a, b| {
            let a_wildcards = a.pattern.matches('*').count() + a.pattern.matches('?').count();
            let b_wildcards = b.pattern.matches('*').count() + b.pattern.matches('?').count();
            a_wildcards.cmp(&b_wildcards)
        });

        for p in sorted {
            if Self::matches_pattern(&p.pattern, command) {
                return Some(p.permission);
            }
        }
        None
    }

    /// Check if a command matches a pattern.
    pub fn matches_pattern(pattern: &str, command: &str) -> bool {
        Self::glob_match(pattern, command)
    }

    /// Glob-style pattern matching.
    fn glob_match(pattern: &str, text: &str) -> bool {
        let pattern_chars: Vec<char> = pattern.chars().collect();
        let text_chars: Vec<char> = text.chars().collect();

        Self::glob_match_recursive(&pattern_chars, &text_chars, 0, 0)
    }

    fn glob_match_recursive(pattern: &[char], text: &[char], pi: usize, ti: usize) -> bool {
        // If pattern is exhausted, text must be too
        if pi >= pattern.len() {
            return ti >= text.len();
        }

        let p = pattern[pi];

        match p {
            '*' => {
                // * matches zero or more characters
                // Try matching zero characters, then one, then two, etc.
                for i in ti..=text.len() {
                    if Self::glob_match_recursive(pattern, text, pi + 1, i) {
                        return true;
                    }
                }
                false
            }
            '?' => {
                // ? matches exactly one character
                if ti < text.len() {
                    Self::glob_match_recursive(pattern, text, pi + 1, ti + 1)
                } else {
                    false
                }
            }
            _ => {
                // Regular character - must match exactly
                if ti < text.len() && text[ti] == p {
                    Self::glob_match_recursive(pattern, text, pi + 1, ti + 1)
                } else {
                    false
                }
            }
        }
    }

    /// Match all patterns and return structured results.
    pub fn match_all(&self, command: &str) -> Vec<(String, Permission, bool)> {
        self.patterns
            .iter()
            .map(|p| {
                let matches = Self::matches_pattern(&p.pattern, command);
                (p.pattern.clone(), p.permission, matches)
            })
            .collect()
    }

    /// Get all patterns.
    pub fn patterns(&self) -> &[PermissionPattern] {
        &self.patterns
    }
}

/// Default bash command patterns for safety.
pub fn default_bash_patterns() -> WildcardMatcher {
    let mut matcher = WildcardMatcher::new();

    // Safe commands - always allow
    matcher.add_pattern("git status*", Permission::Allow);
    matcher.add_pattern("git diff*", Permission::Allow);
    matcher.add_pattern("git log*", Permission::Allow);
    matcher.add_pattern("git show*", Permission::Allow);
    matcher.add_pattern("git branch*", Permission::Allow);
    matcher.add_pattern("ls*", Permission::Allow);
    matcher.add_pattern("cat *", Permission::Allow);
    matcher.add_pattern("head *", Permission::Allow);
    matcher.add_pattern("tail *", Permission::Allow);
    matcher.add_pattern("grep *", Permission::Allow);
    matcher.add_pattern("rg *", Permission::Allow);
    matcher.add_pattern("find * -type*", Permission::Allow);
    matcher.add_pattern("wc *", Permission::Allow);
    matcher.add_pattern("echo *", Permission::Allow);
    matcher.add_pattern("pwd", Permission::Allow);
    matcher.add_pattern("whoami", Permission::Allow);
    matcher.add_pattern("date", Permission::Allow);
    matcher.add_pattern("which *", Permission::Allow);
    matcher.add_pattern("type *", Permission::Allow);

    // Build commands - ask
    matcher.add_pattern("cargo *", Permission::Ask);
    matcher.add_pattern("npm *", Permission::Ask);
    matcher.add_pattern("yarn *", Permission::Ask);
    matcher.add_pattern("pnpm *", Permission::Ask);
    matcher.add_pattern("bun *", Permission::Ask);
    matcher.add_pattern("pip *", Permission::Ask);
    matcher.add_pattern("python *", Permission::Ask);
    matcher.add_pattern("node *", Permission::Ask);
    matcher.add_pattern("make*", Permission::Ask);
    matcher.add_pattern("cmake*", Permission::Ask);

    // Git write operations - ask
    matcher.add_pattern("git add*", Permission::Ask);
    matcher.add_pattern("git commit*", Permission::Ask);
    matcher.add_pattern("git push*", Permission::Ask);
    matcher.add_pattern("git pull*", Permission::Ask);
    matcher.add_pattern("git merge*", Permission::Ask);
    matcher.add_pattern("git rebase*", Permission::Ask);
    matcher.add_pattern("git reset*", Permission::Ask);
    matcher.add_pattern("git checkout*", Permission::Ask);

    // Dangerous commands - deny
    matcher.add_pattern("rm -rf /*", Permission::Deny);
    matcher.add_pattern("rm -rf ~*", Permission::Deny);
    matcher.add_pattern("rm -rf $HOME*", Permission::Deny);
    matcher.add_pattern("sudo rm -rf*", Permission::Deny);
    matcher.add_pattern("chmod 777*", Permission::Deny);
    matcher.add_pattern("curl * | bash*", Permission::Deny);
    matcher.add_pattern("curl * | sh*", Permission::Deny);
    matcher.add_pattern("wget * | bash*", Permission::Deny);
    matcher.add_pattern("> /dev/sd*", Permission::Deny);
    matcher.add_pattern("dd if=* of=/dev/*", Permission::Deny);
    matcher.add_pattern("mkfs*", Permission::Deny);
    matcher.add_pattern(":(){ :|:& };:*", Permission::Deny); // Fork bomb

    matcher
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wildcard_matching() {
        assert!(WildcardMatcher::matches_pattern("git diff*", "git diff"));
        assert!(WildcardMatcher::matches_pattern(
            "git diff*",
            "git diff --staged"
        ));
        assert!(WildcardMatcher::matches_pattern(
            "git diff*",
            "git diff HEAD~1"
        ));
        assert!(!WildcardMatcher::matches_pattern("git diff*", "git status"));
    }

    #[test]
    fn test_question_mark() {
        assert!(WildcardMatcher::matches_pattern("test?.txt", "test1.txt"));
        assert!(WildcardMatcher::matches_pattern("test?.txt", "testa.txt"));
        assert!(!WildcardMatcher::matches_pattern("test?.txt", "test12.txt"));
    }

    #[test]
    fn test_permission_matching() {
        let mut matcher = WildcardMatcher::new();
        matcher.add_pattern("git diff*", Permission::Allow);
        matcher.add_pattern("git push*", Permission::Ask);
        matcher.add_pattern("rm -rf*", Permission::Deny);

        assert_eq!(
            matcher.match_command("git diff --staged"),
            Some(Permission::Allow)
        );
        assert_eq!(
            matcher.match_command("git push origin main"),
            Some(Permission::Ask)
        );
        assert_eq!(matcher.match_command("rm -rf /"), Some(Permission::Deny));
        assert_eq!(matcher.match_command("unknown command"), None);
    }

    #[test]
    fn test_default_patterns() {
        let matcher = default_bash_patterns();

        assert_eq!(matcher.match_command("git status"), Some(Permission::Allow));
        assert_eq!(matcher.match_command("rm -rf /"), Some(Permission::Deny));
        assert_eq!(matcher.match_command("cargo build"), Some(Permission::Ask));
    }
}
