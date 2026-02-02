//! Command completion engine for cortex-tui.
//!
//! This module provides fuzzy completion for slash commands,
//! including scoring and category-aware sorting.

use super::registry::CommandRegistry;
use super::types::CommandCategory;

// ============================================================
// COMPLETION
// ============================================================

/// A single completion suggestion.
#[derive(Debug, Clone)]
pub struct Completion {
    /// The command name (without /).
    pub command: String,
    /// Display text for the completion.
    pub display: String,
    /// Description shown alongside.
    pub description: String,
    /// Category for grouping.
    pub category: CommandCategory,
    /// Relevance score (higher is better).
    pub score: i32,
    /// Whether this is from recent history.
    pub is_recent: bool,
}

impl Completion {
    /// Creates a new completion.
    pub fn new(
        command: String,
        display: String,
        description: String,
        category: CommandCategory,
        score: i32,
    ) -> Self {
        Self {
            command,
            display,
            description,
            category,
            score,
            is_recent: false,
        }
    }

    /// Marks this completion as from recent history.
    pub fn with_recent(mut self) -> Self {
        self.is_recent = true;
        self
    }
}

// ============================================================
// COMPLETION ENGINE
// ============================================================

/// Fuzzy completion engine for slash commands.
pub struct CompletionEngine<'a> {
    registry: &'a CommandRegistry,
    recent: Vec<String>,
    max_recent: usize,
}

impl<'a> CompletionEngine<'a> {
    /// Creates a new completion engine with the given registry.
    pub fn new(registry: &'a CommandRegistry) -> Self {
        Self {
            registry,
            recent: Vec::new(),
            max_recent: 10,
        }
    }

    /// Adds a command to recent history.
    pub fn add_recent(&mut self, command: &str) {
        // Remove if already exists
        self.recent.retain(|c| c != command);

        // Add to front
        self.recent.insert(0, command.to_string());

        // Trim to max size
        if self.recent.len() > self.max_recent {
            self.recent.truncate(self.max_recent);
        }
    }

    /// Gets completions for partial input.
    ///
    /// The input should start with `/` for command completion.
    /// Returns completions sorted by relevance score.
    pub fn complete(&self, partial: &str) -> Vec<Completion> {
        let partial = partial.trim();

        // Must start with /
        if !partial.starts_with('/') {
            return Vec::new();
        }

        let query = &partial[1..].to_lowercase();
        let mut completions = Vec::new();

        // Score and collect matching commands
        for def in self.registry.all() {
            // Check primary name
            if let Some(score) = Self::fuzzy_score(query, def.name) {
                completions.push(Completion::new(
                    def.name.to_string(),
                    format!("/{}", def.name),
                    def.description.to_string(),
                    def.category,
                    score + self.recent_bonus(def.name),
                ));
            } else {
                // Check aliases
                for alias in def.aliases {
                    if let Some(score) = Self::fuzzy_score(query, alias) {
                        completions.push(Completion::new(
                            def.name.to_string(),
                            format!("/{} ({})", def.name, alias),
                            def.description.to_string(),
                            def.category,
                            score - 10 + self.recent_bonus(def.name), // Slight penalty for alias match
                        ));
                        break; // Only add once per command
                    }
                }
            }
        }

        // Sort by score (descending), then by name
        completions.sort_by(|a, b| {
            b.score
                .cmp(&a.score)
                .then_with(|| a.command.cmp(&b.command))
        });

        // Mark recent completions
        for completion in &mut completions {
            if self.recent.contains(&completion.command) {
                completion.is_recent = true;
            }
        }

        completions
    }

    /// Gets completions grouped by category.
    pub fn complete_grouped(&self, partial: &str) -> Vec<(CommandCategory, Vec<Completion>)> {
        let completions = self.complete(partial);
        let mut groups: Vec<(CommandCategory, Vec<Completion>)> = Vec::new();

        // First add recent if any
        let recent: Vec<_> = completions
            .iter()
            .filter(|c| c.is_recent)
            .cloned()
            .collect();

        // Then group by category
        for category in CommandCategory::all() {
            let in_category: Vec<_> = completions
                .iter()
                .filter(|c| c.category == *category && !c.is_recent)
                .cloned()
                .collect();

            if !in_category.is_empty() {
                groups.push((*category, in_category));
            }
        }

        // Prepend recent group if non-empty
        if !recent.is_empty() {
            groups.insert(0, (CommandCategory::General, recent)); // Use General as "Recent" placeholder
        }

        groups
    }

    /// Calculates fuzzy match score.
    ///
    /// Returns `Some(score)` if the pattern matches, `None` otherwise.
    /// Higher scores indicate better matches.
    fn fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
        if pattern.is_empty() {
            return Some(0);
        }

        let text_lower = text.to_lowercase();

