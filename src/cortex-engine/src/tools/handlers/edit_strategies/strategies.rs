//! Individual edit replacement strategy implementations.

use tracing::debug;

use super::helpers::{
    adjust_indentation, calculate_block_similarity, find_context_match, get_leading_whitespace,
    line_end_byte, line_start_byte, normalize_escapes, normalize_indentation, normalize_whitespace,
    trim_each_line, truncate_string,
};
use super::traits::EditStrategy;

// =============================================================================
// Strategy 1: SimpleReplacer - Exact match
// =============================================================================

/// Strategy 1: Simple exact string replacement.
/// This is the most precise strategy - requires exact match.
pub struct SimpleReplacer;

impl EditStrategy for SimpleReplacer {
    fn name(&self) -> &'static str {
        "SimpleReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        if content.contains(old) {
            Some(content.replacen(old, new, 1))
        } else {
            None
        }
    }

    fn try_replace_all(&self, content: &str, old: &str, new: &str) -> Option<String> {
        if content.contains(old) {
            Some(content.replace(old, new))
        } else {
            None
        }
    }

    fn confidence(&self) -> f64 {
        1.0
    }
}

// =============================================================================
// Strategy 2: LineTrimmedReplacer - Trim each line
// =============================================================================

/// Strategy 2: Line-trimmed matching.
/// Trims whitespace from start/end of each line before comparing.
pub struct LineTrimmedReplacer;

impl EditStrategy for LineTrimmedReplacer {
    fn name(&self) -> &'static str {
        "LineTrimmedReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        let old_trimmed = trim_each_line(old);
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_trimmed.lines().collect();

        if old_lines.is_empty() {
            return None;
        }

        // Find matching block in content
        for start_idx in 0..=content_lines.len().saturating_sub(old_lines.len()) {
            let mut matches = true;
            for (j, old_line) in old_lines.iter().enumerate() {
                let content_line = content_lines.get(start_idx + j)?;
                if content_line.trim() != *old_line {
                    matches = false;
                    break;
                }
            }

            if matches {
                // Found a match - reconstruct with original indentation preserved
                let matched_lines = &content_lines[start_idx..start_idx + old_lines.len()];
                let matched_text = matched_lines.join("\n");

                // Calculate byte positions
                let start_byte = line_start_byte(content, start_idx);
                let end_byte = line_end_byte(content, start_idx + old_lines.len() - 1);

                let mut result = String::with_capacity(content.len());
                result.push_str(&content[..start_byte]);
                result.push_str(new);
                result.push_str(&content[end_byte..]);

                debug!(
                    "LineTrimmedReplacer: matched '{}' at lines {}-{}",
                    truncate_string(&matched_text, 30),
                    start_idx + 1,
                    start_idx + old_lines.len()
                );

                return Some(result);
            }
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.95
    }
}

// =============================================================================
// Strategy 3: BlockAnchorReplacer - Match by anchors
// =============================================================================

/// Strategy 3: Block anchor matching.
/// Uses the first and last non-empty lines as anchors to find the block.
pub struct BlockAnchorReplacer;

impl EditStrategy for BlockAnchorReplacer {
    fn name(&self) -> &'static str {
        "BlockAnchorReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        let old_lines: Vec<&str> = old.lines().collect();
        if old_lines.len() < 2 {
            return None;
        }

        // Find first and last non-empty lines as anchors
        let first_anchor = old_lines.iter().find(|l| !l.trim().is_empty())?.trim();
        let last_anchor = old_lines
            .iter()
            .rev()
            .find(|l| !l.trim().is_empty())?
            .trim();

        if first_anchor == last_anchor {
            return None; // Need distinct anchors
        }

        let content_lines: Vec<&str> = content.lines().collect();

        // Find all possible first anchor positions
        let first_positions: Vec<usize> = content_lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.trim() == first_anchor)
            .map(|(i, _)| i)
            .collect();

        // Find all possible last anchor positions
        let last_positions: Vec<usize> = content_lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.trim() == last_anchor)
            .map(|(i, _)| i)
            .collect();

        // Find valid anchor pairs (first before last)
        for &first_pos in &first_positions {
            for &last_pos in &last_positions {
                if last_pos > first_pos {
                    // Validate block size is reasonable
                    let block_size = last_pos - first_pos + 1;
                    let expected_size = old_lines.len();

                    // Allow some flexibility in block size (within 50%)
                    if block_size >= expected_size.saturating_sub(expected_size / 2)
                        && block_size <= expected_size + expected_size / 2
                    {
                        // Verify middle content similarity
                        let content_block = &content_lines[first_pos..=last_pos];
                        let similarity = calculate_block_similarity(&old_lines, content_block);

                        if similarity >= 0.5 {
                            let start_byte = line_start_byte(content, first_pos);
                            let end_byte = line_end_byte(content, last_pos);

                            let mut result = String::with_capacity(content.len());
                            result.push_str(&content[..start_byte]);
                            result.push_str(new);
                            result.push_str(&content[end_byte..]);

                            debug!(
                                "BlockAnchorReplacer: matched block at lines {}-{} (similarity: {:.1}%)",
                                first_pos + 1,
                                last_pos + 1,
                                similarity * 100.0
                            );

                            return Some(result);
                        }
                    }
                }
            }
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.85
    }
}

