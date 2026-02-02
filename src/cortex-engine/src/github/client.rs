//! GitHub API client.
//!
//! Provides a client for interacting with the GitHub API.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::api_client::create_default_client;

/// GitHub API client.
pub struct GitHubClient {
    client: reqwest::Client,
    token: Option<String>,
    owner: String,
    repo: String,
    base_url: String,
}

impl GitHubClient {
    /// Create a new authenticated GitHub client.
    pub fn new(token: &str, repository: &str) -> Result<Self> {
        let (owner, repo) = parse_repository(repository)?;

        let client = create_default_client().context("Failed to create HTTP client")?;

        // Check for GitHub Enterprise Server URL from environment
        let base_url = Self::detect_api_base_url();

        Ok(Self {
            client,
            token: Some(token.to_string()),
            owner,
            repo,
            base_url,
        })
    }

    /// Create an anonymous GitHub client (limited API access).
    pub fn anonymous(repository: &str) -> Result<Self> {
        let (owner, repo) = parse_repository(repository)?;

        let client = create_default_client().context("Failed to create HTTP client")?;

        // Check for GitHub Enterprise Server URL from environment
        let base_url = Self::detect_api_base_url();

        Ok(Self {
            client,
            token: None,
            owner,
            repo,
            base_url,
        })
    }

    /// Create a client for a specific GitHub Enterprise Server instance.
    pub fn with_enterprise_url(
        token: &str,
        repository: &str,
        enterprise_url: &str,
    ) -> Result<Self> {
        let (owner, repo) = parse_repository(repository)?;

        let client = create_default_client().context("Failed to create HTTP client")?;

        // Construct the API URL for GitHub Enterprise Server
        let base_url = Self::construct_ghe_api_url(enterprise_url);

        Ok(Self {
            client,
            token: Some(token.to_string()),
            owner,
            repo,
            base_url,
        })
    }

    /// Detect the API base URL from environment variables or git remote.
    /// Supports GitHub Enterprise Server via:
    /// - GITHUB_ENTERPRISE_URL environment variable
    /// - GH_HOST environment variable (gh CLI compatible)
    /// - GITHUB_API_URL environment variable (direct API URL)
    fn detect_api_base_url() -> String {
        // First check for direct API URL override
        if let Ok(api_url) = std::env::var("GITHUB_API_URL") {
            if !api_url.is_empty() {
                return api_url.trim_end_matches('/').to_string();
            }
        }

        // Check for GitHub Enterprise Server URL
        if let Ok(ghe_url) = std::env::var("GITHUB_ENTERPRISE_URL") {
            if !ghe_url.is_empty() {
                return Self::construct_ghe_api_url(&ghe_url);
            }
        }

        // Check for GH_HOST (gh CLI compatible)
        if let Ok(gh_host) = std::env::var("GH_HOST") {
            if !gh_host.is_empty() && gh_host != "github.com" {
                return format!("https://{}/api/v3", gh_host.trim_end_matches('/'));
            }
        }

        // Default to github.com
        "https://api.github.com".to_string()
    }

    /// Construct the API URL for a GitHub Enterprise Server instance.
    fn construct_ghe_api_url(enterprise_url: &str) -> String {
        let url = enterprise_url.trim_end_matches('/');
        // GitHub Enterprise Server API is at /api/v3
        if url.ends_with("/api/v3") {
            url.to_string()
        } else {
            format!("{}/api/v3", url)
        }
    }

    /// Get the base URL being used.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get pull request information.
    /// Always fetches fresh data from GitHub API to handle force-pushed PRs correctly.
    pub async fn get_pull_request(&self, number: u64) -> Result<PullRequestInfo> {
        let url = format!(
            "{}/repos/{}/{}/pulls/{}",
            self.base_url, self.owner, self.repo, number
        );

        let mut request = self.client.get(&url);

        if let Some(ref token) = self.token {
            request = request.bearer_auth(token);
        }

        // Bypass any caches to always get fresh PR data (important for force-pushed PRs)
        let response = request
            .header("Cache-Control", "no-cache, no-store, must-revalidate")
            .header("Pragma", "no-cache")
            .send()
            .await
            .context("Failed to fetch pull request")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let pr: GitHubPullRequest = response
            .json()
            .await
            .context("Failed to parse pull request response")?;

        Ok(PullRequestInfo {
            number: pr.number,
            title: pr.title,
            author: pr.user.login,
            state: pr.state,
            body: pr.body,
            head_branch: pr.head.ref_name,
            base_branch: pr.base.ref_name,
            head_sha: pr.head.sha,
            mergeable: pr.mergeable,
            draft: pr.draft.unwrap_or(false),
            labels: pr.labels.into_iter().map(|l| l.name).collect(),
        })
    }

