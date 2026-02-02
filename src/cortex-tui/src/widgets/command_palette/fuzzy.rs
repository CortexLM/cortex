//! Fuzzy matching for the command palette.
//!
//! Provides fuzzy scoring algorithms for matching user input against palette items.

/// Computes a fuzzy match score for pattern against text.
///
/// Returns `Some(score)` if the pattern matches, `None` otherwise.
/// Higher scores indicate better matches.
pub fn fuzzy_score(pattern: &str, text: &str) -> Option<i32> {
    if pattern.is_empty() {
        return Some(0);
    }

    // Exact match - highest score
    if text == pattern {
        return Some(1000);
    }

    // Prefix match - high score
    if text.starts_with(pattern) {
        return Some(500 + (100 - text.len().min(100) as i32));
    }

    // Contains - medium score
    if text.contains(pattern) {
        let pos = text.find(pattern).unwrap_or(0);
        return Some(200 - pos.min(100) as i32);
    }

    // Subsequence match - lower score
    let mut pattern_chars = pattern.chars().peekable();
    let mut score = 0;
    let mut consecutive = 0;
    let mut last_match_pos: Option<usize> = None;

    for (i, c) in text.chars().enumerate() {
        if pattern_chars.peek() == Some(&c) {
            pattern_chars.next();

            // Bonus for consecutive matches
            if let Some(last) = last_match_pos {
                if last + 1 == i {
                    consecutive += 1;
                    score += 10 + consecutive * 5;
                } else {
                    consecutive = 0;
                    score += 10;
                }
            } else {
                consecutive = 0;
                score += 10;
            }

            // Bonus for matching at word boundary
            if i == 0 {
                score += 20;
            } else if let Some(prev_char) = text.chars().nth(i - 1)
                && (prev_char == '_' || prev_char == '-' || prev_char == ' ')
            {
                score += 20;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_score_exact_match() {
        assert_eq!(fuzzy_score("help", "help"), Some(1000));
    }

    #[test]
    fn test_fuzzy_score_prefix_match() {
        let score = fuzzy_score("hel", "help").unwrap();
        assert!((500..1000).contains(&score));
    }

    #[test]
    fn test_fuzzy_score_contains() {
        let score = fuzzy_score("mod", "model").unwrap();
        // "mod" at start of "model" gets prefix bonus (500+)
        assert!(score >= 500);
    }

    #[test]
    fn test_fuzzy_score_subsequence() {
        // "hl" in "help" - h at pos 0, l at pos 3 (not consecutive)
        let score = fuzzy_score("hl", "help").unwrap();
        // h gets 10 + 20 (word boundary), l gets 10 = 40
        assert!(score > 0);
    }

    #[test]
    fn test_fuzzy_score_no_match() {
        assert!(fuzzy_score("xyz", "help").is_none());
    }

    #[test]
    fn test_fuzzy_score_empty_pattern() {
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }
}
