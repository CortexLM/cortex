//! Jira REST API client.
//!
//! Provides a client for interacting with the Jira REST API,
//! supporting bi-directional synchronization with issue reading,
//! creation, updates, comments, and status transitions.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::api_client::create_default_client;

/// Jira Cloud API version prefix.
const JIRA_API_PREFIX: &str = "/rest/api/3";

/// Rate limit: 500 requests per 5 minutes for Jira Cloud.
const RATE_LIMIT_REQUESTS: u32 = 500;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(300);

/// Retry configuration for transient failures.
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 100;

/// URL pattern for Jira Cloud issue URLs.
static JIRA_CLOUD_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://([^/]+)\.atlassian\.net/browse/([A-Z]+-\d+)")
        .expect("Invalid Jira Cloud URL regex")
});

/// URL pattern for Jira Server/Data Center issue URLs.
/// NOTE: This pattern may also match Cloud URLs, so we filter in code.
static JIRA_SERVER_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    // Matches URLs like https://jira.company.com/browse/PROJ-123
    Regex::new(r"https?://([^/]+)/browse/([A-Z]+-\d+)").expect("Invalid Jira Server URL regex")
});

/// Short reference pattern for Jira issues (e.g., PROJ-123).
static JIRA_SHORT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z]+-\d+)\b").expect("Invalid Jira short ref regex"));

/// Reference to a Jira issue extracted from text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JiraRef {
    /// Instance identifier (subdomain for cloud, empty for server).
    pub instance: String,
    /// Issue key (e.g., "PROJ-123").
    pub key: String,
}

/// Extract Jira issue references from text.
///
/// Detects both full URLs (Cloud and Server) and short references
/// (if default_instance is provided).
pub fn extract_jira_issues(text: &str, default_instance: Option<&str>) -> Vec<JiraRef> {
    let mut refs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Match Jira Cloud URLs
    for cap in JIRA_CLOUD_URL_PATTERN.captures_iter(text) {
        let instance = cap[1].to_string();
        let key = cap[2].to_string();
        let unique_key = format!("{}:{}", instance, key);
        if seen.insert(unique_key) {
            refs.push(JiraRef { instance, key });
        }
    }

    // Match Jira Server URLs (no instance subdomain)
    // Skip atlassian.net URLs as they are handled by the Cloud pattern
    for cap in JIRA_SERVER_URL_PATTERN.captures_iter(text) {
        let host = &cap[1];
        // Skip if this is a Jira Cloud URL (already handled above)
        if host.ends_with(".atlassian.net") {
            continue;
        }
        let key = cap[2].to_string();
        let instance = default_instance.unwrap_or("").to_string();
        let unique_key = format!("{}:{}", instance, key);
        if seen.insert(unique_key) {
            refs.push(JiraRef { instance, key });
        }
    }

    // Match short references (only if default_instance is provided)
    if let Some(inst) = default_instance {
        for cap in JIRA_SHORT_PATTERN.captures_iter(text) {
            let key = cap[1].to_string();
            let unique_key = format!("{}:{}", inst, key);
            if seen.insert(unique_key) {
                refs.push(JiraRef {
                    instance: inst.to_string(),
                    key,
                });
            }
        }
    }

    refs
}

/// Jira API client.
pub struct JiraClient {
    client: reqwest::Client,
    base_url: String,
    email: String,
    api_token: String,
    rate_limiter: Mutex<RateLimiter>,
    /// Default instance for short references.
    pub default_instance: Option<String>,
}

impl JiraClient {
    /// Create a new authenticated Jira client for Cloud.
    ///
    /// # Arguments
    /// * `instance` - The Atlassian instance subdomain (e.g., "mycompany" for mycompany.atlassian.net)
    /// * `email` - User email for authentication
    /// * `api_token` - API token for authentication
    pub fn new_cloud(instance: &str, email: &str, api_token: &str) -> Result<Self> {
        let base_url = format!("https://{}.atlassian.net", instance);
        Self::new(&base_url, email, api_token, Some(instance.to_string()))
    }

