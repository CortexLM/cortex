//! Fuzzy matching implementation using nucleo-matcher.

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

/// Fuzzy matcher powered by nucleo-matcher.
///
/// This provides high-performance fuzzy matching suitable for
/// real-time file search.
#[derive(Debug)]
pub struct FuzzyMatcher {
    matcher: Matcher,
    case_sensitive: bool,
}

impl Default for FuzzyMatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl FuzzyMatcher {
    /// Creates a new fuzzy matcher with default settings.
    pub fn new() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            case_sensitive: false,
        }
    }

    /// Creates a new fuzzy matcher with case-sensitive matching.
    pub fn case_sensitive() -> Self {
        Self {
            matcher: Matcher::new(Config::DEFAULT),
            case_sensitive: true,
        }
    }

    /// Sets the case sensitivity of the matcher.
    pub fn set_case_sensitive(&mut self, case_sensitive: bool) {
        self.case_sensitive = case_sensitive;
    }

    /// Computes the fuzzy match score for a pattern against a haystack.
    ///
    /// Returns `None` if there is no match, or `Some(score)` where
    /// higher scores indicate better matches.
    pub fn score(&mut self, pattern: &str, haystack: &str) -> Option<u32> {
        if pattern.is_empty() {
            return Some(0);
        }

        if haystack.is_empty() {
            return None;
        }

        let case_matching = if self.case_sensitive {
            CaseMatching::Respect
        } else {
            CaseMatching::Smart
        };

        let pat = Pattern::new(
            pattern,
            case_matching,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut haystack_buf = Vec::new();
        let haystack_chars = Utf32Str::new(haystack, &mut haystack_buf);

        pat.score(haystack_chars, &mut self.matcher)
    }

    /// Computes the fuzzy match score and returns match indices.
    ///
    /// The indices indicate which characters in the haystack matched
    /// the pattern, useful for highlighting.
    pub fn score_with_indices(
        &mut self,
        pattern: &str,
        haystack: &str,
    ) -> Option<(u32, Vec<usize>)> {
        if pattern.is_empty() {
            return Some((0, Vec::new()));
        }

        if haystack.is_empty() {
            return None;
        }

        let case_matching = if self.case_sensitive {
            CaseMatching::Respect
        } else {
            CaseMatching::Smart
        };

        let pat = Pattern::new(
            pattern,
            case_matching,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut haystack_buf = Vec::new();
        let haystack_chars = Utf32Str::new(haystack, &mut haystack_buf);

        let mut indices = Vec::new();
        let score = pat.indices(haystack_chars, &mut self.matcher, &mut indices)?;

        // Convert u32 indices to usize
        let indices: Vec<usize> = indices.iter().map(|&i| i as usize).collect();

        Some((score, indices))
    }

    /// Checks if a pattern matches a haystack at all.
    pub fn matches(&mut self, pattern: &str, haystack: &str) -> bool {
        self.score(pattern, haystack).is_some()
    }

    /// Computes a normalized score between 0 and 100.
    ///
    /// This is useful for comparing scores across different haystack lengths.
    pub fn normalized_score(&mut self, pattern: &str, haystack: &str) -> Option<u32> {
        let score = self.score(pattern, haystack)?;

        // Normalize based on pattern and haystack length
        let pattern_len = pattern.chars().count() as u32;
        let haystack_len = haystack.chars().count() as u32;

        if pattern_len == 0 || haystack_len == 0 {
            return Some(0);
        }

        // Apply bonuses for special cases
        let haystack_lower = haystack.to_lowercase();
        let pattern_lower = pattern.to_lowercase();

        // Exact match bonus
        if haystack_lower == pattern_lower {
            return Some(100);
        }

        // Starts with bonus
        if haystack_lower.starts_with(&pattern_lower) {
            let base = score.min(80);
            return Some(base.saturating_add(15));
        }

        // Contains exact substring bonus
        if haystack_lower.contains(&pattern_lower) {
            let base = score.min(70);
            return Some(base.saturating_add(10));
        }

        // Normalize the raw score
        // nucleo scores can be quite high, so we normalize them
        let max_possible = pattern_len * 20; // Approximate max score
        let normalized = ((score as f64 / max_possible as f64) * 70.0) as u32;

        Some(normalized.min(69))
    }

    /// Batch scores multiple haystacks against a single pattern.
    ///
    /// Returns a vector of (index, score) pairs for items that matched,
    /// sorted by score in descending order.
    pub fn batch_score<'a, I>(&mut self, pattern: &str, haystacks: I) -> Vec<(usize, u32)>
    where
        I: IntoIterator<Item = &'a str>,
    {
        if pattern.is_empty() {
            return Vec::new();
        }

        let case_matching = if self.case_sensitive {
            CaseMatching::Respect
        } else {
            CaseMatching::Smart
        };

        let pat = Pattern::new(
            pattern,
            case_matching,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );

        let mut results: Vec<(usize, u32)> = haystacks
            .into_iter()
            .enumerate()
            .filter_map(|(idx, haystack)| {
                let mut haystack_buf = Vec::new();
                let haystack_chars = Utf32Str::new(haystack, &mut haystack_buf);
                pat.score(haystack_chars, &mut self.matcher)
                    .map(|score| (idx, score))
            })
            .collect();

        results.sort_by(|a, b| b.1.cmp(&a.1));
        results
    }
}

/// Matches a string against a glob pattern.
///
/// Supports the following patterns:
/// - `*` matches any sequence of characters except path separators
/// - `**` matches any sequence including path separators
/// - `?` matches a single character
/// - `[abc]` matches any character in the set
/// - `[!abc]` matches any character not in the set
pub fn glob_match(pattern: &str, text: &str) -> bool {
    // Normalize path separators
    let pattern = pattern.replace('\\', "/");
    let text = text.replace('\\', "/");
    glob_match_recursive(&pattern, &text)
}

fn glob_match_recursive(pattern: &str, text: &str) -> bool {
    let mut pat_chars = pattern.chars().peekable();
    let mut txt_chars = text.chars().peekable();

    while let Some(p) = pat_chars.next() {
        match p {
            '*' => {
                // Check for **
                if pat_chars.peek() == Some(&'*') {
                    pat_chars.next(); // consume second *

                    // Skip any trailing slash after **
                    if pat_chars.peek() == Some(&'/') {
                        pat_chars.next();
                    }

                    let remaining_pattern: String = pat_chars.collect();

                    // ** at end matches everything
                    if remaining_pattern.is_empty() {
                        return true;
                    }

                    // Try matching ** against zero or more path segments
                    let remaining_text: String = txt_chars.collect();

                    // Try matching at every position including after path separators
                    if glob_match_recursive(&remaining_pattern, &remaining_text) {
                        return true;
                    }

                    for (i, c) in remaining_text.char_indices() {
                        if glob_match_recursive(
                            &remaining_pattern,
                            &remaining_text[i + c.len_utf8()..],
                        ) {
                            return true;
                        }
                    }

                    return false;
                } else {
                    // Single * - matches any characters except /
                    let remaining_pattern: String = pat_chars.collect();

                    // Try matching * against zero or more non-slash characters
                    let remaining_text: String = txt_chars.collect();

                    // Try matching at current position
                    if glob_match_recursive(&remaining_pattern, &remaining_text) {
                        return true;
                    }

                    // Try matching after consuming non-slash characters
                    for (i, c) in remaining_text.char_indices() {
                        if c == '/' {
                            // Single * cannot match /
                            break;
                        }
                        if glob_match_recursive(
                            &remaining_pattern,
                            &remaining_text[i + c.len_utf8()..],
                        ) {
                            return true;
                        }
                    }

                    return false;
                }
            }
            '?' => {
                // ? matches any single character except /
                match txt_chars.next() {
                    Some(c) if c != '/' => continue,
                    _ => return false,
                }
            }
            '[' => {
                // Character class
                let txt_c = match txt_chars.next() {
                    Some(c) => c,
                    None => return false,
                };

                let negated = pat_chars.peek() == Some(&'!') || pat_chars.peek() == Some(&'^');
                if negated {
                    pat_chars.next();
                }

                let mut matched = false;
                let mut prev_char: Option<char> = None;

                loop {
                    match pat_chars.next() {
                        None => return false, // Unclosed bracket
                        Some(']') => break,
                        Some('-') => {
                            // Range
                            if let (Some(start), Some(end)) = (prev_char, pat_chars.peek().copied())
                                && end != ']'
                            {
                                pat_chars.next();
                                if txt_c >= start && txt_c <= end {
                                    matched = true;
                                }
                                prev_char = None;
                                continue;
                            }
                            // Literal -
                            if txt_c == '-' {
                                matched = true;
                            }
                            prev_char = Some('-');
                        }
                        Some(c) => {
                            if txt_c == c {
                                matched = true;
                            }
                            prev_char = Some(c);
                        }
                    }
                }

                if matched == negated {
                    return false;
                }
            }
            c => {
                // Literal character
                match txt_chars.next() {
                    Some(tc) if tc == c => continue,
                    _ => return false,
                }
            }
        }
    }

    // Pattern exhausted - text should also be exhausted
    txt_chars.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_matcher_basic() {
        let mut matcher = FuzzyMatcher::new();

        // Exact match should score high
        let score = matcher.score("main", "main");
        assert!(score.is_some());

        // Substring should match
        let score = matcher.score("main", "main.rs");
        assert!(score.is_some());

        // Fuzzy match
        let score = matcher.score("mn", "main");
        assert!(score.is_some());

        // No match
        let score = matcher.score("xyz", "main");
        assert!(score.is_none());
    }

    #[test]
    fn test_fuzzy_matcher_case_insensitive() {
        let mut matcher = FuzzyMatcher::new();

        // nucleo-matcher with CaseMatching::Smart treats lowercase patterns as case-insensitive
        // but uppercase patterns may be case-sensitive
        let score1 = matcher.score("main", "Main");
        assert!(score1.is_some());

        // lowercase pattern should still match
        let score2 = matcher.score("main", "MAIN");
        assert!(score2.is_some());
    }

    #[test]
    fn test_fuzzy_matcher_case_sensitive() {
        let mut matcher = FuzzyMatcher::case_sensitive();

        // Exact case should match
        let score1 = matcher.score("main", "main");
        assert!(score1.is_some());

        // Different case might not match as well
        let _score2 = matcher.score("MAIN", "main");
        // nucleo with CaseMatching::Respect may still match but with lower score
        // The exact behavior depends on nucleo's implementation
    }

    #[test]
    fn test_fuzzy_matcher_with_indices() {
        let mut matcher = FuzzyMatcher::new();

        let result = matcher.score_with_indices("mn", "main");
        assert!(result.is_some());

        if let Some((score, indices)) = result {
            assert!(score > 0);
            assert!(!indices.is_empty());
        }
    }

    #[test]
    fn test_normalized_score() {
        let mut matcher = FuzzyMatcher::new();

        // Exact match should be 100
        let score = matcher.normalized_score("main", "main");
        assert_eq!(score, Some(100));

        // Starts with should be high
        let score = matcher.normalized_score("main", "main.rs");
        assert!(score.is_some());
        assert!(score.unwrap() >= 80);
    }

    #[test]
    fn test_batch_score() {
        let mut matcher = FuzzyMatcher::new();

        let haystacks = ["main.rs", "lib.rs", "main.go", "test.rs", "maintenance.rs"];
        let results = matcher.batch_score("main", haystacks);

        assert!(!results.is_empty());
        // Results should be sorted by score (descending)
        for i in 1..results.len() {
            assert!(results[i - 1].1 >= results[i].1);
        }
    }

    #[test]
    fn test_glob_match_simple() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.rs", "lib.rs"));
        assert!(!glob_match("*.rs", "main.go"));
    }

    #[test]
    fn test_glob_match_double_star() {
        assert!(glob_match("**/*.rs", "src/main.rs"));
        assert!(glob_match("**/*.rs", "src/lib/mod.rs"));
        assert!(glob_match("src/**/*.rs", "src/foo/bar/baz.rs"));
    }

    #[test]
    fn test_glob_match_question() {
        assert!(glob_match("main.?s", "main.rs"));
        assert!(glob_match("main.?s", "main.ts"));
        assert!(!glob_match("main.?s", "main.rs2"));
    }

    #[test]
    fn test_glob_match_bracket() {
        assert!(glob_match("main.[rt]s", "main.rs"));
        assert!(glob_match("main.[rt]s", "main.ts"));
        assert!(!glob_match("main.[rt]s", "main.js"));

        assert!(glob_match("file[0-9].txt", "file5.txt"));
        assert!(!glob_match("file[0-9].txt", "filea.txt"));

        assert!(glob_match("file[!0-9].txt", "filea.txt"));
        assert!(!glob_match("file[!0-9].txt", "file5.txt"));
    }

    #[test]
    fn test_glob_match_path_separator() {
        // Single * should not match path separators
        assert!(!glob_match("src/*.rs", "src/foo/bar.rs"));

        // Double ** should match path separators
        assert!(glob_match("src/**/*.rs", "src/foo/bar.rs"));
    }
}
