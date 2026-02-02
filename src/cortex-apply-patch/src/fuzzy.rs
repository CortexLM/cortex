//! Fuzzy matching for patch application.
//!
//! This module provides fuzzy matching capabilities to handle cases where
//! the patch context doesn't exactly match the file content, such as when:
//! - Lines have been moved slightly
//! - Whitespace has changed
//! - Minor edits have been made

use similar::{ChangeTag, TextDiff};

/// Configuration for fuzzy matching.
#[derive(Debug, Clone)]
pub struct FuzzyConfig {
    /// Maximum number of lines to search from the expected position.
    pub max_offset: usize,
    /// Minimum similarity ratio (0.0 to 1.0) for a line to be considered a match.
    pub min_similarity: f64,
    /// Whether to ignore whitespace differences.
    pub ignore_whitespace: bool,
    /// Whether to ignore case differences.
    pub ignore_case: bool,
}

impl Default for FuzzyConfig {
    fn default() -> Self {
        Self {
            max_offset: 100,
            min_similarity: 0.8,
            ignore_whitespace: true,
            ignore_case: false,
        }
    }
}

/// Fuzzy matcher for finding hunk positions.
#[derive(Debug, Clone)]
pub struct FuzzyMatcher {
    config: FuzzyConfig,
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new(FuzzyConfig::default())
    }
}

impl FuzzyMatcher {
    /// Create a new fuzzy matcher with the given configuration.
    pub fn new(config: FuzzyConfig) -> Self {
        Self { config }
    }

    /// Find the best position to apply a hunk.
    ///
    /// Returns the 0-indexed line number where the hunk should be applied,
    /// or None if no suitable position was found.
    pub fn find_position(
        &self,
        file_lines: &[String],
        match_lines: &[&str],
        suggested_start: usize,
    ) -> Option<(usize, MatchQuality)> {
        if match_lines.is_empty() {
            return Some((suggested_start, MatchQuality::Exact));
        }

        // Try exact match at suggested position first
        if self.matches_exactly(file_lines, match_lines, suggested_start) {
            return Some((suggested_start, MatchQuality::Exact));
        }

        // Try exact match at nearby positions
        for offset in 1..=self.config.max_offset {
            // Try before suggested position
            if suggested_start >= offset {
                let pos = suggested_start - offset;
                if self.matches_exactly(file_lines, match_lines, pos) {
                    return Some((pos, MatchQuality::Offset(offset as isize)));
                }
            }

            // Try after suggested position
            let pos = suggested_start + offset;
            if pos < file_lines.len() && self.matches_exactly(file_lines, match_lines, pos) {
                return Some((pos, MatchQuality::Offset(offset as isize)));
            }
        }

        // If no exact match, try fuzzy matching
        if self.config.min_similarity < 1.0
            && let Some((pos, quality)) =
                self.find_fuzzy_position(file_lines, match_lines, suggested_start)
        {
            return Some((pos, quality));
        }

        None
    }

    /// Check if lines match exactly at a given position.
    fn matches_exactly(&self, file_lines: &[String], match_lines: &[&str], start: usize) -> bool {
        if start + match_lines.len() > file_lines.len() {
            return false;
        }

        for (i, expected) in match_lines.iter().enumerate() {
            let actual = &file_lines[start + i];
            if !self.lines_equal(expected, actual) {
                return false;
            }
        }

        true
    }

    /// Check if two lines are equal according to the fuzzy config.
    fn lines_equal(&self, expected: &str, actual: &str) -> bool {
        let expected = if self.config.ignore_whitespace {
            expected.trim()
        } else {
            expected
        };

        let actual = if self.config.ignore_whitespace {
            actual.trim()
        } else {
            actual
        };

        if self.config.ignore_case {
            expected.eq_ignore_ascii_case(actual)
        } else {
            expected == actual
        }
    }