    /// Create a new authenticated Jira client for Server/Data Center.
    ///
    /// # Arguments
    /// * `base_url` - The base URL of the Jira server (e.g., "https://jira.mycompany.com")
    /// * `email` - User email for authentication
    /// * `api_token` - API token or personal access token
    pub fn new_server(base_url: &str, email: &str, api_token: &str) -> Result<Self> {
        Self::new(base_url, email, api_token, None)
    }

    /// Create a new authenticated Jira client.
    fn new(
        base_url: &str,
        email: &str,
        api_token: &str,
        default_instance: Option<String>,
    ) -> Result<Self> {
        let client = create_default_client().context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
            email: email.to_string(),
            api_token: api_token.to_string(),
            rate_limiter: Mutex::new(RateLimiter::new(RATE_LIMIT_REQUESTS, RATE_LIMIT_WINDOW)),
            default_instance,
        })
    }

    /// Build the API URL for a given endpoint.
    fn api_url(&self, endpoint: &str) -> String {
        format!("{}{}{}", self.base_url, JIRA_API_PREFIX, endpoint)
    }

    /// Execute an HTTP request with rate limiting.
    async fn execute<T: for<'de> Deserialize<'de>>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T> {
        // Check rate limit
        self.rate_limiter.lock().await.check_rate_limit().await?;

        let response = request
            .basic_auth(&self.email, Some(&self.api_token))
            .send()
            .await
            .context("Failed to send request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Jira API error ({}): {}", status, body);
        }

        response.json().await.context("Failed to parse response")
    }

    /// Execute a request with automatic retry on transient failures.
    async fn execute_with_retry<T: for<'de> Deserialize<'de>>(
        &self,
        build_request: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<T> {
        let mut last_error = None;
        let mut delay = Duration::from_millis(INITIAL_RETRY_DELAY_MS);

        for attempt in 0..MAX_RETRIES {
            match self.execute::<T>(build_request()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    let error_str = e.to_string();
                    // Retry on transient errors (rate limits, timeouts, 5xx)
                    if error_str.contains("429")
                        || error_str.contains("503")
                        || error_str.contains("502")
                        || error_str.contains("504")
                        || error_str.contains("timeout")
                    {
                        if attempt < MAX_RETRIES - 1 {
                            tokio::time::sleep(delay).await;
                            delay *= 2; // Exponential backoff
                            last_error = Some(e);
                            continue;
                        }
                    }
                    return Err(e);
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("Max retries exceeded")))
    }

    /// Get issue by key (e.g., "PROJ-123").
    pub async fn get_issue(&self, key: &str) -> Result<JiraIssue> {
        let url = self.api_url(&format!("/issue/{}", key));
        let response: JiraIssueResponse = self
            .execute_with_retry(|| {
                self.client
                    .get(&url)
                    .query(&[("expand", "renderedFields,names,changelog")])
            })
            .await?;

        Ok(response.into())
    }

    /// Get detailed issue information including comments.
    pub async fn get_issue_details(&self, key: &str) -> Result<JiraIssueDetails> {
        let issue = self.get_issue(key).await?;
        let comments = self.get_issue_comments(key).await?;

        Ok(JiraIssueDetails {
            issue,
            comments,
            url: format!("{}/browse/{}", self.base_url, key),
        })
    }

    /// Get comments for an issue.
    pub async fn get_issue_comments(&self, key: &str) -> Result<Vec<JiraComment>> {
        let url = self.api_url(&format!("/issue/{}/comment", key));
        let response: CommentsResponse = self.execute_with_retry(|| self.client.get(&url)).await?;

        Ok(response.comments)
    }

    /// Create a new issue.
    pub async fn create_issue(&self, input: CreateIssueInput) -> Result<JiraIssue> {
        let url = self.api_url("/issue");

        let mut fields: HashMap<String, serde_json::Value> = HashMap::new();
        fields.insert(
            "project".to_string(),
            serde_json::json!({ "key": input.project_key }),
        );
        fields.insert("summary".to_string(), serde_json::json!(input.summary));
        fields.insert(
            "issuetype".to_string(),
            serde_json::json!({ "name": input.issue_type }),
        );

        if let Some(desc) = input.description {
            // Jira Cloud uses Atlassian Document Format (ADF)
            fields.insert(
                "description".to_string(),
                serde_json::json!({
                    "type": "doc",
                    "version": 1,
                    "content": [{
                        "type": "paragraph",
                        "content": [{
                            "type": "text",
                            "text": desc
                        }]
                    }]
                }),
            );
        }

        if let Some(priority) = input.priority {
            fields.insert(
                "priority".to_string(),
                serde_json::json!({ "name": priority }),
            );
        }

        if let Some(assignee) = input.assignee_id {
            fields.insert(
                "assignee".to_string(),
                serde_json::json!({ "accountId": assignee }),
            );
        }

        if let Some(labels) = input.labels {
            fields.insert("labels".to_string(), serde_json::json!(labels));
        }

        let body = serde_json::json!({ "fields": fields });

        let response: CreateIssueResponse = self
            .execute_with_retry(|| self.client.post(&url).json(&body))
            .await?;

        // Fetch the created issue to get full details
        self.get_issue(&response.key).await
    }

    /// Add a comment to an issue.
    pub async fn add_comment(&self, key: &str, body: &str) -> Result<JiraComment> {
        let url = self.api_url(&format!("/issue/{}/comment", key));

        // Jira Cloud uses Atlassian Document Format (ADF)
        let comment_body = serde_json::json!({
            "body": {
                "type": "doc",
                "version": 1,
                "content": [{
                    "type": "paragraph",
                    "content": [{
                        "type": "text",
                        "text": body
                    }]
                }]
            }
        });

        self.execute_with_retry(|| self.client.post(&url).json(&comment_body))
            .await
    }

    /// Get available transitions for an issue.
    pub async fn get_transitions(&self, key: &str) -> Result<Vec<JiraTransition>> {
        let url = self.api_url(&format!("/issue/{}/transitions", key));
        let response: TransitionsResponse =
            self.execute_with_retry(|| self.client.get(&url)).await?;

        Ok(response.transitions)
    }

    /// Transition an issue to a new status.
    pub async fn transition_issue(&self, key: &str, transition_id: &str) -> Result<()> {
        let url = self.api_url(&format!("/issue/{}/transitions", key));

        let body = serde_json::json!({
            "transition": {
                "id": transition_id
            }
        });

        // Check rate limit
        self.rate_limiter.lock().await.check_rate_limit().await?;

        let response = self
            .client
            .post(&url)
            .basic_auth(&self.email, Some(&self.api_token))
            .json(&body)
            .send()
            .await
            .context("Failed to send transition request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Failed to transition issue ({}): {}", status, body);
        }

        Ok(())
    }

    /// Update issue fields.
    pub async fn update_issue(
        &self,
        key: &str,
        fields: HashMap<String, serde_json::Value>,
    ) -> Result<()> {
        let url = self.api_url(&format!("/issue/{}", key));

        let body = serde_json::json!({ "fields": fields });

        // Check rate limit
        self.rate_limiter.lock().await.check_rate_limit().await?;

        let response = self
            .client
            .put(&url)
            .basic_auth(&self.email, Some(&self.api_token))
            .json(&body)
            .send()
            .await
            .context("Failed to send update request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Failed to update issue ({}): {}", status, body);
        }

        Ok(())
    }

    /// Assign issue to a user.
    pub async fn assign_issue(&self, key: &str, account_id: Option<&str>) -> Result<()> {
        let url = self.api_url(&format!("/issue/{}/assignee", key));

        let body = match account_id {
            Some(id) => serde_json::json!({ "accountId": id }),
            None => serde_json::json!({ "accountId": null }),
        };

        // Check rate limit
        self.rate_limiter.lock().await.check_rate_limit().await?;

        let response = self
            .client
            .put(&url)
            .basic_auth(&self.email, Some(&self.api_token))
            .json(&body)
            .send()
            .await
            .context("Failed to send assignee request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Failed to assign issue ({}): {}", status, body);
        }

        Ok(())
    }

    /// Get all projects.
    pub async fn get_projects(&self) -> Result<Vec<JiraProject>> {
        let url = self.api_url("/project");
        self.execute_with_retry(|| self.client.get(&url)).await
    }

    /// Get issue types for a project.
    pub async fn get_issue_types(&self, project_key: &str) -> Result<Vec<JiraIssueType>> {
        let url = self.api_url(&format!("/project/{}", project_key));
        let response: ProjectResponse = self.execute_with_retry(|| self.client.get(&url)).await?;

        Ok(response.issue_types)
    }

    /// Get the current user.
    pub async fn get_myself(&self) -> Result<JiraUser> {
        let url = self.api_url("/myself");
        self.execute_with_retry(|| self.client.get(&url)).await
    }

    /// Search for issues using JQL.
    pub async fn search_issues(&self, jql: &str, max_results: u32) -> Result<Vec<JiraIssue>> {
        let url = self.api_url("/search");
        let response: SearchResponse = self
            .execute_with_retry(|| {
                self.client
                    .get(&url)
                    .query(&[("jql", jql), ("maxResults", &max_results.to_string())])
            })
            .await?;

        Ok(response.issues.into_iter().map(|r| r.into()).collect())
    }

    /// Extract issue references from text and fetch their details.
    pub async fn enrich_context(&self, text: &str) -> Result<Vec<JiraIssueDetails>> {
        let refs = extract_jira_issues(text, self.default_instance.as_deref());
        let mut issues = Vec::new();

        for jira_ref in refs {
            match self.get_issue_details(&jira_ref.key).await {
                Ok(issue) => issues.push(issue),
                Err(e) => {
                    tracing::warn!("Failed to fetch Jira issue {}: {}", jira_ref.key, e);
                }
            }
        }

        Ok(issues)
    }
}