// =============================================================================
// Strategy 4: WhitespaceNormalizedReplacer - Normalize whitespace
// =============================================================================

/// Strategy 4: Whitespace-normalized matching.
/// Normalizes multiple whitespace characters to a single space.
pub struct WhitespaceNormalizedReplacer;

impl EditStrategy for WhitespaceNormalizedReplacer {
    fn name(&self) -> &'static str {
        "WhitespaceNormalizedReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        let old_normalized = normalize_whitespace(old);
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old.lines().collect();

        if old_lines.is_empty() {
            return None;
        }

        // Try to find a matching block by normalizing each potential window
        for start_idx in 0..=content_lines.len().saturating_sub(old_lines.len()) {
            let window = content_lines[start_idx..start_idx + old_lines.len()].join("\n");
            let window_normalized = normalize_whitespace(&window);

            if window_normalized == old_normalized {
                let start_byte = line_start_byte(content, start_idx);
                let end_byte = line_end_byte(content, start_idx + old_lines.len() - 1);

                let mut result = String::with_capacity(content.len());
                result.push_str(&content[..start_byte]);
                result.push_str(new);
                result.push_str(&content[end_byte..]);

                debug!(
                    "WhitespaceNormalizedReplacer: matched at lines {}-{}",
                    start_idx + 1,
                    start_idx + old_lines.len()
                );

                return Some(result);
            }
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.80
    }
}

// =============================================================================
// Strategy 5: IndentationFlexibleReplacer - Ignore indentation
// =============================================================================

/// Strategy 5: Indentation-flexible matching.
/// Matches content regardless of indentation differences (tabs vs spaces, amount).
pub struct IndentationFlexibleReplacer;

impl EditStrategy for IndentationFlexibleReplacer {
    fn name(&self) -> &'static str {
        "IndentationFlexibleReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        let old_normalized = normalize_indentation(old);
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old.lines().collect();

        if old_lines.is_empty() {
            return None;
        }

        for start_idx in 0..=content_lines.len().saturating_sub(old_lines.len()) {
            let window = content_lines[start_idx..start_idx + old_lines.len()].join("\n");
            let window_normalized = normalize_indentation(&window);

            if window_normalized == old_normalized {
                // Preserve original indentation when replacing
                let original_indent = get_leading_whitespace(content_lines[start_idx]);
                let new_adjusted = adjust_indentation(new, original_indent);

                let start_byte = line_start_byte(content, start_idx);
                let end_byte = line_end_byte(content, start_idx + old_lines.len() - 1);

                let mut result = String::with_capacity(content.len());
                result.push_str(&content[..start_byte]);
                result.push_str(&new_adjusted);
                result.push_str(&content[end_byte..]);

                debug!(
                    "IndentationFlexibleReplacer: matched at lines {}-{}, preserving indent '{}'",
                    start_idx + 1,
                    start_idx + old_lines.len(),
                    original_indent.escape_default()
                );

                return Some(result);
            }
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.75
    }
}

// =============================================================================
// Strategy 6: EscapeNormalizedReplacer - Normalize escape chars
// =============================================================================

/// Strategy 6: Escape-normalized matching.
/// Normalizes escape sequences (\n, \t, \r, etc.) for comparison.
pub struct EscapeNormalizedReplacer;

impl EditStrategy for EscapeNormalizedReplacer {
    fn name(&self) -> &'static str {
        "EscapeNormalizedReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        let old_normalized = normalize_escapes(old);
        let content_normalized = normalize_escapes(content);

