//! Tests for the markdown renderer.

#[cfg(test)]
mod tests {
    use pulldown_cmark::HeadingLevel;

    use crate::markdown::renderer::helpers::{get_bullet, hash_string, heading_level_to_u8};
    use crate::markdown::renderer::{IncrementalMarkdownRenderer, MarkdownRenderer};
    use crate::markdown::theme::MarkdownTheme;

    // ============================================================
    // MarkdownRenderer Tests
    // ============================================================

    #[test]
    fn test_markdown_renderer_new() {
        let renderer = MarkdownRenderer::new();
        assert_eq!(renderer.width(), 80);
    }

    #[test]
    fn test_markdown_renderer_with_width() {
        let renderer = MarkdownRenderer::new().with_width(100);
        assert_eq!(renderer.width(), 100);
    }

    #[test]
    fn test_markdown_renderer_with_theme() {
        let theme = MarkdownTheme::default();
        let renderer = MarkdownRenderer::with_theme(theme);
        assert!(renderer.theme().h1.fg.is_some());
    }

    #[test]
    fn test_markdown_renderer_default() {
        let renderer = MarkdownRenderer::default();
        assert_eq!(renderer.width(), 80);
    }

    // ============================================================
    // Simple Paragraph Tests
    // ============================================================

    #[test]
    fn test_simple_paragraph() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("Hello, world!");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("Hello, world!"));
    }

    #[test]
    fn test_multiple_paragraphs() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("First paragraph.\n\nSecond paragraph.");
        // Should have at least 3 lines (first, blank, second)
        assert!(lines.len() >= 2);
    }

    // ============================================================
    // Header Tests
    // ============================================================

    #[test]
    fn test_header_h1() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("# Header 1");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("Header 1"));
    }

    #[test]
    fn test_header_h2() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("## Header 2");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("Header 2"));
    }

    #[test]
    fn test_header_h3() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("### Header 3");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_header_h4() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("#### Header 4");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_header_h5() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("##### Header 5");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_header_h6() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("###### Header 6");
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_all_header_levels() {
        let renderer = MarkdownRenderer::new();
        let md = "# H1\n## H2\n### H3\n#### H4\n##### H5\n###### H6";
        let lines = renderer.render(md);
        // Each header plus blank lines between
        assert!(lines.len() >= 6);
    }

    // ============================================================
    // Text Formatting Tests
    // ============================================================

    #[test]
    fn test_bold() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("This is **bold** text.");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("bold"));
    }

    #[test]
    fn test_italic() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("This is *italic* text.");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("italic"));
    }

    #[test]
    fn test_strikethrough() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("This is ~~strikethrough~~ text.");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("strikethrough"));
    }

    #[test]
    fn test_bold_italic() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("This is ***bold italic*** text.");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("bold italic"));
    }

    // ============================================================
    // Inline Code Tests
    // ============================================================

    #[test]
    fn test_inline_code() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("This is `inline code` in text.");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("inline code"));
    }

    #[test]
    fn test_multiple_inline_code() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("Use `foo` and `bar` functions.");
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("foo"));
        assert!(content.contains("bar"));
    }

    // ============================================================
    // Code Block Tests
    // ============================================================

    #[test]
    fn test_code_block_without_language() {
        let renderer = MarkdownRenderer::new();
        let md = "```\nfn main() {}\n```";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_code_block_with_language() {
        let renderer = MarkdownRenderer::new();
        let md = "```rust\nfn main() {\n    println!(\"Hello\");\n}\n```";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_code_block_multiple_languages() {
        let renderer = MarkdownRenderer::new();

        let rust_md = "```rust\nlet x = 1;\n```";
        let rust_lines = renderer.render(rust_md);
        assert!(!rust_lines.is_empty());

        let python_md = "```python\nx = 1\n```";
        let python_lines = renderer.render(python_md);
        assert!(!python_lines.is_empty());

        let js_md = "```javascript\nconst x = 1;\n```";
        let js_lines = renderer.render(js_md);
        assert!(!js_lines.is_empty());
    }

    // ============================================================
    // List Tests
    // ============================================================

    #[test]
    fn test_unordered_list() {
        let renderer = MarkdownRenderer::new();
        let md = "- Item 1\n- Item 2\n- Item 3";
        let lines = renderer.render(md);
        assert!(lines.len() >= 3);

        let content: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(content.contains("Item 1"));
        assert!(content.contains("Item 2"));
        assert!(content.contains("Item 3"));
    }

    #[test]
    fn test_ordered_list() {
        let renderer = MarkdownRenderer::new();
        let md = "1. First\n2. Second\n3. Third";
        let lines = renderer.render(md);
        assert!(lines.len() >= 3);

        let content: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(content.contains("First"));
        assert!(content.contains("Second"));
        assert!(content.contains("Third"));
    }

    #[test]
    fn test_nested_list() {
        let renderer = MarkdownRenderer::new();
        let md = "- Parent\n  - Child 1\n  - Child 2";
        let lines = renderer.render(md);
        assert!(lines.len() >= 3);
    }

    #[test]
    fn test_deeply_nested_list() {
        let renderer = MarkdownRenderer::new();
        let md = "- Level 1\n  - Level 2\n    - Level 3\n      - Level 4";
        let lines = renderer.render(md);
        assert!(lines.len() >= 4);
    }

    // ============================================================
    // Task List Tests
    // ============================================================

    #[test]
    fn test_task_list() {
        let renderer = MarkdownRenderer::new();
        let md = "- [x] Completed task\n- [ ] Pending task";
        let lines = renderer.render(md);
        assert!(lines.len() >= 2);

        let content: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect();
        assert!(content.contains("[x]") || content.contains("Completed"));
        assert!(content.contains("[ ]") || content.contains("Pending"));
    }

    #[test]
    fn test_mixed_task_list() {
        let renderer = MarkdownRenderer::new();
        let md = "- [x] Done\n- Regular item\n- [ ] Todo";
        let lines = renderer.render(md);
        assert!(lines.len() >= 3);
    }

    // ============================================================
    // Table Tests
    // ============================================================

    #[test]
    fn test_simple_table() {
        let renderer = MarkdownRenderer::new();
        let md = "| A | B |\n|---|---|\n| 1 | 2 |";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_table_with_alignment() {
        let renderer = MarkdownRenderer::new();
        let md = "| Left | Center | Right |\n|:-----|:------:|------:|\n| L | C | R |";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_table_multiple_rows() {
        let renderer = MarkdownRenderer::new();
        let md = "| H1 | H2 |\n|---|---|\n| A | B |\n| C | D |\n| E | F |";
        let lines = renderer.render(md);
        // Table should have multiple lines
        assert!(lines.len() >= 5);
    }

    // ============================================================
    // Blockquote Tests
    // ============================================================

    #[test]
    fn test_blockquote() {
        let renderer = MarkdownRenderer::new();
        let md = "> This is a quote";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("This is a quote") || content.contains("│"));
    }

    #[test]
    fn test_nested_blockquote() {
        let renderer = MarkdownRenderer::new();
        let md = "> Level 1\n>> Level 2";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_blockquote_with_formatting() {
        let renderer = MarkdownRenderer::new();
        let md = "> This is **bold** in a quote";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    // ============================================================
    // Link Tests
    // ============================================================

    #[test]
    fn test_link() {
        let renderer = MarkdownRenderer::new();
        let md = "[Link text](https://example.com)";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
        let content: String = lines[0].spans.iter().map(|s| &*s.content).collect();
        assert!(content.contains("Link text"));
    }

    #[test]
    fn test_link_with_same_text_and_url() {
        let renderer = MarkdownRenderer::new();
        let md = "[https://example.com](https://example.com)";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    // ============================================================
    // Horizontal Rule Tests
    // ============================================================

    #[test]
    fn test_horizontal_rule() {
        let renderer = MarkdownRenderer::new();
        let md = "---";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_horizontal_rule_variants() {
        let renderer = MarkdownRenderer::new();

        let lines1 = renderer.render("---");
        assert!(!lines1.is_empty());

        let lines2 = renderer.render("***");
        assert!(!lines2.is_empty());

        let lines3 = renderer.render("___");
        assert!(!lines3.is_empty());
    }

    // ============================================================
    // IncrementalMarkdownRenderer Tests
    // ============================================================

    #[test]
    fn test_incremental_new() {
        let renderer = MarkdownRenderer::new();
        let incremental = IncrementalMarkdownRenderer::new(renderer);
        assert!(incremental.is_dirty());
        assert!(incremental.source().is_empty());
    }

    #[test]
    fn test_incremental_set_source() {
        let renderer = MarkdownRenderer::new();
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("# Hello");
        assert!(incremental.is_dirty());
        assert_eq!(incremental.source(), "# Hello");

        let _ = incremental.get_lines();
        assert!(!incremental.is_dirty());

        // Setting same source shouldn't mark dirty
        incremental.set_source("# Hello");
        assert!(!incremental.is_dirty());

        // Setting different source should mark dirty
        incremental.set_source("# World");
        assert!(incremental.is_dirty());
    }

    #[test]
    fn test_incremental_append() {
        let renderer = MarkdownRenderer::new();
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.append("Hello ");
        assert_eq!(incremental.source(), "Hello ");

        incremental.append("World");
        assert_eq!(incremental.source(), "Hello World");
    }

    #[test]
    fn test_incremental_get_lines() {
        let renderer = MarkdownRenderer::new();
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("# Hello World");
        let lines = incremental.get_lines();
        assert!(!lines.is_empty());

        // Should not be dirty after get_lines
        assert!(!incremental.is_dirty());
    }

    #[test]
    fn test_incremental_caching() {
        let renderer = MarkdownRenderer::new();
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("Test content");
        let lines1 = incremental.get_lines();
        let lines2 = incremental.get_lines();

        // Both calls should return same result
        assert_eq!(lines1.len(), lines2.len());
    }

    #[test]
    fn test_incremental_invalidate() {
        let renderer = MarkdownRenderer::new();
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("Test");
        let _ = incremental.get_lines();
        assert!(!incremental.is_dirty());

        incremental.invalidate();
        assert!(incremental.is_dirty());
    }

    #[test]
    fn test_incremental_clear() {
        let renderer = MarkdownRenderer::new();
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("Some content");
        let _ = incremental.get_lines();

        incremental.clear();
        assert!(incremental.source().is_empty());
        assert!(incremental.is_dirty());
    }

    #[test]
    fn test_incremental_set_width() {
        let renderer = MarkdownRenderer::new().with_width(80);
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("Test");
        let _ = incremental.get_lines();
        assert!(!incremental.is_dirty());

        incremental.set_width(100);
        assert!(incremental.is_dirty());
    }

    #[test]
    fn test_incremental_width_no_change() {
        let renderer = MarkdownRenderer::new().with_width(80);
        let mut incremental = IncrementalMarkdownRenderer::new(renderer);

        incremental.set_source("Test");
        let _ = incremental.get_lines();

        // Same width shouldn't mark dirty
        incremental.set_width(80);
        assert!(!incremental.is_dirty());
    }

    // ============================================================
    // Edge Cases
    // ============================================================

    #[test]
    fn test_empty_input() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("");
        assert!(lines.is_empty());
    }

    #[test]
    fn test_whitespace_only() {
        let renderer = MarkdownRenderer::new();
        let lines = renderer.render("   \n\n   ");
        // May or may not have lines depending on how whitespace is handled
        assert!(lines.is_empty() || lines.iter().all(|l| l.spans.is_empty()));
    }

    #[test]
    fn test_unicode_content() {
        let renderer = MarkdownRenderer::new();
        let md = "# 你好世界\n\nこんにちは **太字**";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_emoji() {
        let renderer = MarkdownRenderer::new();
        let md = "Hello 👋 World 🌍";
        let lines = renderer.render(md);
        assert!(!lines.is_empty());
    }

    #[test]
    fn test_mixed_content() {
        let renderer = MarkdownRenderer::new();
        let md = r#"# Header

Some **bold** and *italic* text with `code`.

- List item 1
- List item 2

> A quote

```rust
fn main() {}
```

| A | B |
|---|---|
| 1 | 2 |

---

End."#;
        let lines = renderer.render(md);
        assert!(lines.len() > 10);
    }

    // ============================================================
    // Hash Function Test
    // ============================================================

    #[test]
    fn test_hash_string() {
        let h1 = hash_string("hello");
        let h2 = hash_string("hello");
        let h3 = hash_string("world");

        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    // ============================================================
    // Bullet Function Test
    // ============================================================

    #[test]
    fn test_get_bullet() {
        assert_eq!(get_bullet(0), "•");
        assert_eq!(get_bullet(1), "◦");
        assert_eq!(get_bullet(2), "▪");
        assert_eq!(get_bullet(3), "▸");
        assert_eq!(get_bullet(100), "▸"); // Should cap at last
    }

    // ============================================================
    // HeadingLevel Conversion Test
    // ============================================================

    #[test]
    fn test_heading_level_to_u8() {
        assert_eq!(heading_level_to_u8(HeadingLevel::H1), 1);
        assert_eq!(heading_level_to_u8(HeadingLevel::H2), 2);
        assert_eq!(heading_level_to_u8(HeadingLevel::H3), 3);
        assert_eq!(heading_level_to_u8(HeadingLevel::H4), 4);
        assert_eq!(heading_level_to_u8(HeadingLevel::H5), 5);
        assert_eq!(heading_level_to_u8(HeadingLevel::H6), 6);
    }
}