/// Input for creating a new issue.
#[derive(Debug, Clone, Default)]
pub struct CreateIssueInput {
    /// Project key (required).
    pub project_key: String,
    /// Issue summary/title (required).
    pub summary: String,
    /// Issue type name (required, e.g., "Bug", "Task", "Story").
    pub issue_type: String,
    /// Issue description (optional).
    pub description: Option<String>,
    /// Priority name (optional, e.g., "High", "Medium", "Low").
    pub priority: Option<String>,
    /// Assignee account ID (optional).
    pub assignee_id: Option<String>,
    /// Labels (optional).
    pub labels: Option<Vec<String>>,
}

/// Rate limiter for API requests.
struct RateLimiter {
    max_requests: u32,
    window: Duration,
    requests: Vec<Instant>,
}

impl RateLimiter {
    fn new(max_requests: u32, window: Duration) -> Self {
        Self {
            max_requests,
            window,
            requests: Vec::new(),
        }
    }

    async fn check_rate_limit(&mut self) -> Result<()> {
        let now = Instant::now();

        // Remove requests outside the window
        self.requests
            .retain(|&time| now.duration_since(time) < self.window);

        // Check if we've hit the limit
        if self.requests.len() >= self.max_requests as usize {
            let oldest = self.requests[0];
            let wait_time = self.window.saturating_sub(now.duration_since(oldest));

            if !wait_time.is_zero() {
                tokio::time::sleep(wait_time).await;
                // Clear old requests after waiting
                self.requests.clear();
            }
        }

        // Record this request
        self.requests.push(now);
        Ok(())
    }
}