    /// Create a comment on an issue or pull request.
    pub async fn create_comment(&self, issue_number: u64, body: &str) -> Result<u64> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Authentication required to create comments"))?;

        let url = format!(
            "{}/repos/{}/{}/issues/{}/comments",
            self.base_url, self.owner, self.repo, issue_number
        );

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({ "body": body }))
            .send()
            .await
            .context("Failed to create comment")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let comment: GitHubComment = response
            .json()
            .await
            .context("Failed to parse comment response")?;

        Ok(comment.id)
    }

    /// Add a reaction to a comment.
    pub async fn add_reaction(&self, comment_id: u64, reaction: &str) -> Result<()> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Authentication required to add reactions"))?;

        let url = format!(
            "{}/repos/{}/{}/issues/comments/{}/reactions",
            self.base_url, self.owner, self.repo, comment_id
        );

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .header("Accept", "application/vnd.github+json")
            .json(&serde_json::json!({ "content": reaction }))
            .send()
            .await
            .context("Failed to add reaction")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        Ok(())
    }

    /// Get issue information.
    pub async fn get_issue(&self, number: u64) -> Result<IssueInfo> {
        let url = format!(
            "{}/repos/{}/{}/issues/{}",
            self.base_url, self.owner, self.repo, number
        );

        let mut request = self.client.get(&url);

        if let Some(ref token) = self.token {
            request = request.bearer_auth(token);
        }

        let response = request.send().await.context("Failed to fetch issue")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let issue: GitHubIssue = response
            .json()
            .await
            .context("Failed to parse issue response")?;

        Ok(IssueInfo {
            number: issue.number,
            title: issue.title,
            author: issue.user.login,
            state: issue.state,
            body: issue.body,
            labels: issue.labels.into_iter().map(|l| l.name).collect(),
            is_pull_request: issue.pull_request.is_some(),
        })
    }

    /// List files changed in a pull request.
    pub async fn list_pull_request_files(&self, number: u64) -> Result<Vec<PullRequestFile>> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Authentication required to list PR files"))?;

        let url = format!(
            "{}/repos/{}/{}/pulls/{}/files",
            self.base_url, self.owner, self.repo, number
        );

        let response = self
            .client
            .get(&url)
            .bearer_auth(token)
            .send()
            .await
            .context("Failed to list PR files")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let files: Vec<GitHubPullRequestFile> = response
            .json()
            .await
            .context("Failed to parse PR files response")?;

        Ok(files
            .into_iter()
            .map(|f| PullRequestFile {
                filename: f.filename,
                status: f.status,
                additions: f.additions,
                deletions: f.deletions,
                changes: f.changes,
                patch: f.patch,
            })
            .collect())
    }

    /// Create a review comment on a pull request.
    pub async fn create_review_comment(
        &self,
        pr_number: u64,
        body: &str,
        commit_sha: &str,
        path: &str,
        line: u32,
    ) -> Result<u64> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Authentication required to create review comments"))?;

        let url = format!(
            "{}/repos/{}/{}/pulls/{}/comments",
            self.base_url, self.owner, self.repo, pr_number
        );

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({
                "body": body,
                "commit_id": commit_sha,
                "path": path,
                "line": line,
                "side": "RIGHT"
            }))
            .send()
            .await
            .context("Failed to create review comment")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let comment: GitHubComment = response
            .json()
            .await
            .context("Failed to parse comment response")?;

        Ok(comment.id)
    }

    /// Submit a pull request review.
    pub async fn submit_review(
        &self,
        pr_number: u64,
        body: &str,
        event: ReviewEvent,
    ) -> Result<u64> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Authentication required to submit reviews"))?;

        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews",
            self.base_url, self.owner, self.repo, pr_number
        );

        let event_str = match event {
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Comment => "COMMENT",
        };

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({
                "body": body,
                "event": event_str
            }))
            .send()
            .await
            .context("Failed to submit review")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let review: GitHubReview = response
            .json()
            .await
            .context("Failed to parse review response")?;

        Ok(review.id)
    }

    /// Submit a batched pull request review with multiple comments.
    ///
    /// This method creates a single review with multiple comments attached,
    /// which is more efficient than posting comments individually and
    /// results in a single notification to the PR author.
    ///
    /// # Arguments
    /// * `pr_number` - The pull request number
    /// * `body` - The overall review comment body
    /// * `event` - The review event (Approve, RequestChanges, Comment)
    /// * `comments` - List of inline comments to attach to the review
    ///
    /// # Returns
    /// The ID of the created review
    pub async fn submit_batched_review(
        &self,
        pr_number: u64,
        body: &str,
        event: ReviewEvent,
        comments: Vec<ReviewComment>,
    ) -> Result<u64> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Authentication required to submit reviews"))?;

        let url = format!(
            "{}/repos/{}/{}/pulls/{}/reviews",
            self.base_url, self.owner, self.repo, pr_number
        );

        let event_str = match event {
            ReviewEvent::Approve => "APPROVE",
            ReviewEvent::RequestChanges => "REQUEST_CHANGES",
            ReviewEvent::Comment => "COMMENT",
        };

        // Convert comments to GitHub API format
        let api_comments: Vec<serde_json::Value> = comments
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "path": c.path,
                    "line": c.line,
                    "body": c.body,
                    "side": c.side.unwrap_or_else(|| "RIGHT".to_string())
                })
            })
            .collect();

        let response = self
            .client
            .post(&url)
            .bearer_auth(token)
            .json(&serde_json::json!({
                "body": body,
                "event": event_str,
                "comments": api_comments
            }))
            .send()
            .await
            .context("Failed to submit batched review")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("GitHub API error ({}): {}", status, body);
        }

        let review: GitHubReview = response
            .json()
            .await
            .context("Failed to parse review response")?;

        Ok(review.id)
    }
}

