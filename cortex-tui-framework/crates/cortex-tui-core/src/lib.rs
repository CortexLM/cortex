//! Core types and traits for `Cortex TUI`.
//!
//! This crate provides the fundamental building blocks for the `Cortex TUI` terminal UI framework:
//!
//! - [`color`]: RGBA color representation with alpha blending and ANSI conversion
//! - [`style`]: Text styling with colors and attributes (bold, italic, etc.)
//! - [`geometry`]: 2D geometry primitives (Point, Size, Rect)
//! - [`error`]: Error types for the core library
//!
//! # Examples
//!
//! ## Working with Colors
//!
//! ```
//! use cortex_tui_core::color::Color;
//!
//! // Create colors from different sources
//! let red = Color::RED;
//! let hex_color = Color::from_hex("#FF8000").unwrap();
//! let rgb_color = Color::from_rgb_u8(64, 128, 255);
//!
//! // Alpha blending
//! let overlay = Color::new(1.0, 0.0, 0.0, 0.5); // 50% transparent red
//! let background = Color::WHITE;
//! let blended = overlay.blend_over(background);
//!
//! // Generate ANSI escape sequences
//! let fg_escape = red.to_ansi_fg();
//! ```
//!
//! ## Working with Styles
//!
//! ```
//! use cortex_tui_core::style::{Style, TextAttributes};
//! use cortex_tui_core::color::Color;
//!
//! // Create styled text
//! let error_style = Style::new()
//!     .fg(Color::RED)
//!     .bold();
//!
//! let warning_style = Style::new()
//!     .fg(Color::YELLOW)
//!     .bg(Color::BLACK);
//!
//! // Combine styles
//! let combined = error_style.merge(&Style::new().underline());
//! ```
//!
//! ## Working with Geometry
//!
//! ```
//! use cortex_tui_core::geometry::{Point, Size, Rect};
//!
//! // Create geometric primitives
//! let origin = Point::new(0, 0);
//! let size = Size::new(80, 24);
//! let rect = Rect::new(10, 5, 60, 18);
//!
//! // Test containment
//! let point = Point::new(30, 10);
//! assert!(rect.contains_point(point));
//!
//! // Calculate intersections
//! let other = Rect::new(50, 10, 40, 20);
//! if let Some(intersection) = rect.intersection(other) {
//!     println!("Intersection: {:?}", intersection);
//! }
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::return_self_not_must_use)]
#![allow(clippy::cast_possible_truncation)]
#![allow(clippy::cast_sign_loss)]
#![allow(clippy::cast_lossless)]
#![allow(clippy::float_cmp)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::uninlined_format_args)]
#![allow(clippy::many_single_char_names)]
#![allow(clippy::doc_markdown)]
#![allow(clippy::format_collect)]
#![allow(clippy::match_same_arms)]

pub mod color;
pub mod error;
pub mod geometry;
pub mod style;

// Re-export commonly used types at the crate root for convenience
pub use color::Color;
pub use error::{ColorParseError, Error, Result};
pub use geometry::{Point, Rect, Size};
pub use style::{Style, TextAttributes};
