//! Web fetch tool for the agent.

use std::time::Duration;

use async_trait::async_trait;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::error::Result;
use crate::tools::handlers::ToolHandler;
use crate::tools::spec::ToolMetadata;
use crate::tools::{ToolContext, ToolResult};
use cortex_common::create_client_builder;

/// Tool for fetching web content and converting it to markdown.
pub struct WebFetchTool {
    client: Client,
}

#[derive(Debug, Deserialize)]
struct WebFetchArgs {
    url: String,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    timeout: Option<u64>,
}

impl WebFetchTool {
    pub fn new() -> Self {
        // Use a browser-like user agent for better compatibility
        let client = create_client_builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
            .build()
            .expect("HTTP client");

        Self { client }
    }

    fn extract_metadata(&self, html: &str) -> Value {
        let mut metadata = json!({});

        // Extract title
        let title_re = Regex::new(r"(?i)<title>(.*?)</title>").unwrap();
        if let Some(caps) = title_re.captures(html) {
            metadata["title"] = json!(caps.get(1).unwrap().as_str().trim());
        }

        // Extract description
        let desc_re =
            Regex::new(r#"(?i)<meta\s+name=["']description["']\s+content=["'](.*?)["']"#).unwrap();
        if let Some(caps) = desc_re.captures(html) {
            metadata["description"] = json!(caps.get(1).unwrap().as_str().trim());
        }

        // Extract keywords
        let key_re =
            Regex::new(r#"(?i)<meta\s+name=["']keywords["']\s+content=["'](.*?)["']"#).unwrap();
        if let Some(caps) = key_re.captures(html) {
            metadata["keywords"] = json!(caps.get(1).unwrap().as_str().trim());
        }

        metadata
    }

    fn convert_to_markdown(&self, html: &str) -> String {
        let mut md = html.to_string();

        // 1. Remove script and style tags
        let script_re = Regex::new(r"(?is)<script.*?>.*?</script>").unwrap();
        md = script_re.replace_all(&md, "").to_string();
        let style_re = Regex::new(r"(?is)<style.*?>.*?</style>").unwrap();
        md = style_re.replace_all(&md, "").to_string();
        let head_re = Regex::new(r"(?is)<head.*?>.*?</head>").unwrap();
        md = head_re.replace_all(&md, "").to_string();

        // 2. Handle code blocks
        // <pre><code>...</code></pre>
        let pre_code_re = Regex::new(r"(?is)<pre><code>(.*?)</code></pre>").unwrap();
        md = pre_code_re.replace_all(&md, "\n```\n$1\n```\n").to_string();
        let code_re = Regex::new(r"(?is)<code>(.*?)</code>").unwrap();
        md = code_re.replace_all(&md, "`$1`").to_string();

        // 3. Handle tables
        md = self.convert_tables(&md);

        // 4. Handle headers
        let h1_re = Regex::new(r"(?i)<h1.*?>(.*?)</h1>").unwrap();
        md = h1_re.replace_all(&md, "\n# $1\n").to_string();
        let h2_re = Regex::new(r"(?i)<h2.*?>(.*?)</h2>").unwrap();
        md = h2_re.replace_all(&md, "\n## $1\n").to_string();
        let h3_re = Regex::new(r"(?i)<h3.*?>(.*?)</h3>").unwrap();
        md = h3_re.replace_all(&md, "\n### $1\n").to_string();

        // 5. Handle simple tags
        let b_re = Regex::new(r"(?i)<b.*?>(.*?)</b>").unwrap();
        md = b_re.replace_all(&md, "**$1**").to_string();
        let strong_re = Regex::new(r"(?i)<strong.*?>(.*?)</strong>").unwrap();
        md = strong_re.replace_all(&md, "**$1**").to_string();
        let i_re = Regex::new(r"(?i)<i.*?>(.*?)</i>").unwrap();
        md = i_re.replace_all(&md, "*$1*").to_string();
        let em_re = Regex::new(r"(?i)<em.*?>(.*?)</em>").unwrap();
        md = em_re.replace_all(&md, "*$1*").to_string();

        // 6. Handle links
        let a_re = Regex::new(r#"(?i)<a\s+.*?href=["'](.*?)["'].*?>(.*?)</a>"#).unwrap();
        md = a_re.replace_all(&md, "[$2]($1)").to_string();

        // 7. Handle lists
        let li_re = Regex::new(r"(?i)<li.*?>(.*?)</li>").unwrap();
        md = li_re.replace_all(&md, "- $1\n").to_string();

        // 8. Strip remaining tags
        let tag_re = Regex::new(r"<[^>]*>").unwrap();
        md = tag_re.replace_all(&md, "").to_string();

        // 9. Clean up whitespace
        let mut cleaned = String::new();
        let mut last_was_empty = false;
        for line in md.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                if !last_was_empty {
                    cleaned.push('\n');
                    last_was_empty = true;
                }
            } else {
                cleaned.push_str(trimmed);
                cleaned.push('\n');
                last_was_empty = false;
            }
        }

        cleaned.trim().to_string()
    }

    fn convert_tables(&self, html: &str) -> String {
        let mut result = html.to_string();
        let table_re = Regex::new(r"(?is)<table.*?>(.*?)</table>").unwrap();
        let tr_re = Regex::new(r"(?is)<tr.*?>(.*?)</tr>").unwrap();
        let th_re = Regex::new(r"(?is)<th.*?>(.*?)</th>").unwrap();
        let td_re = Regex::new(r"(?is)<td.*?>(.*?)</td>").unwrap();

        let mut offset = 0;
        let original = html.to_string();

        for caps in table_re.captures_iter(&original) {
            let table_content = caps.get(1).unwrap().as_str();
            let mut md_table = String::new();
            let mut first_row = true;
            for tr_caps in tr_re.captures_iter(table_content) {
                let tr_content = tr_caps.get(1).unwrap().as_str();
                let mut row = String::from("|");
                let mut current_col_count = 0;

                // Handle headers
                for th_caps in th_re.captures_iter(tr_content) {
                    let content = self.strip_tags(th_caps.get(1).unwrap().as_str());
                    row.push_str(&format!(" {} |", content.trim()));
                    current_col_count += 1;
                }

                // Handle cells
                for td_caps in td_re.captures_iter(tr_content) {
                    let content = self.strip_tags(td_caps.get(1).unwrap().as_str());
                    row.push_str(&format!(" {} |", content.trim()));
                    current_col_count += 1;
                }

                md_table.push_str(&row);
                md_table.push('\n');

                if first_row {
                    let mut separator = String::from("|");
                    for _ in 0..current_col_count {
                        separator.push_str(" --- |");
                    }
                    md_table.push_str(&separator);
                    md_table.push('\n');
                    first_row = false;
                }
            }

            let full_match = caps.get(0).unwrap();
            let range = full_match.range();
            let start = range.start as i64 + offset;
            let end = range.end as i64 + offset;

            result.replace_range((start as usize)..(end as usize), &md_table);
            offset += md_table.len() as i64 - (range.end - range.start) as i64;
        }

        result
    }

    fn strip_tags(&self, html: &str) -> String {
        let tag_re = Regex::new(r"<[^>]*>").unwrap();
        tag_re.replace_all(html, "").to_string()
    }
}

#[async_trait]
impl ToolHandler for WebFetchTool {
    fn name(&self) -> &str {
        "WebFetch"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let args: WebFetchArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {e}"))),
        };