// API response types

#[derive(Debug, Deserialize)]
struct JiraIssueResponse {
    id: String,
    key: String,
    fields: JiraIssueFields,
}

#[derive(Debug, Deserialize)]
struct JiraIssueFields {
    summary: String,
    description: Option<serde_json::Value>,
    status: JiraStatus,
    priority: Option<JiraPriority>,
    assignee: Option<JiraUser>,
    reporter: Option<JiraUser>,
    #[serde(rename = "issuetype")]
    issue_type: JiraIssueType,
    labels: Vec<String>,
    created: String,
    updated: String,
}

impl From<JiraIssueResponse> for JiraIssue {
    fn from(response: JiraIssueResponse) -> Self {
        JiraIssue {
            id: response.id,
            key: response.key,
            summary: response.fields.summary,
            description: response.fields.description.map(|d| {
                // Try to extract text from ADF format
                extract_text_from_adf(&d)
            }),
            status: response.fields.status,
            priority: response.fields.priority,
            assignee: response.fields.assignee,
            reporter: response.fields.reporter,
            issue_type: response.fields.issue_type,
            labels: response.fields.labels,
            created_at: response.fields.created,
            updated_at: response.fields.updated,
        }
    }
}

/// Extract plain text from Atlassian Document Format.
fn extract_text_from_adf(adf: &serde_json::Value) -> String {
    let mut text = String::new();

    fn walk(node: &serde_json::Value, text: &mut String) {
        if let Some(t) = node.get("text").and_then(|t| t.as_str()) {
            text.push_str(t);
        }
        if let Some(content) = node.get("content").and_then(|c| c.as_array()) {
            for child in content {
                walk(child, text);
            }
            // Add newline after paragraphs
            if node.get("type").and_then(|t| t.as_str()) == Some("paragraph") && !text.is_empty() {
                text.push('\n');
            }
        }
    }

    walk(adf, &mut text);
    text.trim().to_string()
}