    /// Find the best fuzzy match position.
    fn find_fuzzy_position(
        &self,
        file_lines: &[String],
        match_lines: &[&str],
        suggested_start: usize,
    ) -> Option<(usize, MatchQuality)> {
        let mut best_pos = None;
        let mut best_score = 0.0;

        let search_start = suggested_start.saturating_sub(self.config.max_offset);
        let search_end = (suggested_start + self.config.max_offset).min(file_lines.len());

        for pos in search_start..search_end {
            if pos + match_lines.len() > file_lines.len() {
                continue;
            }

            let score = self.calculate_match_score(file_lines, match_lines, pos);
            if score > best_score && score >= self.config.min_similarity {
                best_score = score;
                best_pos = Some(pos);
            }
        }

        best_pos.map(|pos| (pos, MatchQuality::Fuzzy(best_score)))
    }

    /// Calculate the match score for lines at a given position.
    fn calculate_match_score(
        &self,
        file_lines: &[String],
        match_lines: &[&str],
        start: usize,
    ) -> f64 {
        if match_lines.is_empty() {
            return 1.0;
        }

        let mut total_score = 0.0;

        for (i, expected) in match_lines.iter().enumerate() {
            let actual = &file_lines[start + i];
            total_score += self.line_similarity(expected, actual);
        }

        total_score / match_lines.len() as f64
    }

    /// Calculate similarity between two lines (0.0 to 1.0).
    fn line_similarity(&self, expected: &str, actual: &str) -> f64 {
        let expected = if self.config.ignore_whitespace {
            expected.trim()
        } else {
            expected
        };

        let actual = if self.config.ignore_whitespace {
            actual.trim()
        } else {
            actual
        };

        let expected = if self.config.ignore_case {
            expected.to_lowercase()
        } else {
            expected.to_string()
        };

        let actual = if self.config.ignore_case {
            actual.to_lowercase()
        } else {
            actual.to_string()
        };

        if expected == actual {
            return 1.0;
        }

        if expected.is_empty() || actual.is_empty() {
            return if expected.is_empty() && actual.is_empty() {
                1.0
            } else {
                0.0
            };
        }

        // Use the similar crate for text diff based similarity
        let diff = TextDiff::from_chars(&expected, &actual);
        let mut same_count = 0;
        let mut total_count = 0;

        for change in diff.iter_all_changes() {
            match change.tag() {
                ChangeTag::Equal => {
                    same_count += 1;
                    total_count += 1;
                }
                _ => {
                    total_count += 1;
                }
            }
        }

        if total_count == 0 {
            1.0
        } else {
            same_count as f64 / total_count as f64
        }
    }

    /// Find lines that were potentially moved within the file.
    ///
    /// Returns a mapping from old line positions to new line positions.
    pub fn find_moved_lines(
        &self,
        original_lines: &[&str],
        current_lines: &[String],
    ) -> Vec<(usize, usize)> {
        let mut moves = Vec::new();

        for (orig_idx, orig_line) in original_lines.iter().enumerate() {
            // Skip empty lines and very short lines
            if orig_line.trim().len() < 3 {
                continue;
            }

            // Try to find this line in the current file
            for (curr_idx, curr_line) in current_lines.iter().enumerate() {
                if self.lines_equal(orig_line, curr_line) && orig_idx != curr_idx {
                    moves.push((orig_idx, curr_idx));
                    break;
                }
            }
        }

        moves
    }
}

/// Quality of a match found by the fuzzy matcher.
#[derive(Debug, Clone, PartialEq)]
pub enum MatchQuality {
    /// Exact match at the expected position.
    Exact,
    /// Exact match at an offset from the expected position.
    Offset(isize),
    /// Fuzzy match with the given similarity score.
    Fuzzy(f64),
}

impl MatchQuality {
    /// Check if this is an exact match.
    pub fn is_exact(&self) -> bool {
        matches!(self, Self::Exact)
    }

    /// Get a quality score (1.0 for exact, lower for fuzzy).
    pub fn score(&self) -> f64 {
        match self {
            Self::Exact => 1.0,
            Self::Offset(offset) => 1.0 - (offset.unsigned_abs() as f64 * 0.001).min(0.1),
            Self::Fuzzy(similarity) => *similarity,
        }
    }
}

