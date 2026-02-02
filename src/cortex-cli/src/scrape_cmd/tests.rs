//! Tests for the scrape command.

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cortex_engine::create_client_builder;

    use crate::scrape_cmd::command::ScrapeCommand;
    use crate::scrape_cmd::html::{html_to_markdown, html_to_text, normalize_whitespace};
    use crate::scrape_cmd::http::parse_headers;
    use crate::scrape_cmd::types::OutputFormat;
    use crate::scrape_cmd::url::{decode_html_entities, validate_url_security};

    #[test]
    fn test_html_to_markdown_basic() {
        let html = "<h1>Title</h1><p>Hello <strong>world</strong>!</p>";
        let md = html_to_markdown(html, false, false);
        assert!(md.contains("# Title"));
        assert!(md.contains("**world**"));
    }

    #[test]
    fn test_html_to_markdown_links() {
        let html = r#"<a href="https://example.com">Example</a>"#;
        let md = html_to_markdown(html, false, false);
        assert!(md.contains("[Example](https://example.com)"));

        let md_no_links = html_to_markdown(html, false, true);
        assert!(md_no_links.contains("Example"));
        assert!(!md_no_links.contains("]("));
    }

    #[test]
    fn test_html_to_markdown_images() {
        let html = r#"<img src="image.png" alt="Test Image">"#;
        let md = html_to_markdown(html, false, false);
        assert!(md.contains("![Test Image](image.png)"));

        let md_no_images = html_to_markdown(html, true, false);
        assert!(!md_no_images.contains("!["));
    }

    #[test]
    fn test_html_to_text() {
        let html = "<h1>Title</h1><p>Hello <strong>world</strong>!</p>";
        let text = html_to_text(html);
        assert!(text.contains("Title"));
        // The exact whitespace may vary depending on parser, just check key content
        assert!(text.contains("Hello") && text.contains("world"));
    }

    #[test]
    fn test_parse_headers() {
        let headers = vec![
            "Content-Type: application/json".to_string(),
            "Authorization: Bearer token".to_string(),
        ];
        let parsed = parse_headers(&headers).expect("Should parse");
        assert_eq!(
            parsed.get("Content-Type"),
            Some(&"application/json".to_string())
        );
        assert_eq!(
            parsed.get("Authorization"),
            Some(&"Bearer token".to_string())
        );
    }

    #[test]
    fn test_parse_headers_empty_name_or_value() {
        // Test header with just colon
        let headers = vec![":".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("name and value required")
        );

        // Test header with empty name
        let headers = vec![": value".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("name and value required")
        );

        // Test header with empty value
        let headers = vec!["name:".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("name and value required")
        );

        // Test header with just whitespace
        let headers = vec!["  :  ".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("name and value required")
        );

        // Test header with whitespace name and value
        let headers = vec!["  : value".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());

        let headers = vec!["name:  ".to_string()];
        let result = parse_headers(&headers);
        assert!(result.is_err());
    }

    #[test]
    fn test_output_format_parsing() {
        assert_eq!(
            "markdown".parse::<OutputFormat>().ok(),
            Some(OutputFormat::Markdown)
        );
        assert_eq!(
            "md".parse::<OutputFormat>().ok(),
            Some(OutputFormat::Markdown)
        );
        assert_eq!(
            "text".parse::<OutputFormat>().ok(),
            Some(OutputFormat::Text)
        );
        assert_eq!("txt".parse::<OutputFormat>().ok(), Some(OutputFormat::Text));
        assert_eq!(
            "html".parse::<OutputFormat>().ok(),
            Some(OutputFormat::Html)
        );
        assert!("invalid".parse::<OutputFormat>().is_err());
    }

    #[test]
    fn test_normalize_whitespace() {
        // normalize_whitespace collapses multiple whitespace into single space
        let result1 = normalize_whitespace("  hello   world  ");
        assert!(result1.contains("hello") && result1.contains("world"));
        assert!(!result1.contains("  ")); // No double spaces

        let result2 = normalize_whitespace("no\n\nextra\t\tspaces");
        assert!(result2.contains("no") && result2.contains("extra") && result2.contains("spaces"));
    }

    #[tokio::test]
    async fn test_empty_url_validation() {
        let cmd = ScrapeCommand {
            url: String::new(),
            output: None,
            format: "markdown".to_string(),
            method: "GET".to_string(),
            timeout: 30,
            retries: 3,
            user_agent: None,
            headers: vec![],
            cookies: vec![],
            no_follow_redirects: false,
            no_images: false,
            no_links: false,
            selector: vec![],
            xpath: None,
            verbose: false,
            pretty: false,
        };

        let result = cmd.run().await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("URL cannot be empty")
        );
    }

    #[tokio::test]
    async fn test_whitespace_url_validation() {
        let cmd = ScrapeCommand {
            url: "   ".to_string(),
            output: None,
            format: "markdown".to_string(),
            method: "GET".to_string(),
            timeout: 30,
            retries: 3,
            user_agent: None,
            headers: vec![],
            cookies: vec![],
            no_follow_redirects: false,
            no_images: false,
            no_links: false,
            selector: vec![],
            xpath: None,
            verbose: false,
            pretty: false,
        };

        let result = cmd.run().await;
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("URL cannot be empty")
        );
    }

    #[test]
    fn test_timeout_zero_creates_no_timeout_client() {
        // Test that timeout 0 is handled correctly by not setting a timeout
        // This test verifies the command structure, not the HTTP behavior
        let cmd = ScrapeCommand {
            url: "https://example.com".to_string(),
            output: None,
            format: "markdown".to_string(),
            method: "GET".to_string(),
            timeout: 0,
            retries: 3,
            user_agent: None,
            headers: vec![],
            cookies: vec![],
            no_follow_redirects: false,
            no_images: false,
            no_links: false,
            selector: vec![],
            xpath: None,
            verbose: false,
            pretty: false,
        };

        // Verify timeout is 0 (meaning no timeout will be set)
        assert_eq!(cmd.timeout, 0);

        // Build client with same logic as run() to verify it doesn't panic
        let client_builder =
            create_client_builder().redirect(reqwest::redirect::Policy::limited(10));

        // Conditionally set timeout (0 means no timeout)
        let client_builder = if cmd.timeout > 0 {
            client_builder.timeout(Duration::from_secs(cmd.timeout))
        } else {
            client_builder
        };

        // Verify client builds successfully without timeout
        let client = client_builder.build();
        assert!(
            client.is_ok(),
            "Client should build successfully with timeout 0"
        );
    }

    #[test]
    fn test_html_to_markdown_links_with_html_entities() {
        // Test that HTML entities in URLs are decoded properly (Issue #2449)
        let html = r#"<a href="/search?q=test&amp;page=2">Next</a>"#;
        let md = html_to_markdown(html, false, false);
        assert!(
            md.contains("[Next](/search?q=test&page=2)"),
            "Expected decoded URL with &, got: {}",
            md
        );

        // Multiple entities
        let html = r#"<a href="/path?a=1&amp;b=2&amp;c=3">Link</a>"#;
        let md = html_to_markdown(html, false, false);
        assert!(md.contains("/path?a=1&b=2&c=3"));
    }

    #[test]
    fn test_decode_html_entities() {
        use std::borrow::Cow;

        // Basic entities
        assert_eq!(decode_html_entities("&amp;").as_ref(), "&");
        assert_eq!(decode_html_entities("&lt;").as_ref(), "<");
        assert_eq!(decode_html_entities("&gt;").as_ref(), ">");
        assert_eq!(decode_html_entities("&quot;").as_ref(), "\"");

        // URL with multiple entities
        assert_eq!(
            decode_html_entities("/search?q=test&amp;page=2").as_ref(),
            "/search?q=test&page=2"
        );

        // No entities - should return borrowed
        let result = decode_html_entities("plain text");
        assert!(matches!(result, Cow::Borrowed(_)));

        // Mixed content
        assert_eq!(
            decode_html_entities("before &amp; after").as_ref(),
            "before & after"
        );

        // Incomplete entity (no semicolon)
        assert_eq!(decode_html_entities("&amp text").as_ref(), "&amp text");

        // Unknown entity
        assert_eq!(decode_html_entities("&unknown;").as_ref(), "&unknown;");
    }

    #[test]
    fn test_validate_url_security() {
        // Valid URLs should pass
        assert!(validate_url_security("https://example.com/path").is_ok());
        assert!(validate_url_security("https://example.com/search?q=test&page=2").is_ok());

        // URLs with control characters should fail
        assert!(validate_url_security("https://example.com/\0path").is_err());
        assert!(validate_url_security("https://example.com/\npath").is_err());

        // URLs with percent-encoded null bytes should fail
        assert!(validate_url_security("https://example.com/%00path").is_err());
    }
}