#[derive(Debug, Deserialize)]
struct CommentsResponse {
    comments: Vec<JiraComment>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CreateIssueResponse {
    id: String,
    key: String,
}

#[derive(Debug, Deserialize)]
struct TransitionsResponse {
    transitions: Vec<JiraTransition>,
}

#[derive(Debug, Deserialize)]
struct ProjectResponse {
    #[serde(rename = "issueTypes")]
    issue_types: Vec<JiraIssueType>,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    issues: Vec<JiraIssueResponse>,
}

// Public API types

/// Jira issue information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssue {
    pub id: String,
    pub key: String,
    pub summary: String,
    pub description: Option<String>,
    pub status: JiraStatus,
    pub priority: Option<JiraPriority>,
    pub assignee: Option<JiraUser>,
    pub reporter: Option<JiraUser>,
    pub issue_type: JiraIssueType,
    pub labels: Vec<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
}

/// Detailed Jira issue with comments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssueDetails {
    #[serde(flatten)]
    pub issue: JiraIssue,
    pub comments: Vec<JiraComment>,
    pub url: String,
}

impl JiraIssueDetails {
    /// Format issue for context injection.
    pub fn to_context(&self) -> String {
        let mut parts = vec![format!("# {} - {}\n", self.issue.key, self.issue.summary)];

        parts.push(format!("**Type:** {}\n", self.issue.issue_type.name));
        parts.push(format!("**Status:** {}\n", self.issue.status.name));

        if let Some(ref priority) = self.issue.priority {
            parts.push(format!("**Priority:** {}\n", priority.name));
        }

        if let Some(ref assignee) = self.issue.assignee {
            parts.push(format!("**Assignee:** {}\n", assignee.display_name));
        } else {
            parts.push("**Assignee:** Unassigned\n".to_string());
        }

        if !self.issue.labels.is_empty() {
            parts.push(format!("**Labels:** {}\n", self.issue.labels.join(", ")));
        }

        parts.push(format!("**URL:** {}\n", self.url));

        if let Some(ref description) = self.issue.description {
            parts.push(format!("\n## Description\n\n{}\n", description));
        }

        if !self.comments.is_empty() {
            parts.push("\n## Comments\n".to_string());
            for comment in &self.comments {
                parts.push(format!(
                    "\n**{}** ({}):\n{}\n",
                    comment.author.display_name, comment.created, comment.body
                ));
            }
        }

        parts.join("")
    }
}

/// Jira issue status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraStatus {
    pub id: String,
    pub name: String,
    #[serde(rename = "statusCategory")]
    pub status_category: Option<JiraStatusCategory>,
}

/// Jira status category.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraStatusCategory {
    pub id: i32,
    pub key: String,
    pub name: String,
}

/// Jira priority.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraPriority {
    pub id: String,
    pub name: String,
}

