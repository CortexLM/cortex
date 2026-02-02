//! Cortex Protocol - Communication types between client and agent
//!
//! This crate defines the SQ/EQ (Submission Queue / Event Queue) protocol
//! used for asynchronous communication between the user interface and
//! the AI agent.

pub mod approvals;
pub mod config_types;
pub mod conversation_id;
pub mod items;
pub mod models;
pub mod num_format;
pub mod protocol;
pub mod user_input;

#[cfg(test)]
mod tests;

// Re-exports
pub use approvals::*;
pub use config_types::*;
pub use conversation_id::ConversationId;
pub use models::*;
pub use protocol::*;
pub use user_input::{
    MAX_FILE_CONTENT_SIZE, MAX_IMAGE_DATA_SIZE, MAX_PATH_LENGTH, MAX_TEXT_SIZE, MAX_URL_LENGTH,
    UserInput, UserInputValidationError,
};
