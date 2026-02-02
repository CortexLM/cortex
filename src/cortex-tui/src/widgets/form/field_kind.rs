//! Field kind definitions for form fields.

/// Type of a form field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    /// Plain text input.
    Text,
    /// Masked text input (for passwords).
    Secret,
    /// Numeric input.
    Number,
    /// Boolean toggle.
    Toggle,
    /// Selection from a list of options.
    Select(Vec<String>),
}
