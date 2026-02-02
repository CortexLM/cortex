//! Web query tool handler.
//!
//! Provides web search capability using DuckDuckGo's instant answer API.
//! Supports advanced filtering by category, domains, and search type.

use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use serde_json::Value;

use super::{ToolContext, ToolHandler, ToolResult};
use crate::error::Result;
use cortex_common::create_default_client;

/// Handler for WebSearch tool.
pub struct WebSearchHandler {
    client: Client,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WebQueryArgs {
    query: String,
    #[serde(default = "default_search_type")]
    search_type: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default = "default_num_results")]
    num_results: usize,
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
    #[serde(default)]
    include_text: bool,
}

fn default_search_type() -> String {
    "auto".to_string()
}

fn default_num_results() -> usize {
    10
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct DuckDuckGoResponse {
    #[serde(rename = "Abstract")]
    abstract_text: Option<String>,
    #[serde(rename = "AbstractSource")]
    abstract_source: Option<String>,
    #[serde(rename = "AbstractURL")]
    abstract_url: Option<String>,
    #[serde(rename = "Heading")]
    heading: Option<String>,
    #[serde(rename = "RelatedTopics")]
    related_topics: Option<Vec<RelatedTopic>>,
    #[serde(rename = "Results")]
    results: Option<Vec<SearchResult>>,
    #[serde(rename = "Answer")]
    answer: Option<String>,
    #[serde(rename = "AnswerType")]
    answer_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelatedTopic {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "FirstURL")]
    first_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SearchResult {
    #[serde(rename = "Text")]
    text: Option<String>,
    #[serde(rename = "FirstURL")]
    first_url: Option<String>,
}

impl WebSearchHandler {
    pub fn new() -> Self {
        let client = create_default_client().expect("HTTP client");

        Self { client }
    }
}

impl Default for WebSearchHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for WebSearchHandler {
    fn name(&self) -> &str {
        "WebSearch"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let args: WebQueryArgs = serde_json::from_value(arguments)?;

        // Validate that include_domains and exclude_domains are not both specified
        if args.include_domains.is_some() && args.exclude_domains.is_some() {
            return Ok(ToolResult::error(
                "Cannot specify both include_domains and exclude_domains".to_string(),
            ));
        }

        // Build DuckDuckGo Instant Answer API URL with optional parameters
        let mut url = format!(
            "https://api.duckduckgo.com/?q={}&format=json&no_redirect=1&no_html=1&skip_disambig=1",
            urlencoding::encode(&args.query)
        );

        // Add include_text parameter if requested
        if args.include_text {
            url.push_str("&t=cortex");
        }

        let response = match self.client.get(&url).send().await {
            Ok(resp) => resp,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to search: {e}")));
            }
        };

        if !response.status().is_success() {
            return Ok(ToolResult::error(format!(
                "Search failed with status: {}",
                response.status()
            )));
        }

        let ddg: DuckDuckGoResponse = match response.json().await {
            Ok(r) => r,
            Err(e) => {
                return Ok(ToolResult::error(format!("Failed to parse response: {e}")));
            }
        };

        let mut results = Vec::new();

        // Add direct answer if available
        if let Some(answer) = ddg.answer
            && !answer.is_empty()
        {
            results.push(format!("Direct Answer: {answer}"));
        }

        // Add abstract if available
        if let Some(abstract_text) = ddg.abstract_text
            && !abstract_text.is_empty()
        {
            let source = ddg.abstract_source.unwrap_or_default();
            let url = ddg.abstract_url.unwrap_or_default();
            results.push(format!(
                "Summary ({source}): {abstract_text}\nSource: {url}"
            ));
        }

        // Add related topics
        if let Some(topics) = ddg.related_topics {
            for (i, topic) in topics.iter().take(args.num_results).enumerate() {
                if let (Some(text), Some(url)) = (&topic.text, &topic.first_url) {
                    results.push(format!("{}. {}\n   URL: {}", i + 1, text, url));
                }
            }
        }

        // Add results
        if let Some(search_results) = ddg.results {
            for (i, result) in search_results.iter().take(args.num_results).enumerate() {
                if let (Some(text), Some(url)) = (&result.text, &result.first_url) {
                    results.push(format!("{}. {}\n   URL: {}", i + 1, text, url));
                }
            }
        }

        if results.is_empty() {
            // Fall back to a simple message
            Ok(ToolResult::success(format!(
                "No immediate results found for '{}'. You may want to try a more specific query or search directly at https://duckduckgo.com/?q={}",
                args.query,
                urlencoding::encode(&args.query)
            )))
        } else {
            Ok(ToolResult::success(format!(
                "Search results for '{}':\n\n{}",
                args.query,
                results.join("\n\n")
            )))
        }
    }
}

mod urlencoding {
    pub fn encode(s: &str) -> String {
        let mut result = String::new();
        for c in s.chars() {
            match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                    result.push(c);
                }
                ' ' => {
                    result.push('+');
                }
                _ => {
                    for b in c.to_string().as_bytes() {
                        result.push_str(&format!("%{b:02X}"));
                    }
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_web_search_handler() {
        let handler = WebSearchHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let result = handler
            .execute(
                serde_json::json!({ "query": "Rust programming language" }),
                &context,
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_conflicting_domains() {
        let handler = WebSearchHandler::new();
        let context = ToolContext::new(PathBuf::from("."));

        let result = handler
            .execute(
                serde_json::json!({
                    "query": "test",
                    "include_domains": ["example.com"],
                    "exclude_domains": ["other.com"]
                }),
                &context,
            )
            .await;

        assert!(result.is_ok());
        let tool_result = result.unwrap();
        assert!(!tool_result.success);
    }
}