        // Exact match gets highest score
        if text_lower == pattern {
            return Some(1000);
        }

        // Prefix match gets high score
        if text_lower.starts_with(pattern) {
            return Some(500 + (100 - text.len() as i32).max(0));
        }

        // Contains match gets medium score
        if text_lower.contains(pattern) {
            return Some(200 + (50 - text.len() as i32).max(0));
        }

        // Subsequence match gets lower score
        let mut pattern_chars = pattern.chars().peekable();
        let mut score = 0;
        let mut consecutive = 0;
        let mut last_match_pos: Option<usize> = None;

        for (i, c) in text_lower.chars().enumerate() {
            if pattern_chars.peek() == Some(&c) {
                pattern_chars.next();
                score += 10;

                // Bonus for consecutive matches
                if i > 0 && last_match_pos == Some(i - 1) {
                    consecutive += 1;
                    score += consecutive * 5;
                } else {
                    consecutive = 0;
                }

                // Bonus for matching at word boundaries
                if i == 0
                    || !text
                        .chars()
                        .nth(i.saturating_sub(1))
                        .unwrap_or(' ')
                        .is_alphanumeric()
                {
                    score += 15;
                }

                last_match_pos = Some(i);
            }
        }

        if pattern_chars.peek().is_none() {
            Some(score)
        } else {
            None
        }
    }

    /// Returns bonus score if command is in recent history.
    fn recent_bonus(&self, command: &str) -> i32 {
        match self.recent.iter().position(|c| c == command) {
            Some(0) => 200, // Most recent
            Some(1) => 150,
            Some(2) => 100,
            Some(_) => 50,
            None => 0,
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> CommandRegistry {
        CommandRegistry::default()
    }

    #[test]
    fn test_complete_exact() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let completions = engine.complete("/help");
        assert!(!completions.is_empty());
        assert_eq!(completions[0].command, "help");
    }

    #[test]
    fn test_complete_prefix() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let completions = engine.complete("/hel");
        assert!(!completions.is_empty());
        assert!(completions.iter().any(|c| c.command == "help"));
    }

    #[test]
    fn test_complete_fuzzy() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let completions = engine.complete("/ses");
        assert!(completions.iter().any(|c| c.command == "sessions"));
    }

    #[test]
    fn test_complete_empty_query() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let completions = engine.complete("/");
        assert!(!completions.is_empty());
    }

    #[test]
    fn test_complete_no_slash() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let completions = engine.complete("help");
        assert!(completions.is_empty());
    }

    #[test]
    fn test_complete_alias() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let completions = engine.complete("/q");
        assert!(completions.iter().any(|c| c.command == "quit"));
    }

    #[test]
    fn test_recent_boost() {
        let registry = test_registry();
        let mut engine = CompletionEngine::new(&registry);

        engine.add_recent("sessions");

        let completions = engine.complete("/s");
        // Sessions should be near the top due to recent boost
        let sessions_pos = completions
            .iter()
            .position(|c| c.command == "sessions")
            .unwrap();
        assert!(sessions_pos < 5);
    }

    #[test]
    fn test_recent_marked() {
        let registry = test_registry();
        let mut engine = CompletionEngine::new(&registry);

        engine.add_recent("help");

        let completions = engine.complete("/h");
        let help = completions.iter().find(|c| c.command == "help").unwrap();
        assert!(help.is_recent);
    }

    #[test]
    fn test_fuzzy_score() {
        // Exact match
        assert!(CompletionEngine::fuzzy_score("help", "help").unwrap() > 500);

        // Prefix match
        assert!(CompletionEngine::fuzzy_score("hel", "help").unwrap() > 200);

        // Subsequence match
        assert!(CompletionEngine::fuzzy_score("hp", "help").is_some());

        // No match
        assert!(CompletionEngine::fuzzy_score("xyz", "help").is_none());
    }

    #[test]
    fn test_complete_grouped() {
        let registry = test_registry();
        let engine = CompletionEngine::new(&registry);

        let groups = engine.complete_grouped("/");
        assert!(!groups.is_empty());

        // Should have multiple categories
        assert!(groups.len() > 1);
    }

    #[test]
    fn test_add_recent_dedup() {
        let registry = test_registry();
        let mut engine = CompletionEngine::new(&registry);

        engine.add_recent("help");
        engine.add_recent("quit");
        engine.add_recent("help"); // Add again

        assert_eq!(engine.recent.len(), 2);
        assert_eq!(engine.recent[0], "help"); // Most recent first
    }

    #[test]
    fn test_add_recent_max() {
        let registry = test_registry();
        let mut engine = CompletionEngine::new(&registry);

        for i in 0..20 {
            engine.add_recent(&format!("cmd{}", i));
        }

        assert_eq!(engine.recent.len(), engine.max_recent);
    }
}
