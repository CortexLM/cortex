//! Conversation management.

mod history;
mod manager;

pub use history::{ConversationHistory, HistoryEntry};
pub use manager::ConversationManager;