        // Validate URL
        if !args.url.starts_with("http://") && !args.url.starts_with("https://") {
            return Ok(ToolResult::error("URL must start with http:// or https://"));
        }

        let timeout_duration = args
            .timeout
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(30));

        let response = match self
            .client
            .get(&args.url)
            .timeout(timeout_duration)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => return Ok(ToolResult::error(format!("Request failed: {e}"))),
        };

        if !response.status().is_success() {
            return Ok(ToolResult::error(format!(
                "HTTP error: {}",
                response.status()
            )));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();

        let text = match response.text().await {
            Ok(t) => t,
            Err(e) => return Ok(ToolResult::error(format!("Failed to read response: {e}"))),
        };

        let format = args.format.unwrap_or_else(|| "markdown".to_string());
        let mut metadata = self.extract_metadata(&text);
        metadata["url"] = json!(args.url);
        metadata["content_type"] = json!(content_type);

        let output = if content_type.contains("text/html") {
            match format.as_str() {
                "markdown" => self.convert_to_markdown(&text),
                "text" => self.strip_tags(&text),
                "html" => text,
                _ => self.convert_to_markdown(&text),
            }
        } else {
            text
        };

        // Truncate if too large
        let final_output = if output.len() > 100_000 {
            format!(
                "{}...\n[Content truncated at 100000 chars]",
                &output[..100_000]
            )
        } else {
            output
        };

        let tool_metadata = ToolMetadata {
            duration_ms: 0, // Will be filled by executor
            exit_code: Some(0),
            files_modified: vec![],
            data: Some(metadata),
        };

        Ok(ToolResult::success(final_output).with_metadata(tool_metadata))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_extract_metadata() {
        let tool = WebFetchTool::new();
        let html = r#"
            <html>
                <head>
                    <title>Test Title</title>
                    <meta name="description" content="Test Description">
                    <meta name="keywords" content="test, keyword">
                </head>
                <body>Hello</body>
            </html>
        "#;
        let metadata = tool.extract_metadata(html);
        assert_eq!(metadata["title"], "Test Title");
        assert_eq!(metadata["description"], "Test Description");
        assert_eq!(metadata["keywords"], "test, keyword");
    }

    #[tokio::test]
    async fn test_convert_to_markdown() {
        let tool = WebFetchTool::new();
        let html = r#"
            <html>
                <body>
                    <h1>Title</h1>
                    <p>This is a <b>bold</b> and <i>italic</i> text.</p>
                    <pre><code>fn main() {
    println!("Hello");
}</code></pre>
                    <table>
                        <tr><th>Header 1</th><th>Header 2</th></tr>
                        <tr><td>Cell 1</td><td>Cell 2</td></tr>
                    </table>
                    <ul>
                        <li>Item 1</li>
                        <li>Item 2</li>
                    </ul>
                    <a href="https://example.com">Link</a>
                </body>
            </html>
        "#;
        let md = tool.convert_to_markdown(html);
        assert!(md.contains("# Title"));
        assert!(md.contains("**bold**"));
        assert!(md.contains("*italic*"));
        assert!(md.contains("```"));
        assert!(md.contains("| Header 1 | Header 2 |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| Cell 1 | Cell 2 |"));
        assert!(md.contains("- Item 1"));
        assert!(md.contains("[Link](https://example.com)"));
    }
}
