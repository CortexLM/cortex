//! HTML processing and conversion utilities.

use scraper::{Html, Selector};

use super::url::decode_html_entities;

/// Extract main content from HTML, skipping navigation, footers, ads, etc.
pub fn extract_main_content(document: &Html) -> String {
    // Try to find main content areas
    let content_selectors = [
        "main",
        "article",
        "[role='main']",
        "#content",
        "#main-content",
        ".content",
        ".main-content",
        ".post-content",
        ".article-content",
        ".entry-content",
        ".page-content",
    ];

    for selector_str in content_selectors {
        if let Ok(selector) = Selector::parse(selector_str)
            && let Some(element) = document.select(&selector).next()
        {
            return element.html();
        }
    }

    // Fallback: get body and clean it
    if let Ok(body_selector) = Selector::parse("body")
        && let Some(body) = document.select(&body_selector).next()
    {
        return body.html();
    }

    // Last resort: return the whole document
    document.html()
}

/// Convert HTML to clean markdown.
pub fn html_to_markdown(html: &str, no_images: bool, no_links: bool) -> String {
    let _document = Html::parse_fragment(html);
    let mut output = String::new();
    let mut list_depth = 0;
    let mut in_pre = false;
    let mut in_code = false;

    // Remove unwanted elements first
    let cleaned = remove_unwanted_elements(html);
    let document = Html::parse_fragment(&cleaned);

    process_node_to_markdown(
        document.root_element(),
        &mut output,
        &mut list_depth,
        &mut in_pre,
        &mut in_code,
        no_images,
        no_links,
    );

    // Clean up whitespace
    clean_markdown_whitespace(&output)
}

