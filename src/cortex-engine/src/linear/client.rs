//! Linear API client.
//!
//! Provides a GraphQL client for interacting with the Linear API,
//! supporting bi-directional synchronization with issue reading,
//! creation, updates, comments, and status changes.

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

use crate::api_client::create_default_client;

/// Linear GraphQL endpoint.
const LINEAR_GRAPHQL_ENDPOINT: &str = "https://api.linear.app/graphql";

/// Linear OAuth authorization endpoint.
pub const LINEAR_OAUTH_AUTHORIZE: &str = "https://linear.app/oauth/authorize";

/// Linear OAuth token endpoint.
pub const LINEAR_OAUTH_TOKEN: &str = "https://api.linear.app/oauth/token";

/// Rate limit: 2000 requests per hour (Linear's limit).
const RATE_LIMIT_REQUESTS: u32 = 2000;
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(3600);

/// Retry configuration for transient failures.
const MAX_RETRIES: u32 = 3;
const INITIAL_RETRY_DELAY_MS: u64 = 100;

/// URL pattern for Linear issue URLs.
static LINEAR_URL_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https://linear\.app/([^/]+)/issue/([A-Z]+-\d+)").expect("Invalid Linear URL regex")
});

/// Short reference pattern for Linear issues (e.g., ABC-123).
static LINEAR_SHORT_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z]+-\d+)\b").expect("Invalid Linear short ref regex"));

/// Reference to a Linear issue extracted from text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinearRef {
    /// Workspace identifier (from URL or config).
    pub workspace: String,
    /// Issue identifier (e.g., "ABC-123").
    pub identifier: String,
}

/// Extract Linear issue references from text.
///
/// Detects both full URLs and short references (if default_workspace is provided).
pub fn extract_linear_issues(text: &str, default_workspace: Option<&str>) -> Vec<LinearRef> {
    let mut refs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Match full URLs
    for cap in LINEAR_URL_PATTERN.captures_iter(text) {
        let workspace = cap[1].to_string();
        let identifier = cap[2].to_string();
        let key = format!("{}:{}", workspace, identifier);
        if seen.insert(key) {
            refs.push(LinearRef {
                workspace,
                identifier,
            });
        }
    }

    // Match short references (only if default_workspace is provided)
    if let Some(ws) = default_workspace {
        for cap in LINEAR_SHORT_PATTERN.captures_iter(text) {
            let identifier = cap[1].to_string();
            // Skip if this identifier was already found via URL
            let key = format!("{}:{}", ws, identifier);
            if seen.insert(key) {
                refs.push(LinearRef {
                    workspace: ws.to_string(),
                    identifier,
                });
            }
        }
    }

    refs
}

/// Linear API client.
pub struct LinearClient {
    client: reqwest::Client,
    token: String,
    rate_limiter: Mutex<RateLimiter>,
    /// Default workspace for short references.
    pub default_workspace: Option<String>,
}

impl LinearClient {
    /// Create a new authenticated Linear client.
    pub fn new(token: &str) -> Result<Self> {
        Self::with_workspace(token, None)
    }

