//! Error types for Cortex TUI operations.

use thiserror::Error;

/// Core error type for Cortex TUI operations.
#[derive(Error, Debug)]
pub enum Error {
    /// An I/O error occurred.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Terminal operation failed.
    #[error("Terminal error: {0}")]
    Terminal(String),

    /// Invalid dimensions were provided.
    #[error("Invalid dimensions: {0}")]
    InvalidDimensions(String),

    /// Index out of bounds.
    #[error("Index out of bounds: {index} >= {size}")]
    OutOfBounds {
        /// The attempted index
        index: usize,
        /// The actual size
        size: usize,
    },

    /// Feature not supported by the terminal.
    #[error("Unsupported feature: {0}")]
    Unsupported(String),
}

/// Result type alias using the core Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Error type for color parsing operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum ColorParseError {
    /// Input string was empty.
    #[error("empty input")]
    EmptyInput,

    /// Hex string had an invalid length.
    #[error("invalid hex length: {0} (expected 3, 4, 6, or 8)")]
    InvalidLength(usize),

    /// Invalid hexadecimal character.
    #[error("invalid hex character")]
    InvalidHexChar,

    /// Unknown color name.
    #[error("unknown color name: {0}")]
    UnknownColor(String),
}

/// Error type for geometry operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum GeometryError {
    /// Coordinates are out of bounds.
    #[error("coordinates out of bounds: ({x}, {y})")]
    OutOfBounds {
        /// The X coordinate.
        x: i32,
        /// The Y coordinate.
        y: i32,
    },

    /// Invalid rectangle dimensions.
    #[error("invalid rectangle dimensions: width={width}, height={height}")]
    InvalidDimensions {
        /// The width.
        width: u16,
        /// The height.
        height: u16,
    },

    /// Rectangles do not intersect.
    #[error("rectangles do not intersect")]
    NoIntersection,
}

/// Result type alias for geometry operations.
pub type GeometryResult<T> = std::result::Result<T, GeometryError>;

/// Error type for style operations.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum StyleError {
    /// Invalid text attribute combination.
    #[error("invalid attribute combination: {0}")]
    InvalidAttributes(String),

    /// Style parsing failed.
    #[error("failed to parse style: {0}")]
    ParseError(String),
}

/// Result type alias for style operations.
pub type StyleResult<T> = std::result::Result<T, StyleError>;
