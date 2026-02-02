#![allow(clippy::manual_strip)]
//! Slack integration for Cortex CLI.
//!
//! This crate provides comprehensive Slack integration including:
//! - Slack Bot with Socket Mode for real-time events
//! - Event handlers for @mentions and direct messages
//! - Slash commands (/cortex, /cortex-review)
//! - OAuth flow for workspace installation
//! - Message formatting (Markdown to mrkdwn)
//!
//! # Architecture
//!
//! The integration is built around the `CortexSlackBot` struct which manages:
//! - WebSocket connection to Slack via Socket Mode
//! - Event routing and handling
//! - Message sending and formatting
//!
//! # Example
//!
//! ```rust,ignore
//! use cortex_slack::{CortexSlackBot, SlackConfig};
//!
//! let config = SlackConfig::from_env()?;
//! let bot = CortexSlackBot::new(config).await?;
//! bot.start().await?;
//! ```
//!
//! # Configuration
//!
//! Required environment variables:
//! - `SLACK_BOT_TOKEN` - Bot OAuth token (xoxb-...)
//! - `SLACK_APP_TOKEN` - App-level token for Socket Mode (xapp-...)
//! - `SLACK_SIGNING_SECRET` - Signing secret for request verification
//!
//! Optional:
//! - `SLACK_CLIENT_ID` - For OAuth flow
//! - `SLACK_CLIENT_SECRET` - For OAuth flow

pub mod bot;
pub mod commands;
pub mod config;
pub mod error;
pub mod events;
pub mod messages;
pub mod oauth;

// Re-export main types
pub use bot::CortexSlackBot;
pub use config::SlackConfig;
pub use error::{SlackError, SlackResult};
pub use events::{SlackEvent, SlackEventHandler};
pub use messages::{SlackMessageBuilder, markdown_to_mrkdwn};
