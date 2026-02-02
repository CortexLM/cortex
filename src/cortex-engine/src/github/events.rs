//! GitHub event parsing.
//!
//! Parses GitHub webhook event payloads into typed structures.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// GitHub event types.
#[derive(Debug, Clone)]
pub enum GitHubEvent {
    /// Issue comment event.
    IssueComment(IssueCommentEvent),
    /// Pull request event.
    PullRequest(PullRequestEvent),
    /// Pull request review event.
    PullRequestReview(PullRequestReviewEvent),
    /// Issue event.
    Issues(IssueEvent),
    /// Unknown event type.
    Unknown(String),
}

/// Issue comment event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueCommentEvent {
    /// Action (created, edited, deleted).
    pub action: String,
    /// Issue number.
    pub issue_number: u64,
    /// Comment ID.
    pub comment_id: u64,
    /// Comment body.
    pub body: String,
    /// Comment author.
    pub author: String,
    /// Whether the issue is a pull request.
    pub is_pull_request: bool,
    /// Issue/PR title.
    pub issue_title: String,
}

/// Pull request event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestEvent {
    /// Action (opened, closed, synchronize, etc.).
    pub action: String,
    /// PR number.
    pub number: u64,
    /// PR title.
    pub title: String,
    /// PR body.
    pub body: Option<String>,
    /// PR author.
    pub author: String,
    /// Head branch.
    pub head_branch: String,
    /// Base branch.
    pub base_branch: String,
    /// Head SHA.
    pub head_sha: String,
    /// Whether PR is a draft.
    pub draft: bool,
    /// PR labels.
    pub labels: Vec<String>,
}

/// Pull request review event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequestReviewEvent {
    /// Action (submitted, edited, dismissed).
    pub action: String,
    /// PR number.
    pub pr_number: u64,
    /// Review ID.
    pub review_id: u64,
    /// Reviewer username.
    pub reviewer: String,
    /// Review state (approved, changes_requested, commented).
    pub state: String,
    /// Review body.
    pub body: Option<String>,
}

/// Issue event payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueEvent {
    /// Action (opened, closed, reopened, etc.).
    pub action: String,
    /// Issue number.
    pub number: u64,
    /// Issue title.
    pub title: String,
    /// Issue body.
    pub body: String,
    /// Issue author.
    pub author: String,
    /// Issue labels.
    pub labels: Vec<String>,
}

/// Parse a GitHub event from JSON payload.
pub fn parse_event(event_type: &str, payload: &str) -> Result<GitHubEvent> {
    let value: serde_json::Value =
        serde_json::from_str(payload).context("Failed to parse event payload as JSON")?;

    match event_type {
        "issue_comment" => parse_issue_comment(&value),
        "pull_request" => parse_pull_request(&value),
        "pull_request_review" => parse_pull_request_review(&value),
        "issues" => parse_issues(&value),
        _ => Ok(GitHubEvent::Unknown(event_type.to_string())),
    }
}

