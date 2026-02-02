//! Generic Form Widget
//!
//! A reusable form component that can render various field types
//! (Text, Secret, Number, Toggle, Select) and handle user input.

pub mod colors;
pub mod field;
pub mod field_kind;
pub mod modal;
pub mod state;
mod tests;
mod utils;

// Re-export all public types for backwards compatibility
pub use colors::FormModalColors;
pub use field::FormField;
pub use field_kind::FieldKind;
pub use modal::FormModal;
pub use state::FormState;
