//! Web search tool for the agent using Exa AI.

use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::Result;
use crate::tools::handlers::ToolHandler;
use crate::tools::spec::ToolMetadata;
use crate::tools::{ToolContext, ToolResult};
use cortex_common::create_default_client;

const EXA_MCP_URL: &str = "https://mcp.exa.ai/mcp";
const DEFAULT_NUM_RESULTS: usize = 8;

/// Tool for searching the web using Exa AI.
pub struct WebSearchTool {
    client: Client,
}

#[derive(Debug, Deserialize, Serialize)]
struct WebSearchArgs {
    /// The search term or question to query on the web.
    query: String,
    /// Number of search results to return (default: 8).
    #[serde(rename = "num_results")]
    num_results: Option<usize>,
    /// Specify the category of search results (e.g., "company", "research paper", "news").
    #[serde(default)]
    category: Option<String>,
    /// Limit results to specific domains.
    #[serde(default)]
    include_domains: Option<Vec<String>>,
    /// Exclude results from certain domains.
    #[serde(default)]
    exclude_domains: Option<Vec<String>>,
    /// Enable neural search capabilities for more relevant results.
    #[serde(default)]
    use_neural: Option<bool>,
    /// Live crawl mode - 'fallback': use live crawling as backup if cached content unavailable, 'preferred': prioritize live crawling.
    #[serde(default)]
    livecrawl: Option<String>,
    /// Search type - 'auto': balanced search (default), 'fast': quick results, 'deep': comprehensive search.
    #[serde(rename = "type")]
    #[serde(default)]
    search_type: Option<String>,
    /// Maximum characters for context string optimized for LLMs (default: 10000).
    #[serde(default)]
    context_max_characters: Option<usize>,
}

#[derive(Debug, Serialize)]
struct McpSearchRequest {
    jsonrpc: String,
    id: u32,
    method: String,
    params: McpSearchParams,
}

#[derive(Debug, Serialize)]
struct McpSearchParams {
    name: String,
    arguments: McpSearchArguments,
}

#[derive(Debug, Serialize)]
struct McpSearchArguments {
    query: String,
    #[serde(rename = "numResults")]
    num_results: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    category: Option<String>,
    #[serde(rename = "includeDomains", skip_serializing_if = "Option::is_none")]
    include_domains: Option<Vec<String>>,
    #[serde(rename = "excludeDomains", skip_serializing_if = "Option::is_none")]
    exclude_domains: Option<Vec<String>>,
    #[serde(rename = "useNeural", skip_serializing_if = "Option::is_none")]
    use_neural: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    livecrawl: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    search_type: Option<String>,
    #[serde(
        rename = "contextMaxCharacters",
        skip_serializing_if = "Option::is_none"
    )]
    context_max_characters: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct McpSearchResponse {
    #[allow(dead_code)]
    jsonrpc: String,
    result: McpSearchResult,
}

#[derive(Debug, Deserialize)]
struct McpSearchResult {
    content: Vec<McpSearchContent>,
}

#[derive(Debug, Deserialize)]
struct McpSearchContent {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    content_type: String,
    text: String,
}

impl WebSearchTool {
    pub fn new() -> Self {
        let client = create_default_client().expect("HTTP client");

        Self { client }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ToolHandler for WebSearchTool {
    fn name(&self) -> &str {
        "WebSearch"
    }

    async fn execute(&self, arguments: Value, _context: &ToolContext) -> Result<ToolResult> {
        let args: WebSearchArgs = match serde_json::from_value(arguments) {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::error(format!("Invalid arguments: {e}"))),
        };

        let search_request = McpSearchRequest {
            jsonrpc: "2.0".to_string(),
            id: 1,
            method: "tools/call".to_string(),
            params: McpSearchParams {
                name: "web_search_exa".to_string(),
                arguments: McpSearchArguments {
                    query: args.query.clone(),
                    num_results: args.num_results.unwrap_or(DEFAULT_NUM_RESULTS),
                    category: args.category,
                    include_domains: args.include_domains,
                    exclude_domains: args.exclude_domains,
                    use_neural: args.use_neural,
                    livecrawl: args.livecrawl,
                    search_type: args.search_type,
                    context_max_characters: args.context_max_characters,
                },
            },
        };

        let response = match self
            .client
            .post(EXA_MCP_URL)
            .json(&search_request)
            .send()
            .await
        {
            Ok(resp) => resp,
            Err(e) => return Ok(ToolResult::error(format!("Search request failed: {e}"))),
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Ok(ToolResult::error(format!(
                "Search error ({status}): {error_text}"
            )));
        }

        let response_text = match response.text().await {
            Ok(t) => t,
            Err(e) => return Ok(ToolResult::error(format!("Failed to read response: {e}"))),
        };

        // Parse SSE response (Exa MCP returns SSE)
        let mut output = String::new();
        for line in response_text.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(mcp_resp) = serde_json::from_str::<McpSearchResponse>(data) {
                    if let Some(content) = mcp_resp.result.content.first() {
                        output = content.text.clone();
                        break;
                    }
                }
            }
        }

        if output.is_empty() {
            // Try parsing as direct JSON if not SSE
            if let Ok(mcp_resp) = serde_json::from_str::<McpSearchResponse>(&response_text) {
                if let Some(content) = mcp_resp.result.content.first() {
                    output = content.text.clone();
                }
            }
        }

        if output.is_empty() {
            return Ok(ToolResult::success(
                "No search results found. Please try a different query.",
            ));
        }

        let metadata = ToolMetadata {
            duration_ms: 0, // Filled by executor
            exit_code: Some(0),
            files_modified: vec![],
            data: Some(json!({
                "query": args.query,
                "engine": "exa"
            })),
        };

        Ok(ToolResult::success(output).with_metadata(metadata))
    }
}