        if let Some(pos) = content_normalized.find(&old_normalized) {
            // For escape normalization, we can try to find the match in original content
            // by using a sliding window approach
            let content_lines: Vec<&str> = content.lines().collect();
            let old_lines: Vec<&str> = old.lines().collect();

            if old_lines.is_empty() {
                return None;
            }

            // Try to find matching block using normalized comparison
            for start_idx in 0..=content_lines.len().saturating_sub(old_lines.len()) {
                let window = content_lines[start_idx..start_idx + old_lines.len()].join("\n");
                let window_normalized = normalize_escapes(&window);

                if window_normalized == old_normalized {
                    let start_byte = line_start_byte(content, start_idx);
                    let end_byte = line_end_byte(content, start_idx + old_lines.len() - 1);

                    let mut result = String::with_capacity(content.len());
                    result.push_str(&content[..start_byte]);
                    result.push_str(new);
                    result.push_str(&content[end_byte..]);

                    debug!(
                        "EscapeNormalizedReplacer: matched at lines {}-{}",
                        start_idx + 1,
                        start_idx + old_lines.len()
                    );

                    return Some(result);
                }
            }

            // Fallback: if exact position found in normalized content, use that
            if pos < content.len() && pos + old.len() <= content.len() {
                let mut result = String::with_capacity(content.len());
                result.push_str(&content[..pos]);
                result.push_str(new);
                result.push_str(&content[(pos + old.len()).min(content.len())..]);
                return Some(result);
            }
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.70
    }
}

// =============================================================================
// Strategy 7: TrimmedBoundaryReplacer - Match trimmed with context
// =============================================================================

/// Strategy 7: Trimmed boundary matching.
/// Matches on fully trimmed content but uses boundary context for precision.
pub struct TrimmedBoundaryReplacer;

impl EditStrategy for TrimmedBoundaryReplacer {
    fn name(&self) -> &'static str {
        "TrimmedBoundaryReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        let old_trimmed = old.trim();
        if old_trimmed.is_empty() {
            return None;
        }

        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_trimmed.lines().collect();

        if old_lines.is_empty() {
            return None;
        }

        // Build a signature from trimmed content
        let old_signature: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();

        for start_idx in 0..=content_lines.len().saturating_sub(old_lines.len()) {
            let window = &content_lines[start_idx..start_idx + old_lines.len()];
            let window_signature: Vec<&str> = window.iter().map(|l| l.trim()).collect();

            if window_signature == old_signature {
                let start_byte = line_start_byte(content, start_idx);
                let end_byte = line_end_byte(content, start_idx + old_lines.len() - 1);

                let mut result = String::with_capacity(content.len());
                result.push_str(&content[..start_byte]);
                result.push_str(new);
                result.push_str(&content[end_byte..]);

                debug!(
                    "TrimmedBoundaryReplacer: matched at lines {}-{}",
                    start_idx + 1,
                    start_idx + old_lines.len()
                );

                return Some(result);
            }
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.65
    }
}

// =============================================================================
// Strategy 8: ContextAwareReplacer - Match by surrounding context
// =============================================================================

/// Strategy 8: Context-aware matching.
/// Uses surrounding lines (before/after) to identify the correct match location.
pub struct ContextAwareReplacer {
    context_lines: usize,
}

impl ContextAwareReplacer {
    pub fn new(context_lines: usize) -> Self {
        Self { context_lines }
    }
}

impl Default for ContextAwareReplacer {
    fn default() -> Self {
        Self { context_lines: 2 }
    }
}

