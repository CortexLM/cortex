//! Legacy compatibility layer for backwards compatibility with older APIs.

use super::cascade::CascadeReplacer;
use super::error::EditError;
use super::helpers::truncate_string;

/// Legacy Strategy enum for backwards compatibility
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Strategy {
    Exact,
    Line,
    Block,
    Anchor,
    Function,
    Indent,
    Semantic,
    Hybrid,
}

impl Strategy {
    pub fn name(&self) -> &'static str {
        match self {
            Strategy::Exact => "exact",
            Strategy::Line => "line",
            Strategy::Block => "block",
            Strategy::Anchor => "anchor",
            Strategy::Function => "function",
            Strategy::Indent => "indent",
            Strategy::Semantic => "semantic",
            Strategy::Hybrid => "hybrid",
        }
    }

    pub fn all() -> &'static [Strategy] {
        &[
            Strategy::Exact,
            Strategy::Line,
            Strategy::Block,
            Strategy::Anchor,
            Strategy::Function,
            Strategy::Indent,
            Strategy::Semantic,
            Strategy::Hybrid,
        ]
    }

    pub fn fallback_chain() -> &'static [Strategy] {
        Self::all()
    }
}

/// Legacy match result for backwards compatibility
#[derive(Debug, Clone)]
pub struct MatchResult {
    pub matched_text: String,
    pub start_index: usize,
    pub end_index: usize,
    pub confidence: f64,
    pub strategy: Strategy,
    pub info: Option<String>,
}

impl MatchResult {
    pub fn new(
        matched_text: String,
        start_index: usize,
        end_index: usize,
        confidence: f64,
        strategy: Strategy,
    ) -> Self {
        Self {
            matched_text,
            start_index,
            end_index,
            confidence,
            strategy,
            info: None,
        }
    }
}

/// Legacy match error for backwards compatibility
#[derive(Debug, Clone)]
pub enum MatchError {
    NotFound {
        search: String,
        strategies_tried: Vec<&'static str>,
    },
    MultipleMatches {
        count: usize,
        strategy: Strategy,
        hint: String,
    },
}

impl std::fmt::Display for MatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MatchError::NotFound {
                search,
                strategies_tried,
            } => {
                write!(
                    f,
                    "Could not find '{}' in file (tried: {})",
                    search,
                    strategies_tried.join(", ")
                )
            }
            MatchError::MultipleMatches { count, hint, .. } => {
                write!(f, "Found {} occurrences. {}", count, hint)
            }
        }
    }
}

impl std::error::Error for MatchError {}

/// Legacy FuzzyMatcher for backwards compatibility
pub struct FuzzyMatcher {
    content: String,
    cascade: CascadeReplacer,
}

