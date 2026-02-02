//! URL processing and validation utilities.

use std::borrow::Cow;

use anyhow::{Result, bail};

/// Decode common HTML entities in URLs (#2449).
///
/// HTML attributes like href often contain encoded entities like `&amp;` for `&`.
/// This function decodes the most common HTML entities to produce valid URLs.
pub fn decode_html_entities(text: &str) -> Cow<'_, str> {
    // Quick check: if no & character, return as-is
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }

    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '&' {
            // Collect entity name until ; or end
            let mut entity = String::new();
            let mut found_semi = false;

            while let Some(&next) = chars.peek() {
                if next == ';' {
                    chars.next(); // consume semicolon
                    found_semi = true;
                    break;
                }
                if next == '&' || (!next.is_ascii_alphanumeric() && next != '#') {
                    // Not a valid entity, stop collecting
                    break;
                }
                entity.push(chars.next().unwrap());
                // Limit entity length to prevent DoS
                if entity.len() > 10 {
                    break;
                }
            }

            if found_semi {
                // Decode known entities
                let decoded = match entity.as_str() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    "nbsp" => " ",
                    "#38" => "&",
                    "#60" => "<",
                    "#62" => ">",
                    "#34" => "\"",
                    "#39" => "'",
                    _ => {
                        // Unknown entity, keep as-is
                        result.push('&');
                        result.push_str(&entity);
                        result.push(';');
                        continue;
                    }
                };
                result.push_str(decoded);
            } else {
                // Not a valid entity, output what we collected
                result.push('&');
                result.push_str(&entity);
            }
        } else {
            result.push(c);
        }
    }

    Cow::Owned(result)
}

/// Validate and sanitize URL for security (#2448).
///
/// Rejects URLs containing:
/// - Control characters (including null bytes) which could enable request smuggling
/// - Invalid percent-encoded sequences
pub fn validate_url_security(url: &str) -> Result<()> {
    // Check for control characters (ASCII 0-31, 127)
    for (i, c) in url.chars().enumerate() {
        if c.is_control() {
            bail!(
                "URL contains control character at position {} (code: {}). \
                Control characters in URLs can enable HTTP request smuggling attacks.",
                i,
                c as u32
            );
        }
    }

    // Check for null bytes in percent-encoded form
    if url.contains("%00") || url.contains("%0") {
        bail!(
            "URL contains percent-encoded null byte (%00). \
            This can cause security vulnerabilities."
        );
    }

    Ok(())
}

/// Convert common XPath expressions to CSS selectors.
///
/// This function handles the most common XPath patterns by converting them
/// to equivalent CSS selectors. For complex XPath expressions that cannot
/// be converted, it returns an error with guidance.
///
/// Supported patterns:
/// - //tag               -> tag
/// - //tag[@class='x']   -> tag.x
/// - //tag[@id='x']      -> tag#x
/// - //*[@class='x']     -> .x
/// - //*[@id='x']        -> #x
/// - //tag1/tag2         -> tag1 > tag2
/// - //tag1//tag2        -> tag1 tag2
pub fn xpath_to_css_selector(xpath: &str) -> Result<String> {
    let xpath = xpath.trim();

    // Handle union operator (|) by converting each part
    if xpath.contains(" | ") {
        let parts: Vec<&str> = xpath.split(" | ").collect();
        let css_parts: Result<Vec<String>> =
            parts.iter().map(|p| xpath_to_css_selector(p)).collect();
        return Ok(css_parts?.join(", "));
    }

    // Remove leading // or /
    let xpath = xpath.trim_start_matches("//").trim_start_matches('/');

    // Handle //* (any element)
    let xpath = xpath.strip_prefix('*').unwrap_or(xpath);

    let mut result = String::new();
    let mut remaining = xpath;

    while !remaining.is_empty() {
        // Handle descendant separator (//)
        if remaining.starts_with('/') {
            if remaining.starts_with("//") {
                result.push(' ');
                remaining = &remaining[2..];
            } else {
                result.push_str(" > ");
                remaining = &remaining[1..];
            }
            continue;
        }

        // Handle attribute predicates [@attr='value']
        if remaining.starts_with('[')
            && let Some(end) = remaining.find(']')
        {
            let predicate = &remaining[1..end];
            remaining = &remaining[end + 1..];

            // Parse @attr='value' or @attr="value"
            if let Some(attr_part) = predicate.strip_prefix('@') {
                if let Some(eq_pos) = attr_part.find('=') {
                    let attr_name = &attr_part[..eq_pos];
                    let attr_value = attr_part[eq_pos + 1..].trim_matches('\'').trim_matches('"');

                    match attr_name {
                        "class" => {
                            // Handle multiple classes separated by space
                            for class in attr_value.split_whitespace() {
                                result.push('.');
                                result.push_str(class);
                            }
                        }
                        "id" => {
                            result.push('#');
                            result.push_str(attr_value);
                        }
                        _ => {
                            result.push_str(&format!("[{}=\"{}\"]", attr_name, attr_value));
                        }
                    }
                } else {
                    // Just [@attr] without value
                    result.push_str(&format!("[{}]", attr_part));
                }
            } else {
                // Unsupported predicate
                bail!(
                    "XPath predicate '{}' is not supported. Use CSS selector instead.\n\
                         Example: cortex scrape --selector 'div.content' URL",
                    predicate
                );
            }
            continue;
        }

        // Handle element name (until next / or [)
        let end = remaining.find(['/', '[']).unwrap_or(remaining.len());
        if end > 0 {
            let tag = &remaining[..end];
            if tag != "*" {
                result.push_str(tag);
            }
            remaining = &remaining[end..];
        } else {
            break;
        }
    }

    let result = result.trim().to_string();
    if result.is_empty() {
        bail!(
            "Could not convert XPath '{}' to CSS selector. Please use --selector instead.",
            xpath
        );
    }

    Ok(result)
}