impl EditStrategy for ContextAwareReplacer {
    fn name(&self) -> &'static str {
        "ContextAwareReplacer"
    }

    fn try_replace(&self, content: &str, old: &str, new: &str) -> Option<String> {
        if let Some(range) = find_context_match(content, old, self.context_lines) {
            let mut result = String::with_capacity(content.len());
            result.push_str(&content[..range.start]);
            result.push_str(new);
            result.push_str(&content[range.end..]);

            debug!(
                "ContextAwareReplacer: matched at bytes {}-{} using {} context lines",
                range.start, range.end, self.context_lines
            );

            return Some(result);
        }

        None
    }

    fn confidence(&self) -> f64 {
        0.60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Strategy 1: SimpleReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_simple_replacer_exact_match() {
        let replacer = SimpleReplacer;
        let content = "fn main() {\n    println!(\"Hello\");\n}";
        let result = replacer.try_replace(content, "println!(\"Hello\");", "println!(\"World\");");

        assert!(result.is_some());
        let new_content = result.unwrap();
        assert!(new_content.contains("World"));
        assert!(!new_content.contains("Hello"));
    }

    #[test]
    fn test_simple_replacer_no_match() {
        let replacer = SimpleReplacer;
        let content = "fn main() {}";
        let result = replacer.try_replace(content, "nonexistent", "replacement");

        assert!(result.is_none());
    }

    #[test]
    fn test_simple_replacer_replace_all() {
        let replacer = SimpleReplacer;
        let content = "let x = 1;\nlet y = 1;\nlet z = 1;";
        let result = replacer.try_replace_all(content, "1", "2");

        assert!(result.is_some());
        let new_content = result.unwrap();
        assert_eq!(new_content.matches("2").count(), 3);
        assert_eq!(new_content.matches("1").count(), 0);
    }

    // -------------------------------------------------------------------------
    // Strategy 2: LineTrimmedReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_line_trimmed_handles_extra_spaces() {
        let replacer = LineTrimmedReplacer;
        let content = "fn main() {\n    let x = 1;    \n    let y = 2;\n}";
        let search = "let x = 1;\nlet y = 2;";

        let result = replacer.try_replace(content, search, "let z = 3;");

        assert!(result.is_some());
        let new_content = result.unwrap();
        assert!(new_content.contains("let z = 3;"));
    }

    #[test]
    fn test_line_trimmed_preserves_structure() {
        let replacer = LineTrimmedReplacer;
        let content = "  line1  \n  line2  ";
        let search = "line1\nline2";

        let result = replacer.try_replace(content, search, "new1\nnew2");

        assert!(result.is_some());
    }

    // -------------------------------------------------------------------------
    // Strategy 3: BlockAnchorReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_block_anchor_finds_by_anchors() {
        let replacer = BlockAnchorReplacer;
        let content = r#"fn process() {
    // Start processing
    let data = load();
    process_data(data);
    // End processing
}"#;

        let search = r#"// Start processing
    let data = load();
    process_data(data);
    // End processing"#;

        let result = replacer.try_replace(content, search, "// New implementation\ndo_stuff();");

        // BlockAnchorReplacer should find the block using anchors
        assert!(
            result.is_some(),
            "BlockAnchorReplacer should find the block"
        );
    }

    #[test]
    fn test_block_anchor_needs_distinct_anchors() {
        let replacer = BlockAnchorReplacer;
        let content = "same\nsame";
        let search = "same\nsame";

        // Should fail because first and last anchors are the same
        let result = replacer.try_replace(content, search, "new");
        assert!(result.is_none());
    }

    // -------------------------------------------------------------------------
    // Strategy 4: WhitespaceNormalizedReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_whitespace_normalized_multiple_spaces() {
        let replacer = WhitespaceNormalizedReplacer;
        let content = "let   x   =   1;";
        let search = "let x = 1;";

        let result = replacer.try_replace(content, search, "let y = 2;");

        assert!(result.is_some());
        assert!(result.unwrap().contains("let y = 2;"));
    }

    #[test]
    fn test_whitespace_normalized_mixed_whitespace() {
        let replacer = WhitespaceNormalizedReplacer;
        let content = "fn\t \tmain()  {  }";
        let search = "fn main() { }";

        let result = replacer.try_replace(content, search, "fn test() {}");

        assert!(result.is_some());
    }

    // -------------------------------------------------------------------------
    // Strategy 5: IndentationFlexibleReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_indentation_flexible_handles_tabs_vs_spaces() {
        let replacer = IndentationFlexibleReplacer;
        let content = "\t\tlet x = 1;\n\t\tlet y = 2;";
        let search = "    let x = 1;\n    let y = 2;"; // spaces instead of tabs

        let result = replacer.try_replace(content, search, "let z = 3;");

        assert!(result.is_some());
    }

    #[test]
    fn test_indentation_flexible_different_levels() {
        let replacer = IndentationFlexibleReplacer;
        let content = "        deeply_indented();";
        let search = "deeply_indented();";

        let result = replacer.try_replace(content, search, "new_call();");

        assert!(result.is_some());
        // Should preserve original indentation
        assert!(result.unwrap().contains("        new_call();"));
    }

    // -------------------------------------------------------------------------
    // Strategy 6: EscapeNormalizedReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_escape_normalized_newlines() {
        let replacer = EscapeNormalizedReplacer;
        // Test with content that has normalized escapes
        let content = "let s = \"hello world\";";
        let search = "let s = \"hello world\";";

        // Verify exact match works
        let result = replacer.try_replace(content, search, "let s = \"new\";");
        assert!(result.is_some());

        // Test with content that might need escape normalization
        let content2 = "line1\nline2";
        let search2 = "line1\nline2";
        let result2 = replacer.try_replace(content2, search2, "replaced");
        assert!(result2.is_some());
    }

    // -------------------------------------------------------------------------
    // Strategy 7: TrimmedBoundaryReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_trimmed_boundary_matches_trimmed() {
        let replacer = TrimmedBoundaryReplacer;
        let content = "   \n   let x = 1;   \n   let y = 2;   \n   ";
        let search = "let x = 1;\nlet y = 2;";

        let result = replacer.try_replace(content, search, "let z = 3;");

        assert!(result.is_some());
    }

    // -------------------------------------------------------------------------
    // Strategy 8: ContextAwareReplacer Tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_context_aware_uses_surrounding_lines() {
        let replacer = ContextAwareReplacer::new(2);
        let content = "// before1\n// before2\ntarget_line\n// after1\n// after2";
        let search = "target_line";

        let result = replacer.try_replace(content, search, "new_target");

        assert!(result.is_some());
        assert!(result.unwrap().contains("new_target"));
    }
}
