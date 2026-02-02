//! Edit replacement strategies for robust code editing.
//!
//! This module implements 8 cascading replacement strategies for flexible text matching.
//! Each strategy tries progressively more flexible matching approaches to find and
//! replace text in source code.
//!
//! Strategies (in cascade order):
//! 1. SimpleReplacer - Exact string match
//! 2. LineTrimmedReplacer - Ignore spaces at start/end of each line
//! 3. BlockAnchorReplacer - Match by first and last line anchors
//! 4. WhitespaceNormalizedReplacer - Normalize all whitespace (multiple -> single)
//! 5. IndentationFlexibleReplacer - Ignore indentation (tabs/spaces)
//! 6. EscapeNormalizedReplacer - Normalize escape characters
//! 7. TrimmedBoundaryReplacer - Match on trimmed content with context
//! 8. ContextAwareReplacer - Match using surrounding context lines

mod cascade;
mod error;
mod helpers;
mod legacy;
mod strategies;
mod traits;

// Re-export error types
pub use error::EditError;

// Re-export core trait
pub use traits::EditStrategy;

// Re-export all strategy implementations
pub use strategies::{
    BlockAnchorReplacer, ContextAwareReplacer, EscapeNormalizedReplacer,
    IndentationFlexibleReplacer, LineTrimmedReplacer, SimpleReplacer, TrimmedBoundaryReplacer,
    WhitespaceNormalizedReplacer,
};

// Re-export cascade types
pub use cascade::{CascadeReplacer, CascadeResult};

// Re-export legacy compatibility types
pub use legacy::{FuzzyMatcher, MatchError, MatchResult, Strategy, fuzzy_replace};