/// Find the longest common subsequence between two slices.
#[allow(dead_code)]
pub fn longest_common_subsequence<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();

    if m == 0 || n == 0 {
        return Vec::new();
    }

    // Build LCS length table
    let mut dp = vec![vec![0usize; n + 1]; m + 1];

    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }

    // Backtrack to find the actual LCS
    let mut result = Vec::new();
    let mut i = m;
    let mut j = n;

    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }

    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let matcher = FuzzyMatcher::default();
        let file_lines: Vec<String> = vec![
            "line 1".to_string(),
            "line 2".to_string(),
            "line 3".to_string(),
        ];
        let match_lines = vec!["line 2", "line 3"];

        let (pos, quality) = matcher.find_position(&file_lines, &match_lines, 1).unwrap();
        assert_eq!(pos, 1);
        assert!(quality.is_exact());
    }

    #[test]
    fn test_offset_match() {
        let matcher = FuzzyMatcher::default();
        let file_lines: Vec<String> = vec![
            "extra line".to_string(),
            "line 1".to_string(),
            "line 2".to_string(),
            "line 3".to_string(),
        ];
        let match_lines = vec!["line 2", "line 3"];

        let (pos, quality) = matcher.find_position(&file_lines, &match_lines, 1).unwrap();
        assert_eq!(pos, 2);
        assert!(matches!(quality, MatchQuality::Offset(_)));
    }

    #[test]
    fn test_whitespace_ignore() {
        let config = FuzzyConfig {
            ignore_whitespace: true,
            ..Default::default()
        };
        let matcher = FuzzyMatcher::new(config);
        let file_lines: Vec<String> = vec!["  line 1  ".to_string(), "\tline 2".to_string()];
        let match_lines = vec!["line 1", "line 2"];

        let (pos, quality) = matcher.find_position(&file_lines, &match_lines, 0).unwrap();
        assert_eq!(pos, 0);
        assert!(quality.is_exact());
    }

    #[test]
    fn test_fuzzy_match() {
        let config = FuzzyConfig {
            min_similarity: 0.7,
            ..Default::default()
        };
        let matcher = FuzzyMatcher::new(config);
        let file_lines: Vec<String> =
            vec!["this is a line".to_string(), "this is line 2".to_string()];
        let match_lines = vec!["this is a line", "this is also line 2"];

        let result = matcher.find_position(&file_lines, &match_lines, 0);
        assert!(result.is_some());
    }

    #[test]
    fn test_no_match() {
        let matcher = FuzzyMatcher::default();
        let file_lines: Vec<String> = vec!["completely different".to_string()];
        let match_lines = vec!["expected line"];

        let result = matcher.find_position(&file_lines, &match_lines, 0);
        assert!(result.is_none());
    }

    #[test]
    fn test_line_similarity() {
        let matcher = FuzzyMatcher::default();

        // Exact match
        assert!((matcher.line_similarity("hello", "hello") - 1.0).abs() < 0.001);

        // Very different
        assert!(matcher.line_similarity("hello", "world") < 0.5);

        // Similar
        assert!(matcher.line_similarity("hello world", "hello worlds") > 0.8);
    }

    #[test]
    fn test_lcs() {
        let a = vec!["a", "b", "c", "d", "e"];
        let b = vec!["x", "a", "c", "d", "y"];

        let lcs = longest_common_subsequence(&a, &b);

        // LCS should be [(0,1), (2,2), (3,3)] corresponding to "a", "c", "d"
        assert_eq!(lcs.len(), 3);
    }

    #[test]
    fn test_match_quality_score() {
        assert!((MatchQuality::Exact.score() - 1.0).abs() < 0.001);
        assert!(MatchQuality::Offset(10).score() < 1.0);
        assert!(MatchQuality::Fuzzy(0.8).score() < 1.0);
    }
}
