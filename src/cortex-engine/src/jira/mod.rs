//! Jira integration module.
//!
//! This module provides Jira REST API client for integrating with
//! Jira issue tracking.
//!
//! Features:
//! - Bi-directional synchronization (read/write issues)
//! - URL parsing for ticket detection
//! - Rate limiting with automatic retry
//! - Context enrichment from text
//! - Support for both Cloud and Server/Data Center deployments

pub mod client;

pub use client::{
    CreateIssueInput as JiraCreateIssueInput, JiraClient, JiraComment, JiraIssue, JiraIssueDetails,
    JiraIssueType, JiraPriority, JiraProject, JiraRef, JiraStatus, JiraTransition, JiraUser,
    extract_jira_issues,
};