/// Recursively process HTML nodes to markdown.
fn process_node_to_markdown(
    node: scraper::ElementRef,
    output: &mut String,
    list_depth: &mut usize,
    in_pre: &mut bool,
    in_code: &mut bool,
    no_images: bool,
    no_links: bool,
) {
    for child in node.children() {
        match child.value() {
            scraper::Node::Text(text) => {
                let text_content = text.text.as_ref();
                if *in_pre || *in_code {
                    output.push_str(text_content);
                } else {
                    // Normalize whitespace
                    let normalized = normalize_whitespace(text_content);
                    if !normalized.is_empty() {
                        output.push_str(&normalized);
                    }
                }
            }
            scraper::Node::Element(elem) => {
                let tag = elem.name.local.as_ref();
                if let Some(element_ref) = scraper::ElementRef::wrap(child) {
                    match tag {
                        // Skip unwanted elements
                        "script" | "style" | "nav" | "footer" | "header" | "aside" | "noscript" => {
                        }

                        // Headers
                        "h1" => {
                            output.push_str("\n\n# ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }
                        "h2" => {
                            output.push_str("\n\n## ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }
                        "h3" => {
                            output.push_str("\n\n### ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }
                        "h4" => {
                            output.push_str("\n\n#### ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }
                        "h5" => {
                            output.push_str("\n\n##### ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }
                        "h6" => {
                            output.push_str("\n\n###### ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }

                        // Paragraphs and blocks
                        "p" | "div" | "section" | "article" => {
                            output.push_str("\n\n");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }

                        // Line breaks
                        "br" => {
                            output.push('\n');
                        }
                        "hr" => {
                            output.push_str("\n\n---\n\n");
                        }

                        // Formatting
                        "strong" | "b" => {
                            output.push_str("**");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("**");
                        }
                        "em" | "i" => {
                            output.push('*');
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push('*');
                        }
                        "s" | "strike" | "del" => {
                            output.push_str("~~");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("~~");
                        }

                        // Code
                        "code" => {
                            if !*in_pre {
                                output.push('`');
                                *in_code = true;
                            }
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            if !*in_pre {
                                output.push('`');
                                *in_code = false;
                            }
                        }
                        "pre" => {
                            output.push_str("\n\n```\n");
                            *in_pre = true;
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            *in_pre = false;
                            output.push_str("\n```\n\n");
                        }

                        // Lists
                        "ul" => {
                            output.push('\n');
                            *list_depth += 1;
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            *list_depth -= 1;
                            output.push('\n');
                        }
                        "ol" => {
                            output.push('\n');
                            *list_depth += 1;
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            *list_depth -= 1;
                            output.push('\n');
                        }
                        "li" => {
                            let indent = "  ".repeat(list_depth.saturating_sub(1));
                            output.push_str(&format!("\n{indent}- "));
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                        }

                        // Blockquote
                        "blockquote" => {
                            output.push_str("\n\n> ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }

                        // Links
                        "a" => {
                            if no_links {
                                process_node_to_markdown(
                                    element_ref,
                                    output,
                                    list_depth,
                                    in_pre,
                                    in_code,
                                    no_images,
                                    no_links,
                                );
                            } else {
                                let href_raw = elem.attr("href").unwrap_or("");
                                // Decode HTML entities in href (e.g., &amp; -> &) (#2449)
                                let href = decode_html_entities(href_raw);
                                output.push('[');
                                process_node_to_markdown(
                                    element_ref,
                                    output,
                                    list_depth,
                                    in_pre,
                                    in_code,
                                    no_images,
                                    no_links,
                                );
                                output.push_str(&format!("]({})", href));
                            }
                        }

                        // Images
                        "img" => {
                            if !no_images {
                                let src = elem.attr("src").unwrap_or("");
                                let alt = elem.attr("alt").unwrap_or("image");
                                output.push_str(&format!("![{alt}]({src})"));
                            }
                        }

                        // Tables
                        "table" => {
                            output.push_str("\n\n");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str("\n\n");
                        }
                        "thead" | "tbody" | "tfoot" => {
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                        }
                        "tr" => {
                            output.push_str("| ");
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push('\n');
                        }
                        "th" | "td" => {
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                            output.push_str(" | ");
                        }

                        // Default: recurse
                        _ => {
                            process_node_to_markdown(
                                element_ref,
                                output,
                                list_depth,
                                in_pre,
                                in_code,
                                no_images,
                                no_links,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Convert HTML to plain text.
pub fn html_to_text(html: &str) -> String {
    let cleaned = remove_unwanted_elements(html);
    let document = Html::parse_fragment(&cleaned);

    let mut output = String::new();
    extract_text_content(document.root_element(), &mut output);

    // Clean up whitespace
    clean_text_whitespace(&output)
}

/// Recursively extract text content.
fn extract_text_content(node: scraper::ElementRef, output: &mut String) {
    for child in node.children() {
        match child.value() {
            scraper::Node::Text(text) => {
                let text_content = text.text.as_ref();
                let normalized = normalize_whitespace(text_content);
                if !normalized.is_empty() {
                    output.push_str(&normalized);
                }
            }
            scraper::Node::Element(elem) => {
                let tag = elem.name.local.as_ref();
                if let Some(element_ref) = scraper::ElementRef::wrap(child) {
                    match tag {
                        // Skip unwanted elements
                        "script" | "style" | "nav" | "footer" | "header" | "aside" | "noscript" => {
                        }

                        // Add line breaks for block elements
                        "p" | "div" | "section" | "article" | "h1" | "h2" | "h3" | "h4" | "h5"
                        | "h6" | "li" | "br" | "tr" => {
                            output.push('\n');
                            extract_text_content(element_ref, output);
                            output.push('\n');
                        }

                        _ => {
                            extract_text_content(element_ref, output);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Clean HTML by removing unwanted elements and optionally images/links.
pub fn clean_html(html: &str, no_images: bool, no_links: bool) -> String {
    let mut cleaned = remove_unwanted_elements(html);

    if no_images {
        // Remove img tags
        let document = Html::parse_fragment(&cleaned);
        let _result = String::new();
        for node in document.root_element().descendants() {
            if let Some(element) = node.value().as_element()
                && element.name.local.as_ref() != "img"
            {
                // Keep non-img elements
            }
        }
        // Simple regex-like removal
        cleaned = remove_tags(&cleaned, "img");
    }

    if no_links {
        // Convert links to just their text content
        cleaned = unwrap_tags(&cleaned, "a");
    }

    cleaned
}

/// Remove script, style, and other unwanted elements.
pub fn remove_unwanted_elements(html: &str) -> String {
    let unwanted_tags = [
        "script", "style", "noscript", "iframe", "svg", "canvas", "video", "audio",
    ];

    let mut result = html.to_string();
    for tag in unwanted_tags {
        result = remove_tags(&result, tag);
    }
    result
}

/// Remove all instances of a specific HTML tag.
pub fn remove_tags(html: &str, tag: &str) -> String {
    // Simple approach: use regex-like pattern matching
    let open_tag = format!("<{tag}");
    let close_tag = format!("</{tag}>");

    let mut result = html.to_string();

    // Handle self-closing tags (like <img />)
    while let Some(start) = result.to_lowercase().find(&open_tag) {
        if let Some(end) = result[start..].find('>') {
            // Check if it's a self-closing tag or has content
            let tag_end = start + end + 1;
            let lower = result.to_lowercase();
            if let Some(close_pos) = lower[tag_end..].find(&close_tag) {
                let close_end = tag_end + close_pos + close_tag.len();
                result = format!("{}{}", &result[..start], &result[close_end..]);
            } else {
                // Self-closing tag
                result = format!("{}{}", &result[..start], &result[tag_end..]);
            }
        } else {
            break;
        }
    }

    result
}

/// Unwrap tags, keeping their content.
pub fn unwrap_tags(html: &str, tag: &str) -> String {
    let document = Html::parse_fragment(html);
    let mut output = String::new();

    fn process_node(node: scraper::ElementRef, tag_to_unwrap: &str, output: &mut String) {
        for child in node.children() {
            match child.value() {
                scraper::Node::Text(text) => {
                    output.push_str(&text.text);
                }
                scraper::Node::Element(elem) => {
                    let current_tag = elem.name.local.as_ref();
                    if let Some(element_ref) = scraper::ElementRef::wrap(child) {
                        if current_tag == tag_to_unwrap {
                            // Just process children, don't output the tag
                            process_node(element_ref, tag_to_unwrap, output);
                        } else {
                            // Output the tag and process children
                            output.push('<');
                            output.push_str(current_tag);

                            // Copy attributes
                            for (name, value) in elem.attrs() {
                                output.push(' ');
                                output.push_str(name);
                                output.push_str("=\"");
                                output.push_str(value);
                                output.push('"');
                            }
                            output.push('>');

                            process_node(element_ref, tag_to_unwrap, output);

                            // Close tag
                            output.push_str("</");
                            output.push_str(current_tag);
                            output.push('>');
                        }
                    }
                }
                _ => {}
            }
        }
    }

    process_node(document.root_element(), tag, &mut output);
    output
}

/// Normalize whitespace in text.
pub fn normalize_whitespace(text: &str) -> String {
    let mut result = String::new();
    let mut last_was_space = false;

    for c in text.chars() {
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }
    }

    result
}

/// Clean up excessive whitespace in markdown output.
pub fn clean_markdown_whitespace(markdown: &str) -> String {
    let mut lines: Vec<&str> = markdown.lines().collect();

    // Remove leading/trailing empty lines
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }

    // Collapse multiple empty lines into two
    let mut result = String::new();
    let mut empty_count = 0;

    for line in lines {
        if line.trim().is_empty() {
            empty_count += 1;
            if empty_count <= 2 {
                result.push('\n');
            }
        } else {
            empty_count = 0;
            result.push_str(line);
            result.push('\n');
        }
    }

    // Trim trailing whitespace on each line
    result
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Clean up excessive whitespace in text output.
pub fn clean_text_whitespace(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();

    // Clean each line and collapse empty lines
    let mut result = String::new();
    let mut empty_count = 0;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            empty_count += 1;
            if empty_count <= 1 {
                result.push('\n');
            }
        } else {
            empty_count = 0;
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}
