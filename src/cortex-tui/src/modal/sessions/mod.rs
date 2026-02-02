//! Sessions Modal
//!
//! A modal for managing sessions - listing, resuming, creating new, and deleting.

mod modal_impl;
mod rendering;
mod session_action;
mod session_info;

#[cfg(test)]
mod tests;

// Re-export public items to maintain backwards compatibility
pub use modal_impl::SessionsModal;
pub use session_info::SessionInfo;