/// A comment to be included in a batched review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    /// The relative path of the file to comment on.
    pub path: String,
    /// The line number in the diff to comment on.
    pub line: u32,
    /// The comment body.
    pub body: String,
    /// The side of the diff to comment on (LEFT or RIGHT). Defaults to RIGHT.
    pub side: Option<String>,
}

/// Parse repository string (owner/repo) into components.
fn parse_repository(repository: &str) -> Result<(String, String)> {
    let parts: Vec<&str> = repository.split('/').collect();
    if parts.len() != 2 {
        bail!(
            "Invalid repository format. Expected 'owner/repo', got '{}'",
            repository
        );
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Pull request information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestInfo {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: String,
    pub body: Option<String>,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub mergeable: Option<bool>,
    pub draft: bool,
    pub labels: Vec<String>,
}

/// Issue information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueInfo {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub state: String,
    pub body: Option<String>,
    pub labels: Vec<String>,
    pub is_pull_request: bool,
}

/// File changed in a pull request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestFile {
    pub filename: String,
    pub status: String,
    pub additions: u32,
    pub deletions: u32,
    pub changes: u32,
    pub patch: Option<String>,
}

/// Review event type.
#[derive(Debug, Clone, Copy)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

// GitHub API response types

#[derive(Debug, Deserialize)]
struct GitHubPullRequest {
    number: u64,
    title: String,
    state: String,
    body: Option<String>,
    user: GitHubUser,
    head: GitHubRef,
    base: GitHubRef,
    mergeable: Option<bool>,
    draft: Option<bool>,
    labels: Vec<GitHubLabel>,
}

#[derive(Debug, Deserialize)]
struct GitHubIssue {
    number: u64,
    title: String,
    state: String,
    body: Option<String>,
    user: GitHubUser,
    labels: Vec<GitHubLabel>,
    pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct GitHubUser {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GitHubRef {
    #[serde(rename = "ref")]
    ref_name: String,
    sha: String,
}

#[derive(Debug, Deserialize)]
struct GitHubLabel {
    name: String,
}

#[derive(Debug, Deserialize)]
struct GitHubComment {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct GitHubReview {
    id: u64,
}

#[derive(Debug, Deserialize)]
struct GitHubPullRequestFile {
    filename: String,
    status: String,
    additions: u32,
    deletions: u32,
    changes: u32,
    patch: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_repository() {
        let (owner, repo) = parse_repository("cortex-ai/cortex").unwrap();
        assert_eq!(owner, "cortex-ai");
        assert_eq!(repo, "cortex");
    }

    #[test]
    fn test_parse_repository_invalid() {
        assert!(parse_repository("invalid").is_err());
        assert!(parse_repository("too/many/parts").is_err());
    }
}