/// Jira user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraUser {
    #[serde(rename = "accountId")]
    pub account_id: Option<String>,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "emailAddress")]
    pub email: Option<String>,
    pub active: Option<bool>,
}

/// Jira issue type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraIssueType {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "subtask")]
    pub is_subtask: bool,
}

/// Jira project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraProject {
    pub id: String,
    pub key: String,
    pub name: String,
    pub description: Option<String>,
}

/// Jira comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraComment {
    pub id: String,
    #[serde(deserialize_with = "deserialize_comment_body")]
    pub body: String,
    pub author: JiraUser,
    pub created: String,
    pub updated: String,
}

/// Custom deserializer for comment body (can be ADF or plain text).
fn deserialize_comment_body<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;

    // If it's a string, return as-is
    if let Some(s) = value.as_str() {
        return Ok(s.to_string());
    }

    // If it's an ADF document, extract text
    Ok(extract_text_from_adf(&value))
}

/// Jira workflow transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JiraTransition {
    pub id: String,
    pub name: String,
    pub to: JiraStatus,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_jira_issues_cloud_url() {
        let text = "Check https://mycompany.atlassian.net/browse/PROJ-123 for details";
        let refs = extract_jira_issues(text, None);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].instance, "mycompany");
        assert_eq!(refs[0].key, "PROJ-123");
    }

    #[test]
    fn test_extract_jira_issues_multiple_urls() {
        let text = "See https://team1.atlassian.net/browse/ABC-123 and https://team2.atlassian.net/browse/XYZ-456";
        let refs = extract_jira_issues(text, None);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].key, "ABC-123");
        assert_eq!(refs[1].key, "XYZ-456");
    }

    #[test]
    fn test_extract_jira_issues_short_reference() {
        let text = "Working on PROJ-123 and PROJ-456";
        let refs = extract_jira_issues(text, Some("mycompany"));
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].instance, "mycompany");
        assert_eq!(refs[0].key, "PROJ-123");
        assert_eq!(refs[1].key, "PROJ-456");
    }

    #[test]
    fn test_extract_jira_issues_no_short_without_instance() {
        let text = "Working on PROJ-123 without instance";
        let refs = extract_jira_issues(text, None);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_jira_issues_deduplication() {
        let text = "https://mycompany.atlassian.net/browse/PROJ-123 and PROJ-123 mentioned again";
        let refs = extract_jira_issues(text, Some("mycompany"));
        // Should only have one entry since same instance:key
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].key, "PROJ-123");
    }

    #[test]
    fn test_create_issue_input_default() {
        let input = CreateIssueInput::default();
        assert!(input.project_key.is_empty());
        assert!(input.summary.is_empty());
        assert!(input.issue_type.is_empty());
        assert!(input.description.is_none());
        assert!(input.priority.is_none());
        assert!(input.assignee_id.is_none());
        assert!(input.labels.is_none());
    }

    #[test]
    fn test_jira_ref_equality() {
        let ref1 = JiraRef {
            instance: "mycompany".to_string(),
            key: "PROJ-123".to_string(),
        };
        let ref2 = JiraRef {
            instance: "mycompany".to_string(),
            key: "PROJ-123".to_string(),
        };
        let ref3 = JiraRef {
            instance: "mycompany".to_string(),
            key: "PROJ-456".to_string(),
        };
        assert_eq!(ref1, ref2);
        assert_ne!(ref1, ref3);
    }

    #[test]
    fn test_extract_text_from_adf() {
        let adf = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [{
                "type": "paragraph",
                "content": [{
                    "type": "text",
                    "text": "Hello, world!"
                }]
            }]
        });
        let text = extract_text_from_adf(&adf);
        assert_eq!(text, "Hello, world!");
    }

    #[test]
    fn test_rate_limiter_creation() {
        let limiter = RateLimiter::new(100, Duration::from_secs(60));
        assert_eq!(limiter.max_requests, 100);
        assert_eq!(limiter.window, Duration::from_secs(60));
        assert!(limiter.requests.is_empty());
    }
}
