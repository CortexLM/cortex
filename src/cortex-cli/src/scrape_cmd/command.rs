//! Scrape command definition and implementation.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Parser;
use cortex_engine::create_client_builder;

use super::html::{clean_html, extract_main_content, html_to_markdown, html_to_text};
use super::http::{format_http_error, parse_headers};
use super::types::OutputFormat;
use super::url::{validate_url_security, xpath_to_css_selector};
use super::xml::format_xml;
use scraper::{Html, Selector};

/// Scrape web content and convert to clean formats.
#[derive(Debug, Parser)]
pub struct ScrapeCommand {
    /// URL to scrape.
    #[arg(required = true)]
    pub url: String,

    /// Output file path (stdout if not specified).
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Output format (markdown, text, html).
    #[arg(short, long, default_value = "markdown")]
    pub format: String,

    /// HTTP method to use for the request.
    /// Use HEAD to check headers without downloading content.
    /// Supported values: GET (default), HEAD, POST.
    #[arg(long, default_value = "GET", value_name = "METHOD")]
    pub method: String,

    /// Request timeout in seconds (0 means no timeout).
    #[arg(short, long, default_value = "30")]
    pub timeout: u64,

    /// Number of retries for 5xx server errors (default: 3).
    /// Set to 0 to disable retries.
    #[arg(long, default_value = "3")]
    pub retries: u32,

    /// Custom User-Agent string to identify the request.
    /// Common examples:
    ///   Mozilla/5.0 (compatible; Googlebot/2.1)   - Googlebot
    ///   Mozilla/5.0 (Windows NT 10.0; Win64)      - Windows browser
    ///   curl/7.68.0                               - curl client
    ///   PostmanRuntime/7.29.0                     - Postman
    /// Some sites block requests from unknown user agents.
    #[arg(long)]
    pub user_agent: Option<String>,

    /// Custom headers (format: "Header-Name: value").
    #[arg(long = "header", short = 'H', value_name = "HEADER")]
    pub headers: Vec<String>,

    /// Cookie to send with the request (format: "name=value").
    /// Can be specified multiple times for multiple cookies.
    /// Example: --cookie "session=abc123" --cookie "user=john"
    #[arg(long = "cookie", value_name = "COOKIE")]
    pub cookies: Vec<String>,

    /// Disable following HTTP redirects.
    /// By default, up to 10 redirects are followed.
    #[arg(long)]
    pub no_follow_redirects: bool,

    /// Strip images from output.
    #[arg(long)]
    pub no_images: bool,

    /// Strip links from output (keep text only).
    #[arg(long)]
    pub no_links: bool,

    /// CSS selectors to extract specific elements from the page.
    /// Can be specified multiple times to combine selectors.
    /// Examples:
    ///   article           - Select all <article> elements
    ///   .content          - Select elements with class "content"
    ///   #main             - Select element with id "main"
    ///   div.post > p      - Select <p> children of div.post
    ///   table tbody tr    - Select table rows in tbody
    ///   h1, h2, h3        - Select multiple heading levels
    ///   [data-id="123"]   - Select by attribute
    /// Multiple selectors: --selector "h1" --selector "p"
    #[arg(long, value_name = "SELECTOR", action = clap::ArgAction::Append, conflicts_with = "xpath")]
    pub selector: Vec<String>,

    /// XPath expression to extract specific elements from the page.
    /// Examples:
    ///   //article                      - Select all <article> elements
    ///   //div[@class='content']        - Select div with class "content"
    ///   //*[@id='main']                - Select element with id "main"
    ///   //div[@class='post']/p         - Select <p> children of div.post
    ///   //table/tbody/tr               - Select table rows in tbody
    ///   //h1 | //h2 | //h3             - Select multiple heading levels
    /// Note: XPath support requires external parsing and may be slower than CSS.
    #[arg(long, value_name = "XPATH", conflicts_with = "selector")]
    pub xpath: Option<String>,

    /// Show verbose output (includes fetching info).
    #[arg(short, long)]
    pub verbose: bool,

    /// Pretty-print JSON and XML responses with proper formatting.
    #[arg(long)]
    pub pretty: bool,
}

