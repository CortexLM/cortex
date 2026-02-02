//! GitHub integration module.
//!
//! This module provides GitHub API client, event parsing, and workflow generation
//! for CI/CD automation with Cortex.

pub mod client;
pub mod events;
pub mod workflow;

// Re-exports
pub use client::{GitHubClient, PullRequestInfo};
pub use events::{
    GitHubEvent, IssueCommentEvent, IssueEvent, PullRequestEvent, PullRequestReviewEvent,
    parse_event,
};
pub use workflow::{WorkflowConfig, generate_workflow};