fn parse_issue_comment(value: &serde_json::Value) -> Result<GitHubEvent> {
    let action = value["action"].as_str().unwrap_or("unknown").to_string();

    let issue = &value["issue"];
    let comment = &value["comment"];

    let event = IssueCommentEvent {
        action,
        issue_number: issue["number"].as_u64().unwrap_or(0),
        comment_id: comment["id"].as_u64().unwrap_or(0),
        body: comment["body"].as_str().unwrap_or("").to_string(),
        author: comment["user"]["login"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        is_pull_request: issue.get("pull_request").is_some(),
        issue_title: issue["title"].as_str().unwrap_or("").to_string(),
    };

    Ok(GitHubEvent::IssueComment(event))
}

fn parse_pull_request(value: &serde_json::Value) -> Result<GitHubEvent> {
    let action = value["action"].as_str().unwrap_or("unknown").to_string();

    let pr = &value["pull_request"];

    let labels: Vec<String> = pr["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let event = PullRequestEvent {
        action,
        number: pr["number"].as_u64().unwrap_or(0),
        title: pr["title"].as_str().unwrap_or("").to_string(),
        body: pr["body"].as_str().map(String::from),
        author: pr["user"]["login"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        head_branch: pr["head"]["ref"].as_str().unwrap_or("").to_string(),
        base_branch: pr["base"]["ref"].as_str().unwrap_or("").to_string(),
        head_sha: pr["head"]["sha"].as_str().unwrap_or("").to_string(),
        draft: pr["draft"].as_bool().unwrap_or(false),
        labels,
    };

    Ok(GitHubEvent::PullRequest(event))
}

fn parse_pull_request_review(value: &serde_json::Value) -> Result<GitHubEvent> {
    let action = value["action"].as_str().unwrap_or("unknown").to_string();

    let review = &value["review"];
    let pr = &value["pull_request"];

    let event = PullRequestReviewEvent {
        action,
        pr_number: pr["number"].as_u64().unwrap_or(0),
        review_id: review["id"].as_u64().unwrap_or(0),
        reviewer: review["user"]["login"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        state: review["state"].as_str().unwrap_or("").to_string(),
        body: review["body"].as_str().map(String::from),
    };

    Ok(GitHubEvent::PullRequestReview(event))
}

fn parse_issues(value: &serde_json::Value) -> Result<GitHubEvent> {
    let action = value["action"].as_str().unwrap_or("unknown").to_string();

    let issue = &value["issue"];

    let labels: Vec<String> = issue["labels"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|l| l["name"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let event = IssueEvent {
        action,
        number: issue["number"].as_u64().unwrap_or(0),
        title: issue["title"].as_str().unwrap_or("").to_string(),
        body: issue["body"].as_str().unwrap_or("").to_string(),
        author: issue["user"]["login"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        labels,
    };

    Ok(GitHubEvent::Issues(event))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_issue_comment() {
        let payload = r#"{
            "action": "created",
            "issue": {
                "number": 42,
                "title": "Test issue"
            },
            "comment": {
                "id": 123,
                "body": "/cortex help",
                "user": {
                    "login": "testuser"
                }
            }
        }"#;

        let event = parse_event("issue_comment", payload).unwrap();
        match event {
            GitHubEvent::IssueComment(e) => {
                assert_eq!(e.action, "created");
                assert_eq!(e.issue_number, 42);
                assert_eq!(e.comment_id, 123);
                assert_eq!(e.body, "/cortex help");
                assert_eq!(e.author, "testuser");
            }
            _ => panic!("Expected IssueComment event"),
        }
    }

    #[test]
    fn test_parse_pull_request() {
        let payload = r#"{
            "action": "opened",
            "pull_request": {
                "number": 99,
                "title": "Add new feature",
                "body": "This PR adds...",
                "user": {
                    "login": "developer"
                },
                "head": {
                    "ref": "feature-branch",
                    "sha": "abc123"
                },
                "base": {
                    "ref": "main"
                },
                "draft": false,
                "labels": [
                    {"name": "enhancement"}
                ]
            }
        }"#;

        let event = parse_event("pull_request", payload).unwrap();
        match event {
            GitHubEvent::PullRequest(e) => {
                assert_eq!(e.action, "opened");
                assert_eq!(e.number, 99);
                assert_eq!(e.title, "Add new feature");
                assert_eq!(e.author, "developer");
                assert_eq!(e.head_branch, "feature-branch");
                assert_eq!(e.base_branch, "main");
                assert_eq!(e.labels, vec!["enhancement"]);
            }
            _ => panic!("Expected PullRequest event"),
        }
    }

    #[test]
    fn test_parse_unknown_event() {
        let payload = r#"{"action": "something"}"#;
        let event = parse_event("unknown_event", payload).unwrap();
        match event {
            GitHubEvent::Unknown(t) => assert_eq!(t, "unknown_event"),
            _ => panic!("Expected Unknown event"),
        }
    }
}