impl ScrapeCommand {
    /// Run the scrape command.
    pub async fn run(self) -> Result<()> {
        // Validate URL is not empty
        if self.url.trim().is_empty() {
            bail!("URL cannot be empty");
        }

        // Validate URL for security (control characters, etc.) (#2448)
        validate_url_security(&self.url)?;

        // Validate URL protocol - only http:// and https:// are supported (#2004)
        let url_lower = self.url.to_lowercase();
        if !url_lower.starts_with("http://") && !url_lower.starts_with("https://") {
            // Check for common unsupported protocols to give a helpful error
            let protocol = self.url.split("://").next().unwrap_or("");
            bail!(
                "Unsupported protocol '{}'. Use http:// or https://\n\n\
                Examples:\n\
                  cortex scrape https://example.com\n\
                  cortex scrape http://localhost:8080/page",
                protocol
            );
        }

        // Parse output format
        let format: OutputFormat = self.format.parse()?;

        // Build HTTP client with redirect policy and cookie store
        // Cookie store is enabled to persist cookies across redirects, which is
        // required for auth-gated pages that set cookies then redirect.
        let redirect_policy = if self.no_follow_redirects {
            reqwest::redirect::Policy::none()
        } else {
            reqwest::redirect::Policy::limited(10)
        };
        let mut client_builder = create_client_builder()
            .redirect(redirect_policy)
            .cookie_store(true);

        // Override timeout if specified (0 means no timeout)
        // Apply timeout to both connection phase and overall request
        if self.timeout > 0 {
            let timeout = Duration::from_secs(self.timeout);
            client_builder = client_builder.timeout(timeout).connect_timeout(timeout); // Ensure TCP connection respects timeout too
        }

        // Override user agent if specified
        if let Some(ref user_agent) = self.user_agent {
            client_builder = client_builder.user_agent(user_agent);
        }

        let client = client_builder
            .build()
            .context("Failed to build HTTP client")?;

        // Parse the HTTP method
        let method_upper = self.method.to_uppercase();

        // Add custom headers
        let parsed_headers = parse_headers(&self.headers)?;

        // Build cookie header from --cookie flags
        let cookie_header = if !self.cookies.is_empty() {
            Some(self.cookies.join("; "))
        } else {
            None
        };

        if self.verbose {
            eprintln!("Fetching: {} (method: {})", self.url, method_upper);
        }

        // Build request based on HTTP method
        let mut request = match method_upper.as_str() {
            "GET" => client.get(&self.url),
            "HEAD" => client.head(&self.url),
            "POST" => client.post(&self.url),
            _ => bail!(
                "Unsupported HTTP method: {}. Use GET, HEAD, or POST.",
                self.method
            ),
        };

        // Add custom headers
        for (name, value) in &parsed_headers {
            request = request.header(name.as_str(), value.as_str());
        }

        // Add cookies if specified
        if let Some(ref cookies) = cookie_header {
            request = request.header("Cookie", cookies.as_str());
        }

        let response = request.send().await.context("Failed to fetch URL")?;

        // For HEAD requests, just show headers and return
        if method_upper == "HEAD" {
            if !response.status().is_success() {
                bail!("{}", format_http_error(&response));
            }

            let content_type = response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html");

            println!("Status: {}", response.status());
            println!("Content-Type: {content_type}");
            if let Some(content_length) = response.headers().get("content-length") {
                println!(
                    "Content-Length: {}",
                    content_length.to_str().unwrap_or("unknown")
                );
            }
            if let Some(last_modified) = response.headers().get("last-modified") {
                println!(
                    "Last-Modified: {}",
                    last_modified.to_str().unwrap_or("unknown")
                );
            }
            return Ok(());
        }

        // For GET/POST, check response status

        if !response.status().is_success() {
            bail!(
                "HTTP error: {} {}",
                response.status().as_u16(),
                response.status().canonical_reason().unwrap_or("Unknown")
            );
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("text/html")
            .to_string();

        // Capture Content-Length header before consuming response
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());

        if self.verbose {
            eprintln!("Content-Type: {content_type}");
            if let Some(len) = content_length {
                eprintln!("Content-Length: {} bytes", len);
            }
        }

        let body = response
            .text()
            .await
            .context("Failed to read response body")?;

        if self.verbose {
            eprintln!("Received {} bytes", body.len());
        }

        // Detect content type and handle JSON/XML specially (Issues #1979, #1980)
        let content_type_lower = content_type.to_lowercase();
        let is_json =
            content_type_lower.contains("application/json") || content_type_lower.contains("+json");
        let is_xml = content_type_lower.contains("application/xml")
            || content_type_lower.contains("text/xml")
            || content_type_lower.contains("+xml");

        let output = if is_json {
            // Handle JSON response (Issue #1979)
            if self.pretty {
                // Pretty-print JSON
                match serde_json::from_str::<serde_json::Value>(&body) {
                    Ok(json_value) => {
                        serde_json::to_string_pretty(&json_value).unwrap_or_else(|_| body.clone())
                    }
                    Err(_) => body, // If parsing fails, return raw body
                }
            } else {
                body
            }
        } else if is_xml {
            // Handle XML response (Issue #1980)
            if self.pretty { format_xml(&body) } else { body }
        } else {
            // Parse and convert HTML
            self.process_html(&body, format)?
        };

        // Write output
        match &self.output {
            Some(path) => {
                std::fs::write(path, &output)
                    .with_context(|| format!("Failed to write to: {}", path.display()))?;
                if self.verbose {
                    eprintln!("Output written to: {}", path.display());
                }
            }
            None => {
                print!("{output}");
                std::io::stdout().flush()?;
            }
        }

        Ok(())
    }

    /// Process HTML content based on options.
    fn process_html(&self, html: &str, format: OutputFormat) -> Result<String> {
        let document = Html::parse_document(html);

        // If an XPath is provided, convert to CSS selector for common patterns
        let effective_selectors = if let Some(xpath_str) = &self.xpath {
            vec![xpath_to_css_selector(xpath_str)?]
        } else {
            self.selector.clone()
        };

        // If selectors are provided, extract only that content
        let content_html = if !effective_selectors.is_empty() {
            let mut selected = String::new();
            let mut matched_any = false;

            for selector_str in &effective_selectors {
                let selector = Selector::parse(selector_str).map_err(|e| {
                    anyhow::anyhow!("Invalid CSS selector '{}': {e:?}", selector_str)
                })?;

                for element in document.select(&selector) {
                    selected.push_str(&element.html());
                    matched_any = true;
                }
            }
            if !matched_any {
                let selectors_display = effective_selectors.join(", ");
                bail!("No elements matched selectors: {selectors_display}");
            }
            selected
        } else {
            // Extract main content, skipping nav, footer, ads, etc.
            extract_main_content(&document)
        };

        // Convert based on format
        let output = match format {
            OutputFormat::Markdown => {
                html_to_markdown(&content_html, self.no_images, self.no_links)
            }
            OutputFormat::Text => html_to_text(&content_html),
            OutputFormat::Html => clean_html(&content_html, self.no_images, self.no_links),
        };

        Ok(output)
    }
}