impl FuzzyMatcher {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            cascade: CascadeReplacer::new(),
        }
    }

    pub fn find_best_match(
        &self,
        search: &str,
        _replace_all: bool,
    ) -> Result<MatchResult, MatchError> {
        // Check for exact matches first
        let matches: Vec<_> = self.content.match_indices(search).collect();

        if matches.len() == 1 {
            let (start, matched) = matches[0];
            return Ok(MatchResult {
                matched_text: matched.to_string(),
                start_index: start,
                end_index: start + matched.len(),
                confidence: 1.0,
                strategy: Strategy::Exact,
                info: None,
            });
        } else if matches.len() > 1 {
            return Err(MatchError::MultipleMatches {
                count: matches.len(),
                strategy: Strategy::Exact,
                hint: "Provide more surrounding context to make the match unique".into(),
            });
        }

        // Try fuzzy matching with cascade
        // For backwards compat, we need to find the match location
        let cascade_result = self
            .cascade
            .replace(&self.content, search, "__PLACEHOLDER__");

        match cascade_result {
            Ok(result) => {
                // Find where the placeholder ended up
                if let Some(pos) = result.content.find("__PLACEHOLDER__") {
                    let strategy = match result.strategy_name {
                        "SimpleReplacer" => Strategy::Exact,
                        "LineTrimmedReplacer" => Strategy::Line,
                        "BlockAnchorReplacer" => Strategy::Block,
                        "WhitespaceNormalizedReplacer" | "IndentationFlexibleReplacer" => {
                            Strategy::Indent
                        }
                        "ContextAwareReplacer" => Strategy::Anchor,
                        _ => Strategy::Hybrid,
                    };

                    Ok(MatchResult {
                        matched_text: search.to_string(),
                        start_index: pos,
                        end_index: pos + search.len(),
                        confidence: result.confidence,
                        strategy,
                        info: Some(format!("Matched using {}", result.strategy_name)),
                    })
                } else {
                    Err(MatchError::NotFound {
                        search: truncate_string(search, 50),
                        strategies_tried: self.cascade.strategy_names(),
                    })
                }
            }
            Err(EditError::NoMatchFound {
                search,
                strategies_tried,
            }) => Err(MatchError::NotFound {
                search,
                strategies_tried,
            }),
            Err(EditError::MultipleMatches {
                count,
                strategy: _,
                hint,
            }) => Err(MatchError::MultipleMatches {
                count,
                strategy: Strategy::Hybrid,
                hint,
            }),
        }
    }

    pub fn find_matches_with_strategy(&self, search: &str, strategy: Strategy) -> Vec<MatchResult> {
        let mut results = Vec::new();

        // For backwards compatibility, just do exact matching for each strategy
        let matches: Vec<_> = self.content.match_indices(search).collect();

        for (start, matched) in matches {
            results.push(MatchResult {
                matched_text: matched.to_string(),
                start_index: start,
                end_index: start + matched.len(),
                confidence: 1.0,
                strategy,
                info: None,
            });
        }

        results
    }
}

/// Legacy fuzzy_replace function for backwards compatibility
pub fn fuzzy_replace(
    content: &str,
    old_str: &str,
    new_str: &str,
    replace_all: bool,
) -> Result<(String, MatchResult), MatchError> {
    let cascade = CascadeReplacer::new();

    let result = if replace_all {
        cascade.replace_all(content, old_str, new_str)
    } else {
        cascade.replace(content, old_str, new_str)
    };

    match result {
        Ok(cascade_result) => {
            let strategy = match cascade_result.strategy_name {
                "SimpleReplacer" => Strategy::Exact,
                "LineTrimmedReplacer" => Strategy::Line,
                "BlockAnchorReplacer" => Strategy::Block,
                "WhitespaceNormalizedReplacer" | "IndentationFlexibleReplacer" => Strategy::Indent,
                "ContextAwareReplacer" => Strategy::Anchor,
                _ => Strategy::Hybrid,
            };

            let match_result = MatchResult {
                matched_text: old_str.to_string(),
                start_index: 0,
                end_index: old_str.len(),
                confidence: cascade_result.confidence,
                strategy,
                info: Some(format!("Matched using {}", cascade_result.strategy_name)),
            };

            Ok((cascade_result.content, match_result))
        }
        Err(EditError::NoMatchFound {
            search,
            strategies_tried,
        }) => Err(MatchError::NotFound {
            search,
            strategies_tried,
        }),
        Err(EditError::MultipleMatches {
            count,
            strategy: _,
            hint,
        }) => Err(MatchError::MultipleMatches {
            count,
            strategy: Strategy::Hybrid,
            hint,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_fuzzy_replace() {
        let content = "fn main() {\n    println!(\"Hello\");\n}";
        let result = fuzzy_replace(
            content,
            "println!(\"Hello\");",
            "println!(\"World\");",
            false,
        );

        assert!(result.is_ok());
        let (new_content, match_result) = result.unwrap();
        assert!(new_content.contains("World"));
        assert_eq!(match_result.strategy, Strategy::Exact);
    }

    #[test]
    fn test_legacy_fuzzy_matcher() {
        let content = "fn main() {\n    let x = 1;\n}";
        let matcher = FuzzyMatcher::new(content);

        let result = matcher.find_best_match("let x = 1;", false);
        assert!(result.is_ok());
    }
}
