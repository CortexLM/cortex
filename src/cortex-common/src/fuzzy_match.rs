//! Fuzzy string matching utilities.

/// Simple fuzzy match score.
/// Returns a score between 0 and 100, where 100 is an exact match.
pub fn fuzzy_score(pattern: &str, text: &str) -> u32 {
    if pattern.is_empty() {
        return 100;
    }
    if text.is_empty() {
        return 0;
    }

    let pattern_lower = pattern.to_lowercase();
    let text_lower = text.to_lowercase();

    // Exact match
    if text_lower == pattern_lower {
        return 100;
    }

    // Starts with
    if text_lower.starts_with(&pattern_lower) {
        return 90;
    }

    // Contains
    if text_lower.contains(&pattern_lower) {
        return 70;
    }

    // Character-by-character fuzzy match
    let mut pattern_idx = 0;
    let pattern_chars: Vec<char> = pattern_lower.chars().collect();
    let mut matches = 0;
    let mut consecutive = 0;
    let mut max_consecutive = 0;

    for c in text_lower.chars() {
        if pattern_idx < pattern_chars.len() && c == pattern_chars[pattern_idx] {
            matches += 1;
            consecutive += 1;
            max_consecutive = max_consecutive.max(consecutive);
            pattern_idx += 1;
        } else {
            consecutive = 0;
        }
    }

    if pattern_idx == pattern_chars.len() {
        // All pattern characters found in order
        let base_score = 50;
        let match_bonus = (matches as f32 / text.len() as f32 * 20.0) as u32;
        let consecutive_bonus = (max_consecutive as f32 / pattern.len() as f32 * 10.0) as u32;
        (base_score + match_bonus + consecutive_bonus).min(69)
    } else {
        0
    }
}

/// Filter and sort items by fuzzy match score.
pub fn fuzzy_filter<T, F>(items: &[T], pattern: &str, get_text: F) -> Vec<(usize, u32)>
where
    F: Fn(&T) -> &str,
{
    let mut scored: Vec<(usize, u32)> = items
        .iter()
        .enumerate()
        .map(|(i, item)| (i, fuzzy_score(pattern, get_text(item))))
        .filter(|(_, score)| *score > 0)
        .collect();

    scored.sort_by(|a, b| b.1.cmp(&a.1));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuzzy_score_exact() {
        assert_eq!(fuzzy_score("test", "test"), 100);
        assert_eq!(fuzzy_score("TEST", "test"), 100);
    }

    #[test]
    fn test_fuzzy_score_starts_with() {
        let score = fuzzy_score("te", "test");
        assert!(score >= 90);
    }

    #[test]
    fn test_fuzzy_score_contains() {
        let score = fuzzy_score("es", "test");
        assert!(score >= 70);
    }

    #[test]
    fn test_fuzzy_score_no_match() {
        assert_eq!(fuzzy_score("xyz", "test"), 0);
    }

    #[test]
    fn test_fuzzy_filter() {
        let items = vec!["apple", "application", "banana", "apply"];
        let results = fuzzy_filter(&items, "app", |s| s);

        assert_eq!(results.len(), 3);
        // "apple" and "apply" should score higher than "application"
        assert!(results.iter().any(|(i, _)| items[*i] == "apple"));
    }
}