    /// Create a new authenticated Linear client with a default workspace.
    pub fn with_workspace(token: &str, default_workspace: Option<String>) -> Result<Self> {
        let client = create_default_client().context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            token: token.to_string(),
            rate_limiter: Mutex::new(RateLimiter::new(RATE_LIMIT_REQUESTS, RATE_LIMIT_WINDOW)),
            default_workspace,
        })
    }

    /// Execute a GraphQL query with automatic retry on transient failures.
    async fn execute_with_retry<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: Option<serde_json::Value>,
    ) -> Result<T> {
        let mut last_error = None;
        let mut delay = Duration::from_millis(INITIAL_RETRY_DELAY_MS);

        for attempt in 0..MAX_RETRIES {
            match self.execute_query::<T>(query, variables.clone()).await {
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

    /// Execute a GraphQL query.
    async fn execute_query<T: for<'de> Deserialize<'de>>(
        &self,
        query: &str,
        variables: Option<serde_json::Value>,
    ) -> Result<T> {
        // Check rate limit
        self.rate_limiter.lock().await.check_rate_limit().await?;

        let mut body = serde_json::json!({
            "query": query,
        });

        if let Some(vars) = variables {
            body["variables"] = vars;
        }

        let response = self
            .client
            .post(LINEAR_GRAPHQL_ENDPOINT)
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .context("Failed to send GraphQL request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("Linear API error ({}): {}", status, body);
        }

        let graphql_response: GraphQLResponse<T> = response
            .json()
            .await
            .context("Failed to parse GraphQL response")?;

        if let Some(errors) = graphql_response.errors {
            let error_messages: Vec<String> = errors.iter().map(|e| e.message.clone()).collect();
            bail!("GraphQL errors: {}", error_messages.join(", "));
        }

        graphql_response
            .data
            .ok_or_else(|| anyhow::anyhow!("No data in GraphQL response"))
    }

    /// Get all teams.
    pub async fn get_teams(&self) -> Result<Vec<Team>> {
        let query = r#"
            query {
                teams {
                    nodes {
                        id
                        name
                        key
                        description
                    }
                }
            }
        "#;

        let response: TeamsResponse = self.execute_query(query, None).await?;
        Ok(response.teams.nodes)
    }

    /// Get issues for a team.
    pub async fn get_issues(&self, team_id: &str) -> Result<Vec<Issue>> {
        let query = r#"
            query($teamId: String!) {
                team(id: $teamId) {
                    issues {
                        nodes {
                            id
                            identifier
                            title
                            description
                            priority
                            state {
                                id
                                name
                                type
                            }
                            assignee {
                                id
                                name
                                email
                            }
                            createdAt
                            updatedAt
                            url
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "teamId": team_id,
        });

        let response: TeamIssuesResponse = self.execute_query(query, Some(variables)).await?;
        Ok(response.team.issues.nodes)
    }

    /// Get issue details by ID.
    pub async fn get_issue_details(&self, issue_id: &str) -> Result<IssueDetails> {
        let query = r#"
            query($issueId: String!) {
                issue(id: $issueId) {
                    id
                    identifier
                    title
                    description
                    priority
                    state {
                        id
                        name
                        type
                    }
                    assignee {
                        id
                        name
                        email
                    }
                    creator {
                        id
                        name
                        email
                    }
                    team {
                        id
                        name
                        key
                    }
                    labels {
                        nodes {
                            id
                            name
                            color
                        }
                    }
                    comments {
                        nodes {
                            id
                            body
                            createdAt
                            user {
                                id
                                name
                            }
                        }
                    }
                    createdAt
                    updatedAt
                    url
                }
            }
        "#;

        let variables = serde_json::json!({
            "issueId": issue_id,
        });

        let response: IssueDetailsResponse = self.execute_query(query, Some(variables)).await?;
        Ok(response.issue)
    }

    /// Get issue by identifier (e.g., "ABC-123") - searches across workspaces.
    pub async fn get_issue_by_identifier(&self, identifier: &str) -> Result<IssueDetails> {
        let query = r#"
            query($filter: IssueFilter!) {
                issues(filter: $filter, first: 1) {
                    nodes {
                        id
                        identifier
                        title
                        description
                        priority
                        state {
                            id
                            name
                            type
                        }
                        assignee {
                            id
                            name
                            email
                        }
                        creator {
                            id
                            name
                            email
                        }
                        team {
                            id
                            name
                            key
                        }
                        labels {
                            nodes {
                                id
                                name
                                color
                            }
                        }
                        comments {
                            nodes {
                                id
                                body
                                createdAt
                                user {
                                    id
                                    name
                                }
                            }
                        }
                        createdAt
                        updatedAt
                        url
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "filter": {
                "identifier": { "eq": identifier }
            }
        });

        let response: IssuesQueryResponse = self.execute_with_retry(query, Some(variables)).await?;

        response
            .issues
            .nodes
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Issue not found: {}", identifier))
    }

    /// Create a new issue.
    pub async fn create_issue(&self, input: CreateIssueInput) -> Result<Issue> {
        let query = r#"
            mutation($input: IssueCreateInput!) {
                issueCreate(input: $input) {
                    success
                    issue {
                        id
                        identifier
                        title
                        description
                        priority
                        state {
                            id
                            name
                            type
                        }
                        assignee {
                            id
                            name
                            email
                        }
                        createdAt
                        updatedAt
                        url
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "input": {
                "teamId": input.team_id,
                "title": input.title,
                "description": input.description,
                "priority": input.priority,
                "assigneeId": input.assignee_id,
                "labelIds": input.label_ids,
                "stateId": input.state_id,
            }
        });

        let response: CreateIssueResponse = self.execute_with_retry(query, Some(variables)).await?;

        if !response.issue_create.success {
            bail!("Failed to create issue");
        }

        response
            .issue_create
            .issue
            .ok_or_else(|| anyhow::anyhow!("Issue creation succeeded but no issue returned"))
    }

    /// Add a comment to an issue.
    pub async fn add_comment(&self, issue_id: &str, body: &str) -> Result<Comment> {
        let query = r#"
            mutation($input: CommentCreateInput!) {
                commentCreate(input: $input) {
                    success
                    comment {
                        id
                        body
                        createdAt
                        user {
                            id
                            name
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "input": {
                "issueId": issue_id,
                "body": body
            }
        });

        let response: CreateCommentResponse =
            self.execute_with_retry(query, Some(variables)).await?;

        if !response.comment_create.success {
            bail!("Failed to create comment");
        }

        response
            .comment_create
            .comment
            .ok_or_else(|| anyhow::anyhow!("Comment creation succeeded but no comment returned"))
    }

    /// Update issue state (status).
    pub async fn update_issue_state(&self, issue_id: &str, state_id: &str) -> Result<Issue> {
        let query = r#"
            mutation($id: String!, $input: IssueUpdateInput!) {
                issueUpdate(id: $id, input: $input) {
                    success
                    issue {
                        id
                        identifier
                        title
                        description
                        priority
                        state {
                            id
                            name
                            type
                        }
                        assignee {
                            id
                            name
                            email
                        }
                        createdAt
                        updatedAt
                        url
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "id": issue_id,
            "input": {
                "stateId": state_id
            }
        });

        let response: UpdateIssueResponse = self.execute_with_retry(query, Some(variables)).await?;

        if !response.issue_update.success {
            bail!("Failed to update issue state");
        }

        response
            .issue_update
            .issue
            .ok_or_else(|| anyhow::anyhow!("Issue update succeeded but no issue returned"))
    }

    /// Update issue priority.
    pub async fn update_issue_priority(&self, issue_id: &str, priority: u32) -> Result<Issue> {
        let query = r#"
            mutation($id: String!, $input: IssueUpdateInput!) {
                issueUpdate(id: $id, input: $input) {
                    success
                    issue {
                        id
                        identifier
                        title
                        description
                        priority
                        state {
                            id
                            name
                            type
                        }
                        assignee {
                            id
                            name
                            email
                        }
                        createdAt
                        updatedAt
                        url
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "id": issue_id,
            "input": {
                "priority": priority
            }
        });

        let response: UpdateIssueResponse = self.execute_with_retry(query, Some(variables)).await?;

        if !response.issue_update.success {
            bail!("Failed to update issue priority");
        }

        response
            .issue_update
            .issue
            .ok_or_else(|| anyhow::anyhow!("Issue update succeeded but no issue returned"))
    }

    /// Update issue assignee.
    pub async fn update_issue_assignee(
        &self,
        issue_id: &str,
        assignee_id: Option<&str>,
    ) -> Result<Issue> {
        let query = r#"
            mutation($id: String!, $input: IssueUpdateInput!) {
                issueUpdate(id: $id, input: $input) {
                    success
                    issue {
                        id
                        identifier
                        title
                        description
                        priority
                        state {
                            id
                            name
                            type
                        }
                        assignee {
                            id
                            name
                            email
                        }
                        createdAt
                        updatedAt
                        url
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "id": issue_id,
            "input": {
                "assigneeId": assignee_id
            }
        });

        let response: UpdateIssueResponse = self.execute_with_retry(query, Some(variables)).await?;

        if !response.issue_update.success {
            bail!("Failed to update issue assignee");
        }

        response
            .issue_update
            .issue
            .ok_or_else(|| anyhow::anyhow!("Issue update succeeded but no issue returned"))
    }

    /// Get workflow states for a team.
    pub async fn get_team_states(&self, team_id: &str) -> Result<Vec<IssueState>> {
        let query = r#"
            query($teamId: String!) {
                team(id: $teamId) {
                    states {
                        nodes {
                            id
                            name
                            type
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "teamId": team_id,
        });

        let response: TeamStatesResponse = self.execute_query(query, Some(variables)).await?;
        Ok(response.team.states.nodes)
    }

    /// Get the current user info.
    pub async fn get_viewer(&self) -> Result<User> {
        let query = r#"
            query {
                viewer {
                    id
                    name
                    email
                }
            }
        "#;

        let response: ViewerResponse = self.execute_query(query, None).await?;
        Ok(response.viewer)
    }

    /// Extract issue references from text and fetch their details.
    pub async fn enrich_context(&self, text: &str) -> Result<Vec<IssueDetails>> {
        let refs = extract_linear_issues(text, self.default_workspace.as_deref());
        let mut issues = Vec::new();

        for linear_ref in refs {
            match self.get_issue_by_identifier(&linear_ref.identifier).await {
                Ok(issue) => issues.push(issue),
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch Linear issue {}: {}",
                        linear_ref.identifier,
                        e
                    );
                }
            }
        }

        Ok(issues)
    }
}

/// Input for creating a new issue.
#[derive(Debug, Clone, Default)]
pub struct CreateIssueInput {
    /// Team ID (required).
    pub team_id: String,
    /// Issue title (required).
    pub title: String,
    /// Issue description (optional).
    pub description: Option<String>,
    /// Priority (0-4, 0=No priority, 4=Urgent) (optional).
    pub priority: Option<u32>,
    /// Assignee user ID (optional).
    pub assignee_id: Option<String>,
    /// Label IDs (optional).
    pub label_ids: Option<Vec<String>>,
    /// Initial state ID (optional).
    pub state_id: Option<String>,
}

// Internal response types for mutations

#[derive(Debug, Deserialize)]
struct IssuesQueryResponse {
    issues: IssueDetailsConnection,
}

#[derive(Debug, Deserialize)]
struct IssueDetailsConnection {
    nodes: Vec<IssueDetails>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateIssueResponse {
    issue_create: IssueCreatePayload,
}

#[derive(Debug, Deserialize)]
struct IssueCreatePayload {
    success: bool,
    issue: Option<Issue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateIssueResponse {
    issue_update: IssueUpdatePayload,
}

#[derive(Debug, Deserialize)]
struct IssueUpdatePayload {
    success: bool,
    issue: Option<Issue>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateCommentResponse {
    comment_create: CommentCreatePayload,
}

#[derive(Debug, Deserialize)]
struct CommentCreatePayload {
    success: bool,
    comment: Option<Comment>,
}

#[derive(Debug, Deserialize)]
struct TeamStatesResponse {
    team: TeamWithStates,
}

#[derive(Debug, Deserialize)]
struct TeamWithStates {
    states: StatesConnection,
}

#[derive(Debug, Deserialize)]
struct StatesConnection {
    nodes: Vec<IssueState>,
}

#[derive(Debug, Deserialize)]
struct ViewerResponse {
    viewer: User,
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

// GraphQL response types

#[derive(Debug, Deserialize)]
struct GraphQLResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQLError>>,
}

#[derive(Debug, Deserialize)]
struct GraphQLError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct TeamsResponse {
    teams: TeamsConnection,
}

#[derive(Debug, Deserialize)]
struct TeamsConnection {
    nodes: Vec<Team>,
}

#[derive(Debug, Deserialize)]
struct TeamIssuesResponse {
    team: TeamWithIssues,
}

#[derive(Debug, Deserialize)]
struct TeamWithIssues {
    issues: IssuesConnection,
}

#[derive(Debug, Deserialize)]
struct IssuesConnection {
    nodes: Vec<Issue>,
}

#[derive(Debug, Deserialize)]
struct IssueDetailsResponse {
    issue: IssueDetails,
}

// Public API types

/// Linear team information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub key: String,
    pub description: Option<String>,
}

/// Linear issue information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: u32,
    pub state: IssueState,
    pub assignee: Option<User>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub url: String,
}

/// Detailed issue information with comments and labels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueDetails {
    pub id: String,
    pub identifier: String,
    pub title: String,
    pub description: Option<String>,
    pub priority: u32,
    pub state: IssueState,
    pub assignee: Option<User>,
    pub creator: User,
    pub team: TeamInfo,
    pub labels: LabelsConnection,
    pub comments: CommentsConnection,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    #[serde(rename = "updatedAt")]
    pub updated_at: String,
    pub url: String,
}

/// Issue state information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueState {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub state_type: String,
}

/// User information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
}

/// Team information (minimal).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamInfo {
    pub id: String,
    pub name: String,
    pub key: String,
}

/// Label information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    pub color: String,
}

/// Labels connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LabelsConnection {
    pub nodes: Vec<Label>,
}

/// Comment information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub body: String,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub user: User,
}

/// Comments connection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentsConnection {
    pub nodes: Vec<Comment>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rate_limiter_creation() {
        let limiter = RateLimiter::new(100, Duration::from_secs(60));
        assert_eq!(limiter.max_requests, 100);
        assert_eq!(limiter.window, Duration::from_secs(60));
        assert!(limiter.requests.is_empty());
    }

    #[test]
    fn test_linear_endpoints() {
        assert_eq!(LINEAR_GRAPHQL_ENDPOINT, "https://api.linear.app/graphql");
        assert_eq!(LINEAR_OAUTH_AUTHORIZE, "https://linear.app/oauth/authorize");
        assert_eq!(LINEAR_OAUTH_TOKEN, "https://api.linear.app/oauth/token");
    }

    #[test]
    fn test_extract_linear_issues_from_url() {
        let text = "Check https://linear.app/myteam/issue/ABC-123 for details";
        let refs = extract_linear_issues(text, None);
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].workspace, "myteam");
        assert_eq!(refs[0].identifier, "ABC-123");
    }

    #[test]
    fn test_extract_linear_issues_multiple_urls() {
        let text =
            "See https://linear.app/team1/issue/ABC-123 and https://linear.app/team2/issue/XYZ-456";
        let refs = extract_linear_issues(text, None);
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].identifier, "ABC-123");
        assert_eq!(refs[1].identifier, "XYZ-456");
    }

    #[test]
    fn test_extract_linear_issues_short_reference() {
        let text = "Working on ABC-123 and DEF-456";
        let refs = extract_linear_issues(text, Some("myteam"));
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].workspace, "myteam");
        assert_eq!(refs[0].identifier, "ABC-123");
        assert_eq!(refs[1].identifier, "DEF-456");
    }

    #[test]
    fn test_extract_linear_issues_no_short_without_workspace() {
        let text = "Working on ABC-123 without workspace";
        let refs = extract_linear_issues(text, None);
        assert!(refs.is_empty());
    }

    #[test]
    fn test_extract_linear_issues_deduplication() {
        let text = "https://linear.app/myteam/issue/ABC-123 and ABC-123 mentioned again";
        let refs = extract_linear_issues(text, Some("myteam"));
        // Should only have one entry since same workspace:identifier
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].identifier, "ABC-123");
    }

    #[test]
    fn test_create_issue_input_default() {
        let input = CreateIssueInput::default();
        assert!(input.team_id.is_empty());
        assert!(input.title.is_empty());
        assert!(input.description.is_none());
        assert!(input.priority.is_none());
        assert!(input.assignee_id.is_none());
        assert!(input.label_ids.is_none());
        assert!(input.state_id.is_none());
    }

    #[test]
    fn test_create_issue_input_builder() {
        let input = CreateIssueInput {
            team_id: "team-123".to_string(),
            title: "Test Issue".to_string(),
            description: Some("Test description".to_string()),
            priority: Some(2),
            assignee_id: Some("user-456".to_string()),
            label_ids: Some(vec!["label-1".to_string()]),
            state_id: Some("state-789".to_string()),
        };
        assert_eq!(input.team_id, "team-123");
        assert_eq!(input.title, "Test Issue");
        assert_eq!(input.description, Some("Test description".to_string()));
        assert_eq!(input.priority, Some(2));
    }

    #[test]
    fn test_linear_ref_equality() {
        let ref1 = LinearRef {
            workspace: "team".to_string(),
            identifier: "ABC-123".to_string(),
        };
        let ref2 = LinearRef {
            workspace: "team".to_string(),
            identifier: "ABC-123".to_string(),
        };
        let ref3 = LinearRef {
            workspace: "team".to_string(),
            identifier: "ABC-456".to_string(),
        };
        assert_eq!(ref1, ref2);
        assert_ne!(ref1, ref3);
    }
}
