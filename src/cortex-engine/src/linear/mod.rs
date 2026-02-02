//! Linear integration module.
//!
//! This module provides Linear API client and OAuth authentication
//! for integrating with Linear issue tracking.
//!
//! Features:
//! - Bi-directional synchronization (read/write issues)
//! - URL and short reference parsing
//! - Rate limiting with automatic retry
//! - Context enrichment from text

pub mod client;

pub use client::{
    Comment, CommentsConnection, CreateIssueInput, Issue, IssueDetails, IssueState,
    LINEAR_OAUTH_AUTHORIZE, LINEAR_OAUTH_TOKEN, Label, LabelsConnection, LinearClient, LinearRef,
    Team, TeamInfo, User, extract_linear_issues,
};
