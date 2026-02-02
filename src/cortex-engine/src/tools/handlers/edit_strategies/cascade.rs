//! Cascade replacer that orchestrates all edit strategies.

use tracing::debug;

use super::error::EditError;
use super::helpers::truncate_string;
use super::strategies::{
    BlockAnchorReplacer, ContextAwareReplacer, EscapeNormalizedReplacer,
    IndentationFlexibleReplacer, LineTrimmedReplacer, SimpleReplacer, TrimmedBoundaryReplacer,
    WhitespaceNormalizedReplacer,
};
use super::traits::EditStrategy;

/// Result from a successful cascade replacement
#[derive(Debug, Clone)]
pub struct CascadeResult {
    /// The new content after replacement
    pub content: String,
    /// Name of the strategy that succeeded
    pub strategy_name: &'static str,
    /// Confidence level of the match
    pub confidence: f64,
}

/// Cascade replacer that tries multiple strategies in order.
/// Returns the result from the first successful strategy.
pub struct CascadeReplacer {
    strategies: Vec<Box<dyn EditStrategy>>,
}

impl CascadeReplacer {
    /// Creates a new cascade replacer with all 8 strategies in order.
    pub fn new() -> Self {
        Self {
            strategies: vec![
                Box::new(SimpleReplacer),
                Box::new(LineTrimmedReplacer),
                Box::new(BlockAnchorReplacer),
                Box::new(WhitespaceNormalizedReplacer),
                Box::new(IndentationFlexibleReplacer),
                Box::new(EscapeNormalizedReplacer),
                Box::new(TrimmedBoundaryReplacer),
                Box::new(ContextAwareReplacer::default()),
            ],
        }
    }

    /// Creates a cascade replacer with custom strategies.
    pub fn with_strategies(strategies: Vec<Box<dyn EditStrategy>>) -> Self {
        Self { strategies }
    }

    /// Attempts to replace `old` with `new` in `content` using cascade of strategies.
    /// Returns the new content and the name of the strategy that succeeded.
    pub fn replace(&self, content: &str, old: &str, new: &str) -> Result<CascadeResult, EditError> {
        let mut tried_strategies = Vec::new();

        for strategy in &self.strategies {
            tried_strategies.push(strategy.name());

            if let Some(result) = strategy.try_replace(content, old, new) {
                debug!(
                    "Cascade replacement succeeded with strategy '{}' (confidence: {:.0}%)",
                    strategy.name(),
                    strategy.confidence() * 100.0
                );

                return Ok(CascadeResult {
                    content: result,
                    strategy_name: strategy.name(),
                    confidence: strategy.confidence(),
                });
            }
        }

        Err(EditError::NoMatchFound {
            search: truncate_string(old, 100),
            strategies_tried: tried_strategies,
        })
    }

    /// Attempts to replace all occurrences of `old` with `new`.
    pub fn replace_all(
        &self,
        content: &str,
        old: &str,
        new: &str,
    ) -> Result<CascadeResult, EditError> {
        let mut tried_strategies = Vec::new();

        for strategy in &self.strategies {
            tried_strategies.push(strategy.name());

            if let Some(result) = strategy.try_replace_all(content, old, new) {
                debug!(
                    "Cascade replace_all succeeded with strategy '{}' (confidence: {:.0}%)",
                    strategy.name(),
                    strategy.confidence() * 100.0
                );

                return Ok(CascadeResult {
                    content: result,
                    strategy_name: strategy.name(),
                    confidence: strategy.confidence(),
                });
            }
        }

        Err(EditError::NoMatchFound {
            search: truncate_string(old, 100),
            strategies_tried: tried_strategies,
        })
    }

    /// Returns the list of strategy names in order.
    pub fn strategy_names(&self) -> Vec<&'static str> {
        self.strategies.iter().map(|s| s.name()).collect()
    }
}

impl Default for CascadeReplacer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cascade_tries_strategies_in_order() {
        let cascade = CascadeReplacer::new();

        // Exact match should use SimpleReplacer
        let content = "let x = 1;";
        let result = cascade.replace(content, "let x = 1;", "let y = 2;");

        assert!(result.is_ok());
        let cascade_result = result.unwrap();
        assert_eq!(cascade_result.strategy_name, "SimpleReplacer");
        assert_eq!(cascade_result.confidence, 1.0);
    }

    #[test]
    fn test_cascade_falls_back_to_fuzzy() {
        let cascade = CascadeReplacer::new();

        // With indentation differences, should fall back to a fuzzy strategy
        let content = "    fn test() {\n        let x = 1;\n    }";
        let search = "fn test() {\n    let x = 1;\n}"; // Different indentation
        let result = cascade.replace(content, search, "fn test() {}");

        assert!(result.is_ok());
        let cascade_result = result.unwrap();
        // Should use a fuzzy strategy since exact match fails
        // IndentationFlexibleReplacer or LineTrimmedReplacer should match
        assert!(
            cascade_result.strategy_name != "SimpleReplacer",
            "Expected fuzzy strategy, got: {}",
            cascade_result.strategy_name
        );
    }

    #[test]
    fn test_cascade_returns_error_when_no_match() {
        let cascade = CascadeReplacer::new();

        let content = "fn main() {}";
        let result = cascade.replace(content, "nonexistent_function()", "replacement");

        assert!(result.is_err());
        if let Err(EditError::NoMatchFound {
            strategies_tried, ..
        }) = result
        {
            assert_eq!(strategies_tried.len(), 8); // All 8 strategies tried
        }
    }

    #[test]
    fn test_cascade_replace_all() {
        let cascade = CascadeReplacer::new();

        let content = "let x = 1;\nlet y = 1;\nlet z = 1;";
        let result = cascade.replace_all(content, "1", "2");

        assert!(result.is_ok());
        let new_content = result.unwrap().content;
        assert_eq!(new_content.matches("2").count(), 3);
    }
}
