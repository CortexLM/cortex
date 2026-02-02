//! RGBA color representation and manipulation.
//!
//! This module provides the [`Color`] type for representing colors with full
//! alpha channel support, along with conversion utilities for various color
//! formats used in terminal applications.
//!
//! # Color Representation
//!
//! Colors are represented using normalized f32 components in the range 0.0 to 1.0,
//! which provides:
//! - Compatibility with GPU rendering conventions
//! - Efficient alpha blending calculations
//! - High precision for color manipulation
//!
//! # Supported Formats
//!
//! - Hex strings: `#RGB`, `#RGBA`, `#RRGGBB`, `#RRGGBBAA`
//! - RGB u8 values: `(0-255, 0-255, 0-255)`
//! - ANSI 256 color palette indices
//! - Named CSS/ANSI colors
//!
//! # Examples
//!
//! ```
//! use cortex_tui_core::color::Color;
//!
//! // From hex string
//! let red = Color::from_hex("#FF0000").unwrap();
//!
//! // From RGB u8 values
//! let green = Color::from_rgb_u8(0, 255, 0);
//!
//! // Using predefined constants
//! let blue = Color::BLUE;
//!
//! // Alpha blending
//! let overlay = Color::new(1.0, 0.0, 0.0, 0.5); // 50% transparent red
//! let background = Color::WHITE;
//! let blended = overlay.blend_over(background);
//! ```

use crate::error::ColorParseError;
use std::fmt;

/// An RGBA color with normalized f32 components in the range 0.0 to 1.0.
///
/// The color components are:
/// - `r`: Red (0.0 = no red, 1.0 = full red)
/// - `g`: Green (0.0 = no green, 1.0 = full green)
/// - `b`: Blue (0.0 = no blue, 1.0 = full blue)
/// - `a`: Alpha (0.0 = fully transparent, 1.0 = fully opaque)
#[derive(Clone, Copy, PartialEq)]
pub struct Color {
    /// Red component (0.0 - 1.0).
    pub r: f32,
    /// Green component (0.0 - 1.0).
    pub g: f32,
    /// Blue component (0.0 - 1.0).
    pub b: f32,
    /// Alpha component (0.0 = transparent, 1.0 = opaque).
    pub a: f32,
}

impl Default for Color {
    fn default() -> Self {
        Self::TRANSPARENT
    }
}

impl fmt::Debug for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.a == 1.0 {
            write!(f, "Color::rgb({:.3}, {:.3}, {:.3})", self.r, self.g, self.b)
        } else {
            write!(
                f,
                "Color::rgba({:.3}, {:.3}, {:.3}, {:.3})",
                self.r, self.g, self.b, self.a
            )
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_hex())
    }
}

// ============================================================================
// Common color constants
// ============================================================================

impl Color {
    /// Fully transparent color (alpha = 0).
    pub const TRANSPARENT: Self = Self::rgba(0.0, 0.0, 0.0, 0.0);

    /// Opaque black (#000000).
    pub const BLACK: Self = Self::rgb(0.0, 0.0, 0.0);

    /// Opaque white (#FFFFFF).
    pub const WHITE: Self = Self::rgb(1.0, 1.0, 1.0);

    /// Opaque red (#FF0000).
    pub const RED: Self = Self::rgb(1.0, 0.0, 0.0);

    /// Opaque green (#00FF00).
    pub const GREEN: Self = Self::rgb(0.0, 1.0, 0.0);

    /// Opaque blue (#0000FF).
    pub const BLUE: Self = Self::rgb(0.0, 0.0, 1.0);

    /// Opaque yellow (#FFFF00).
    pub const YELLOW: Self = Self::rgb(1.0, 1.0, 0.0);

    /// Opaque cyan (#00FFFF).
    pub const CYAN: Self = Self::rgb(0.0, 1.0, 1.0);

    /// Opaque magenta (#FF00FF).
    pub const MAGENTA: Self = Self::rgb(1.0, 0.0, 1.0);

    /// Opaque gray (#808080).
    pub const GRAY: Self = Self::rgb(0.5, 0.5, 0.5);

    /// Dark gray (#404040).
    pub const DARK_GRAY: Self = Self::rgb(0.25, 0.25, 0.25);

    /// Light gray (#C0C0C0).
    pub const LIGHT_GRAY: Self = Self::rgb(0.75, 0.75, 0.75);

    /// Orange (#FFA500).
    pub const ORANGE: Self = Self::rgb(1.0, 0.647, 0.0);

    /// Purple (#800080).
    pub const PURPLE: Self = Self::rgb(0.5, 0.0, 0.5);

    /// Brown (#A52A2A).
    pub const BROWN: Self = Self::rgb(0.647, 0.165, 0.165);

    /// Pink (#FFC0CB).
    pub const PINK: Self = Self::rgb(1.0, 0.753, 0.796);

    /// Navy (#000080).
    pub const NAVY: Self = Self::rgb(0.0, 0.0, 0.5);

    /// Teal (#008080).
    pub const TEAL: Self = Self::rgb(0.0, 0.5, 0.5);

    /// Olive (#808000).
    pub const OLIVE: Self = Self::rgb(0.5, 0.5, 0.0);

    /// Maroon (#800000).
    pub const MAROON: Self = Self::rgb(0.5, 0.0, 0.0);
}

// ============================================================================
// ANSI standard colors (16-color palette)
// ============================================================================

impl Color {
    /// ANSI Black (color 0).
    pub const ANSI_BLACK: Self = Self::rgb(0.0, 0.0, 0.0);

    /// ANSI Red (color 1).
    pub const ANSI_RED: Self = Self::rgb(0.5, 0.0, 0.0);

    /// ANSI Green (color 2).
    pub const ANSI_GREEN: Self = Self::rgb(0.0, 0.5, 0.0);

    /// ANSI Yellow (color 3).
    pub const ANSI_YELLOW: Self = Self::rgb(0.5, 0.5, 0.0);

    /// ANSI Blue (color 4).
    pub const ANSI_BLUE: Self = Self::rgb(0.0, 0.0, 0.5);

    /// ANSI Magenta (color 5).
    pub const ANSI_MAGENTA: Self = Self::rgb(0.5, 0.0, 0.5);

    /// ANSI Cyan (color 6).
    pub const ANSI_CYAN: Self = Self::rgb(0.0, 0.5, 0.5);

    /// ANSI White (color 7).
    pub const ANSI_WHITE: Self = Self::rgb(0.75, 0.75, 0.75);

    /// ANSI Bright Black (color 8).
    pub const ANSI_BRIGHT_BLACK: Self = Self::rgb(0.5, 0.5, 0.5);

    /// ANSI Bright Red (color 9).
    pub const ANSI_BRIGHT_RED: Self = Self::rgb(1.0, 0.0, 0.0);

    /// ANSI Bright Green (color 10).
    pub const ANSI_BRIGHT_GREEN: Self = Self::rgb(0.0, 1.0, 0.0);

    /// ANSI Bright Yellow (color 11).
    pub const ANSI_BRIGHT_YELLOW: Self = Self::rgb(1.0, 1.0, 0.0);

    /// ANSI Bright Blue (color 12).
    pub const ANSI_BRIGHT_BLUE: Self = Self::rgb(0.0, 0.0, 1.0);

    /// ANSI Bright Magenta (color 13).
    pub const ANSI_BRIGHT_MAGENTA: Self = Self::rgb(1.0, 0.0, 1.0);

    /// ANSI Bright Cyan (color 14).
    pub const ANSI_BRIGHT_CYAN: Self = Self::rgb(0.0, 1.0, 1.0);

    /// ANSI Bright White (color 15).
    pub const ANSI_BRIGHT_WHITE: Self = Self::rgb(1.0, 1.0, 1.0);
}

// ============================================================================
// Constructors
// ============================================================================

impl Color {
    /// Creates a new color from normalized RGBA components.
    ///
    /// All components should be in the range 0.0 to 1.0.
    #[inline]
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// Creates a new color from normalized RGBA components (alias for `rgba`).
    #[inline]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self::rgba(r, g, b, a)
    }

    /// Creates a new opaque color from normalized RGB components.
    #[inline]
    pub const fn rgb(r: f32, g: f32, b: f32) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    /// Creates a new color from u8 RGBA components (0-255).
    #[inline]
    pub fn from_rgba_u8(r: u8, g: u8, b: u8, a: u8) -> Self {
        const INV_255: f32 = 1.0 / 255.0;
        Self {
            r: r as f32 * INV_255,
            g: g as f32 * INV_255,
            b: b as f32 * INV_255,
            a: a as f32 * INV_255,
        }
    }

    /// Creates a new opaque color from u8 RGB components (0-255).
    #[inline]
    pub fn from_rgb_u8(r: u8, g: u8, b: u8) -> Self {
        Self::from_rgba_u8(r, g, b, 255)
    }

    /// Creates a grayscale color with the given luminance.
    #[inline]
    pub const fn gray(luminance: f32) -> Self {
        Self::rgb(luminance, luminance, luminance)
    }

    /// Creates a grayscale color with the given luminance and alpha.
    #[inline]
    pub const fn gray_alpha(luminance: f32, alpha: f32) -> Self {
        Self::rgba(luminance, luminance, luminance, alpha)
    }
}

// ============================================================================
// Hex parsing and formatting
// ============================================================================

impl Color {
    /// Parses a color from a hex string.
    ///
    /// Supports the following formats:
    /// - `#RGB` (shorthand, expanded to `#RRGGBB`)
    /// - `#RGBA` (shorthand with alpha, expanded to `#RRGGBBAA`)
    /// - `#RRGGBB` (standard 6-digit hex)
    /// - `#RRGGBBAA` (8-digit hex with alpha)
    ///
    /// The `#` prefix is optional.
    ///
    /// # Examples
    ///
    /// ```
    /// use cortex_tui_core::color::Color;
    ///
    /// let red = Color::from_hex("#FF0000").unwrap();
    /// let green = Color::from_hex("00FF00").unwrap();
    /// let semi_blue = Color::from_hex("#0000FF80").unwrap();
    /// let short_white = Color::from_hex("#FFF").unwrap();
    /// ```
    pub fn from_hex(hex: &str) -> Result<Self, ColorParseError> {
        let hex = hex.strip_prefix('#').unwrap_or(hex);

        if hex.is_empty() {
            return Err(ColorParseError::EmptyInput);
        }

        // Expand shorthand notation
        let expanded: String = match hex.len() {
            3 => {
                // #RGB -> #RRGGBB
                let chars: Vec<char> = hex.chars().collect();
                format!(
                    "{}{}{}{}{}{}",
                    chars[0], chars[0], chars[1], chars[1], chars[2], chars[2]
                )
            }
            4 => {
                // #RGBA -> #RRGGBBAA
                let chars: Vec<char> = hex.chars().collect();
                format!(
                    "{}{}{}{}{}{}{}{}",
                    chars[0], chars[0], chars[1], chars[1], chars[2], chars[2], chars[3], chars[3]
                )
            }
            6 | 8 => hex.to_string(),
            len => return Err(ColorParseError::InvalidLength(len)),
        };

        let parse_component = |s: &str| -> Result<u8, ColorParseError> {
            u8::from_str_radix(s, 16).map_err(|_| ColorParseError::InvalidHexChar)
        };

        let r = parse_component(&expanded[0..2])?;
        let g = parse_component(&expanded[2..4])?;
        let b = parse_component(&expanded[4..6])?;
        let a = if expanded.len() == 8 {
            parse_component(&expanded[6..8])?
        } else {
            255
        };

        Ok(Self::from_rgba_u8(r, g, b, a))
    }

    /// Converts the color to a hex string.
    ///
    /// Returns `#RRGGBB` for opaque colors or `#RRGGBBAA` for colors with alpha.
    pub fn to_hex(&self) -> String {
        let (r, g, b, a) = self.to_rgba_u8();
        if a == 255 {
            format!("#{:02X}{:02X}{:02X}", r, g, b)
        } else {
            format!("#{:02X}{:02X}{:02X}{:02X}", r, g, b, a)
        }
    }
}

// ============================================================================
// Named color parsing
// ============================================================================

impl Color {
    /// Parses a color from a named color string or hex value.
    ///
    /// Supports CSS color names, ANSI color names, "transparent", and hex strings.
    ///
    /// # Examples
    ///
    /// ```
    /// use cortex_tui_core::color::Color;
    ///
    /// let red = Color::parse("red").unwrap();
    /// let transparent = Color::parse("transparent").unwrap();
    /// let hex_blue = Color::parse("#0000FF").unwrap();
    /// ```
    pub fn parse(input: &str) -> Result<Self, ColorParseError> {
        let lower = input.to_lowercase();

        // Check for special values
        if lower == "transparent" {
            return Ok(Self::TRANSPARENT);
        }

        // Check named colors
        if let Some(color) = Self::from_name(&lower) {
            return Ok(color);
        }

        // Try parsing as hex
        Self::from_hex(input)
    }

    /// Returns a color for the given CSS/ANSI color name, or `None` if not found.
    pub fn from_name(name: &str) -> Option<Self> {
        let lower = name.to_lowercase();
        match lower.as_str() {
            // Standard colors
            "black" => Some(Self::BLACK),
            "white" => Some(Self::WHITE),
            "red" => Some(Self::RED),
            "green" | "lime" => Some(Self::GREEN),
            "blue" => Some(Self::BLUE),
            "yellow" => Some(Self::YELLOW),
            "cyan" | "aqua" => Some(Self::CYAN),
            "magenta" | "fuchsia" => Some(Self::MAGENTA),
            "gray" | "grey" => Some(Self::GRAY),
            "darkgray" | "darkgrey" => Some(Self::DARK_GRAY),
            "lightgray" | "lightgrey" | "silver" => Some(Self::LIGHT_GRAY),
            "orange" => Some(Self::ORANGE),
            "purple" => Some(Self::PURPLE),
            "brown" => Some(Self::BROWN),
            "pink" => Some(Self::PINK),
            "navy" => Some(Self::NAVY),
            "teal" => Some(Self::TEAL),
            "olive" => Some(Self::OLIVE),
            "maroon" => Some(Self::MAROON),

            // ANSI bright colors
            "brightblack" => Some(Self::ANSI_BRIGHT_BLACK),
            "brightred" => Some(Self::ANSI_BRIGHT_RED),
            "brightgreen" => Some(Self::ANSI_BRIGHT_GREEN),
            "brightyellow" => Some(Self::ANSI_BRIGHT_YELLOW),
            "brightblue" => Some(Self::ANSI_BRIGHT_BLUE),
            "brightmagenta" => Some(Self::ANSI_BRIGHT_MAGENTA),
            "brightcyan" => Some(Self::ANSI_BRIGHT_CYAN),
            "brightwhite" => Some(Self::ANSI_BRIGHT_WHITE),

            _ => None,
        }
    }
}

// ============================================================================
// HSV conversion
// ============================================================================

impl Color {
    /// Creates a color from HSV (Hue, Saturation, Value) components.
    ///
    /// - `h`: Hue in degrees (0-360, wraps around)
    /// - `s`: Saturation (0.0-1.0)
    /// - `v`: Value/brightness (0.0-1.0)
    ///
    /// # Examples
    ///
    /// ```
    /// use cortex_tui_core::color::Color;
    ///
    /// let red = Color::from_hsv(0.0, 1.0, 1.0);
    /// let green = Color::from_hsv(120.0, 1.0, 1.0);
    /// let blue = Color::from_hsv(240.0, 1.0, 1.0);
    /// ```
    #[allow(clippy::many_single_char_names)]
    pub fn from_hsv(h: f32, s: f32, v: f32) -> Self {
        let h = h.rem_euclid(360.0);
        let s = s.clamp(0.0, 1.0);
        let v = v.clamp(0.0, 1.0);

        let sector = ((h / 60.0).floor() as u8) % 6;
        let f = h / 60.0 - (h / 60.0).floor();

        let p = v * (1.0 - s);
        let q = v * (1.0 - f * s);
        let t = v * (1.0 - (1.0 - f) * s);

        let (r, g, b) = match sector {
            0 => (v, t, p),
            1 => (q, v, p),
            2 => (p, v, t),
            3 => (p, q, v),
            4 => (t, p, v),
            5 => (v, p, q),
            _ => unreachable!(),
        };

        Self::rgb(r, g, b)
    }

    /// Creates a color from HSV with alpha.
    pub fn from_hsva(h: f32, s: f32, v: f32, a: f32) -> Self {
        Self::from_hsv(h, s, v).with_alpha(a)
    }

    /// Converts the color to HSV components.
    ///
    /// Returns `(h, s, v)` where:
    /// - `h`: Hue in degrees (0-360)
    /// - `s`: Saturation (0.0-1.0)
    /// - `v`: Value/brightness (0.0-1.0)
    pub fn to_hsv(&self) -> (f32, f32, f32) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let delta = max - min;

        let v = max;

        let s = if max == 0.0 { 0.0 } else { delta / max };

        let h = if delta == 0.0 {
            0.0
        } else if max == self.r {
            60.0 * (((self.g - self.b) / delta) % 6.0)
        } else if max == self.g {
            60.0 * (((self.b - self.r) / delta) + 2.0)
        } else {
            60.0 * (((self.r - self.g) / delta) + 4.0)
        };

        let h = if h < 0.0 { h + 360.0 } else { h };

        (h, s, v)
    }
}

// ============================================================================
// ANSI 256 color conversion
// ============================================================================

impl Color {
    /// Creates a color from an ANSI 256-color palette index.
    ///
    /// The 256-color palette is organized as follows:
    /// - 0-15: Standard ANSI colors (same as 16-color palette)
    /// - 16-231: 6×6×6 color cube
    /// - 232-255: Grayscale ramp
    pub fn from_ansi_256(index: u8) -> Self {
        match index {
            // Standard ANSI colors (0-15)
            0 => Self::ANSI_BLACK,
            1 => Self::ANSI_RED,
            2 => Self::ANSI_GREEN,
            3 => Self::ANSI_YELLOW,
            4 => Self::ANSI_BLUE,
            5 => Self::ANSI_MAGENTA,
            6 => Self::ANSI_CYAN,
            7 => Self::ANSI_WHITE,
            8 => Self::ANSI_BRIGHT_BLACK,
            9 => Self::ANSI_BRIGHT_RED,
            10 => Self::ANSI_BRIGHT_GREEN,
            11 => Self::ANSI_BRIGHT_YELLOW,
            12 => Self::ANSI_BRIGHT_BLUE,
            13 => Self::ANSI_BRIGHT_MAGENTA,
            14 => Self::ANSI_BRIGHT_CYAN,
            15 => Self::ANSI_BRIGHT_WHITE,

            // 6×6×6 color cube (16-231)
            16..=231 => {
                let idx = index - 16;
                let r = idx / 36;
                let g = (idx % 36) / 6;
                let b = idx % 6;

                // Cube values: 0, 95, 135, 175, 215, 255
                let to_value = |n: u8| -> u8 {
                    if n == 0 {
                        0
                    } else {
                        55 + n * 40
                    }
                };

                Self::from_rgb_u8(to_value(r), to_value(g), to_value(b))
            }

            // Grayscale ramp (232-255)
            232..=255 => {
                let gray = 8 + (index - 232) * 10;
                Self::from_rgb_u8(gray, gray, gray)
            }
        }
    }

    /// Converts the color to the nearest ANSI 256-color palette index.
    ///
    /// Uses the color cube for chromatic colors and the grayscale ramp
    /// for near-grayscale colors.
    pub fn to_ansi_256(&self) -> u8 {
        let (r, g, b, _) = self.to_rgba_u8();

        // Check if the color is approximately grayscale
        let avg = ((r as u16 + g as u16 + b as u16) / 3) as u8;
        let max_diff = r.abs_diff(avg).max(g.abs_diff(avg)).max(b.abs_diff(avg));

        if max_diff < 10 {
            // Use grayscale ramp (232-255)
            if avg < 8 {
                16 // Pure black in the cube
            } else if avg > 248 {
                231 // Pure white in the cube
            } else {
                232 + ((avg - 8) / 10)
            }
        } else {
            // Use 6×6×6 color cube (16-231)
            let r_idx = Self::rgb_to_cube_index(r);
            let g_idx = Self::rgb_to_cube_index(g);
            let b_idx = Self::rgb_to_cube_index(b);

            16 + (36 * r_idx) + (6 * g_idx) + b_idx
        }
    }

    /// Converts an RGB value to a 6-level color cube index.
    fn rgb_to_cube_index(val: u8) -> u8 {
        // 6-level color cube boundaries: 0, 95, 135, 175, 215, 255
        if val < 48 {
            0
        } else if val < 115 {
            1
        } else if val < 155 {
            2
        } else if val < 195 {
            3
        } else if val < 235 {
            4
        } else {
            5
        }
    }
}

// ============================================================================
// ANSI 16-color (basic color) conversion
// ============================================================================

/// Basic ANSI 16-color palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BasicColor {
    /// Black (color 0).
    Black,
    /// Red (color 1).
    Red,
    /// Green (color 2).
    Green,
    /// Yellow (color 3).
    Yellow,
    /// Blue (color 4).
    Blue,
    /// Magenta (color 5).
    Magenta,
    /// Cyan (color 6).
    Cyan,
    /// White (color 7).
    White,
    /// Bright black / dark gray (color 8).
    BrightBlack,
    /// Bright red (color 9).
    BrightRed,
    /// Bright green (color 10).
    BrightGreen,
    /// Bright yellow (color 11).
    BrightYellow,
    /// Bright blue (color 12).
    BrightBlue,
    /// Bright magenta (color 13).
    BrightMagenta,
    /// Bright cyan (color 14).
    BrightCyan,
    /// Bright white (color 15).
    BrightWhite,
}

impl BasicColor {
    /// Returns the ANSI foreground color code (30-37, 90-97).
    pub const fn fg_code(self) -> u8 {
        match self {
            Self::Black => 30,
            Self::Red => 31,
            Self::Green => 32,
            Self::Yellow => 33,
            Self::Blue => 34,
            Self::Magenta => 35,
            Self::Cyan => 36,
            Self::White => 37,
            Self::BrightBlack => 90,
            Self::BrightRed => 91,
            Self::BrightGreen => 92,
            Self::BrightYellow => 93,
            Self::BrightBlue => 94,
            Self::BrightMagenta => 95,
            Self::BrightCyan => 96,
            Self::BrightWhite => 97,
        }
    }

    /// Returns the ANSI background color code (40-47, 100-107).
    pub const fn bg_code(self) -> u8 {
        self.fg_code() + 10
    }

    /// Returns the ANSI 256 palette index for this basic color.
    pub const fn to_ansi_256(self) -> u8 {
        match self {
            Self::Black => 0,
            Self::Red => 1,
            Self::Green => 2,
            Self::Yellow => 3,
            Self::Blue => 4,
            Self::Magenta => 5,
            Self::Cyan => 6,
            Self::White => 7,
            Self::BrightBlack => 8,
            Self::BrightRed => 9,
            Self::BrightGreen => 10,
            Self::BrightYellow => 11,
            Self::BrightBlue => 12,
            Self::BrightMagenta => 13,
            Self::BrightCyan => 14,
            Self::BrightWhite => 15,
        }
    }

    /// Converts from an ANSI 256 palette index (0-15 only).
    pub fn from_ansi_256(index: u8) -> Option<Self> {
        match index {
            0 => Some(Self::Black),
            1 => Some(Self::Red),
            2 => Some(Self::Green),
            3 => Some(Self::Yellow),
            4 => Some(Self::Blue),
            5 => Some(Self::Magenta),
            6 => Some(Self::Cyan),
            7 => Some(Self::White),
            8 => Some(Self::BrightBlack),
            9 => Some(Self::BrightRed),
            10 => Some(Self::BrightGreen),
            11 => Some(Self::BrightYellow),
            12 => Some(Self::BrightBlue),
            13 => Some(Self::BrightMagenta),
            14 => Some(Self::BrightCyan),
            15 => Some(Self::BrightWhite),
            _ => None,
        }
    }

    /// Converts to a full `Color` value.
    pub fn to_color(self) -> Color {
        Color::from_ansi_256(self.to_ansi_256())
    }
}

impl Color {
    /// Converts the color to the nearest basic ANSI 16-color.
    pub fn to_basic_color(&self) -> BasicColor {
        let (r, g, b, _) = self.to_rgba_u8();

        // Calculate luminance to determine if bright variant
        let luma = (r as f32 * 0.299 + g as f32 * 0.587 + b as f32 * 0.114) / 255.0;
        let is_bright = luma > 0.5;

        // Find dominant channel(s)
        let max_val = r.max(g).max(b);
        let threshold = (max_val as f32 * 0.7) as u8;

        let has_r = r >= threshold && r > 30;
        let has_g = g >= threshold && g > 30;
        let has_b = b >= threshold && b > 30;

        match (has_r, has_g, has_b, is_bright) {
            (false, false, false, false) => BasicColor::Black,
            (false, false, false, true) => BasicColor::BrightBlack,
            (true, false, false, false) => BasicColor::Red,
            (true, false, false, true) => BasicColor::BrightRed,
            (false, true, false, false) => BasicColor::Green,
            (false, true, false, true) => BasicColor::BrightGreen,
            (true, true, false, false) => BasicColor::Yellow,
            (true, true, false, true) => BasicColor::BrightYellow,
            (false, false, true, false) => BasicColor::Blue,
            (false, false, true, true) => BasicColor::BrightBlue,
            (true, false, true, false) => BasicColor::Magenta,
            (true, false, true, true) => BasicColor::BrightMagenta,
            (false, true, true, false) => BasicColor::Cyan,
            (false, true, true, true) => BasicColor::BrightCyan,
            (true, true, true, false) => BasicColor::White,
            (true, true, true, true) => BasicColor::BrightWhite,
        }
    }
}

// ============================================================================
// ANSI escape sequence generation
// ============================================================================

impl Color {
    /// Generates the ANSI escape sequence for setting this color as foreground.
    ///
    /// Uses 24-bit true color format: `\x1b[38;2;R;G;Bm`
    pub fn to_ansi_fg(&self) -> String {
        let (r, g, b, _) = self.to_rgba_u8();
        format!("\x1b[38;2;{};{};{}m", r, g, b)
    }

    /// Generates the ANSI escape sequence for setting this color as background.
    ///
    /// Uses 24-bit true color format: `\x1b[48;2;R;G;Bm`
    /// For transparent colors, returns the default background escape sequence.
    pub fn to_ansi_bg(&self) -> String {
        if self.is_transparent() {
            return "\x1b[49m".to_string();
        }
        let (r, g, b, _) = self.to_rgba_u8();
        format!("\x1b[48;2;{};{};{}m", r, g, b)
    }

    /// Generates the ANSI escape sequence for setting this color as foreground
    /// using the 256-color palette.
    pub fn to_ansi_fg_256(&self) -> String {
        format!("\x1b[38;5;{}m", self.to_ansi_256())
    }

    /// Generates the ANSI escape sequence for setting this color as background
    /// using the 256-color palette.
    pub fn to_ansi_bg_256(&self) -> String {
        if self.is_transparent() {
            return "\x1b[49m".to_string();
        }
        format!("\x1b[48;5;{}m", self.to_ansi_256())
    }

    /// Generates the ANSI escape sequence for setting this color as foreground
    /// using the basic 16-color palette.
    pub fn to_ansi_fg_16(&self) -> String {
        format!("\x1b[{}m", self.to_basic_color().fg_code())
    }

    /// Generates the ANSI escape sequence for setting this color as background
    /// using the basic 16-color palette.
    pub fn to_ansi_bg_16(&self) -> String {
        if self.is_transparent() {
            return "\x1b[49m".to_string();
        }
        format!("\x1b[{}m", self.to_basic_color().bg_code())
    }
}

// ============================================================================
// Component access and conversion
// ============================================================================

impl Color {
    /// Returns the color components as u8 values (0-255).
    #[inline]
    pub fn to_rgba_u8(&self) -> (u8, u8, u8, u8) {
        (
            (self.r.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.g.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.b.clamp(0.0, 1.0) * 255.0).round() as u8,
            (self.a.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    }

    /// Returns the RGB components as u8 values (0-255).
    #[inline]
    pub fn to_rgb_u8(&self) -> (u8, u8, u8) {
        let (r, g, b, _) = self.to_rgba_u8();
        (r, g, b)
    }

    /// Returns the color as a packed u32 value (0xRRGGBBAA).
    #[inline]
    pub fn to_u32(&self) -> u32 {
        let (r, g, b, a) = self.to_rgba_u8();
        ((r as u32) << 24) | ((g as u32) << 16) | ((b as u32) << 8) | (a as u32)
    }

    /// Creates a color from a packed u32 value (0xRRGGBBAA).
    #[inline]
    pub fn from_u32(value: u32) -> Self {
        Self::from_rgba_u8(
            ((value >> 24) & 0xFF) as u8,
            ((value >> 16) & 0xFF) as u8,
            ((value >> 8) & 0xFF) as u8,
            (value & 0xFF) as u8,
        )
    }
}

// ============================================================================
// Alpha and transparency
// ============================================================================

impl Color {
    /// Returns whether the color has any transparency (alpha < 1.0).
    #[inline]
    pub fn has_alpha(&self) -> bool {
        self.a < 1.0
    }

    /// Returns whether the color is fully transparent (alpha ≈ 0).
    #[inline]
    pub fn is_transparent(&self) -> bool {
        self.a < 0.001
    }

    /// Returns whether the color is fully opaque (alpha ≈ 1.0).
    #[inline]
    pub fn is_opaque(&self) -> bool {
        self.a >= 0.999
    }

    /// Returns a new color with the specified alpha value.
    #[inline]
    pub const fn with_alpha(self, alpha: f32) -> Self {
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a: alpha,
        }
    }

    /// Returns a new color with the alpha multiplied by the given factor.
    #[inline]
    pub fn multiply_alpha(self, factor: f32) -> Self {
        Self {
            r: self.r,
            g: self.g,
            b: self.b,
            a: (self.a * factor).clamp(0.0, 1.0),
        }
    }

    /// Returns a fully opaque version of this color.
    #[inline]
    pub const fn opaque(self) -> Self {
        self.with_alpha(1.0)
    }
}

// ============================================================================
// Color manipulation
// ============================================================================

impl Color {
    /// Returns a lighter version of the color.
    ///
    /// `amount` is in the range 0.0 to 1.0, where 1.0 produces white.
    #[inline]
    pub fn lighten(self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        Self {
            r: self.r + (1.0 - self.r) * amount,
            g: self.g + (1.0 - self.g) * amount,
            b: self.b + (1.0 - self.b) * amount,
            a: self.a,
        }
    }

    /// Returns a darker version of the color.
    ///
    /// `amount` is in the range 0.0 to 1.0, where 1.0 produces black.
    #[inline]
    pub fn darken(self, amount: f32) -> Self {
        let amount = amount.clamp(0.0, 1.0);
        Self {
            r: self.r * (1.0 - amount),
            g: self.g * (1.0 - amount),
            b: self.b * (1.0 - amount),
            a: self.a,
        }
    }

    /// Returns the color with inverted RGB components.
    #[inline]
    pub const fn invert(self) -> Self {
        Self {
            r: 1.0 - self.r,
            g: 1.0 - self.g,
            b: 1.0 - self.b,
            a: self.a,
        }
    }

    /// Returns a grayscale version of the color using luminance weighting.
    #[inline]
    pub fn grayscale(self) -> Self {
        // Use standard luminance coefficients (Rec. 709)
        let luma = self.r * 0.2126 + self.g * 0.7152 + self.b * 0.0722;
        Self {
            r: luma,
            g: luma,
            b: luma,
            a: self.a,
        }
    }

    /// Returns the luminance of the color (0.0 to 1.0).
    #[inline]
    pub fn luminance(&self) -> f32 {
        self.r * 0.2126 + self.g * 0.7152 + self.b * 0.0722
    }

    /// Linearly interpolates between this color and another.
    ///
    /// `t` is in the range 0.0 to 1.0, where 0.0 returns `self` and 1.0 returns `other`.
    #[inline]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            r: self.r + (other.r - self.r) * t,
            g: self.g + (other.g - self.g) * t,
            b: self.b + (other.b - self.b) * t,
            a: self.a + (other.a - self.a) * t,
        }
    }

    /// Returns true if this color is approximately equal to another.
    #[inline]
    pub fn approx_eq(&self, other: &Self, epsilon: f32) -> bool {
        (self.r - other.r).abs() < epsilon
            && (self.g - other.g).abs() < epsilon
            && (self.b - other.b).abs() < epsilon
            && (self.a - other.a).abs() < epsilon
    }
}

// ============================================================================
// Alpha blending
// ============================================================================

impl Color {
    /// Blends this color over a background color using standard alpha compositing.
    ///
    /// This implements the Porter-Duff "over" operation.
    #[inline]
    pub fn blend_over(self, background: Self) -> Self {
        // Fully opaque overlay - no blending needed
        if self.a >= 1.0 {
            return self;
        }

        // Fully transparent overlay - return background
        if self.a <= 0.0 {
            return background;
        }

        let out_a = self.a + background.a * (1.0 - self.a);

        if out_a <= 0.0 {
            return Self::TRANSPARENT;
        }

        Self {
            r: (self.r * self.a + background.r * background.a * (1.0 - self.a)) / out_a,
            g: (self.g * self.a + background.g * background.a * (1.0 - self.a)) / out_a,
            b: (self.b * self.a + background.b * background.a * (1.0 - self.a)) / out_a,
            a: out_a,
        }
    }

    /// Blends this color over a background using perceptual alpha.
    ///
    /// This uses a perceptual alpha curve that produces better visual results
    /// for terminal UIs with transparency effects.
    #[inline]
    pub fn blend_perceptual(self, background: Self) -> Self {
        // Fully opaque overlay - no blending needed
        if self.a >= 1.0 {
            return self;
        }

        // Fully transparent overlay - return background
        if self.a <= 0.0 {
            return background;
        }

        // Apply perceptual alpha curve for better visual blending
        let perceptual_alpha = if self.a > 0.8 {
            // For high alpha values, use more aggressive curve
            let normalized = (self.a - 0.8) * 5.0;
            let curved = normalized.powf(0.2);
            0.8 + (curved * 0.2)
        } else {
            self.a.powf(0.9)
        };

        let inv_alpha = 1.0 - perceptual_alpha;

        Self {
            r: self.r * perceptual_alpha + background.r * inv_alpha,
            g: self.g * perceptual_alpha + background.g * inv_alpha,
            b: self.b * perceptual_alpha + background.b * inv_alpha,
            a: background.a, // Preserve background alpha
        }
    }

    /// Pre-multiplies the RGB components by the alpha value.
    #[inline]
    pub fn premultiply(self) -> Self {
        Self {
            r: self.r * self.a,
            g: self.g * self.a,
            b: self.b * self.a,
            a: self.a,
        }
    }

    /// Converts from pre-multiplied alpha to straight alpha.
    #[inline]
    pub fn unpremultiply(self) -> Self {
        if self.a <= 0.0 {
            return Self::TRANSPARENT;
        }
        Self {
            r: self.r / self.a,
            g: self.g / self.a,
            b: self.b / self.a,
            a: self.a,
        }
    }
}

// ============================================================================
// Type conversions
// ============================================================================

impl From<(f32, f32, f32)> for Color {
    fn from((r, g, b): (f32, f32, f32)) -> Self {
        Self::rgb(r, g, b)
    }
}

impl From<(f32, f32, f32, f32)> for Color {
    fn from((r, g, b, a): (f32, f32, f32, f32)) -> Self {
        Self::rgba(r, g, b, a)
    }
}

impl From<(u8, u8, u8)> for Color {
    fn from((r, g, b): (u8, u8, u8)) -> Self {
        Self::from_rgb_u8(r, g, b)
    }
}

impl From<(u8, u8, u8, u8)> for Color {
    fn from((r, g, b, a): (u8, u8, u8, u8)) -> Self {
        Self::from_rgba_u8(r, g, b, a)
    }
}

impl From<BasicColor> for Color {
    fn from(basic: BasicColor) -> Self {
        basic.to_color()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod constructor_tests {
        use super::*;

        #[test]
        fn test_rgb() {
            let c = Color::rgb(0.5, 0.25, 0.75);
            assert_eq!(c.r, 0.5);
            assert_eq!(c.g, 0.25);
            assert_eq!(c.b, 0.75);
            assert_eq!(c.a, 1.0);
        }

        #[test]
        fn test_rgba() {
            let c = Color::rgba(0.5, 0.25, 0.75, 0.5);
            assert_eq!(c.r, 0.5);
            assert_eq!(c.g, 0.25);
            assert_eq!(c.b, 0.75);
            assert_eq!(c.a, 0.5);
        }

        #[test]
        fn test_from_rgb_u8() {
            let c = Color::from_rgb_u8(255, 128, 0);
            assert!((c.r - 1.0).abs() < 0.01);
            assert!((c.g - 0.502).abs() < 0.01);
            assert!((c.b - 0.0).abs() < 0.01);
            assert_eq!(c.a, 1.0);
        }

        #[test]
        fn test_from_rgba_u8() {
            let c = Color::from_rgba_u8(255, 128, 0, 128);
            assert!((c.a - 0.502).abs() < 0.01);
        }

        #[test]
        fn test_gray() {
            let c = Color::gray(0.5);
            assert_eq!(c.r, 0.5);
            assert_eq!(c.g, 0.5);
            assert_eq!(c.b, 0.5);
            assert_eq!(c.a, 1.0);
        }
    }

    mod hex_tests {
        use super::*;

        #[test]
        fn test_from_hex_6digit() {
            let c = Color::from_hex("#FF8000").unwrap();
            let (r, g, b, a) = c.to_rgba_u8();
            assert_eq!(r, 255);
            assert_eq!(g, 128);
            assert_eq!(b, 0);
            assert_eq!(a, 255);
        }

        #[test]
        fn test_from_hex_8digit() {
            let c = Color::from_hex("#FF800080").unwrap();
            let (r, g, b, a) = c.to_rgba_u8();
            assert_eq!(r, 255);
            assert_eq!(g, 128);
            assert_eq!(b, 0);
            assert_eq!(a, 128);
        }

        #[test]
        fn test_from_hex_3digit() {
            let c = Color::from_hex("#F80").unwrap();
            let (r, g, b, _) = c.to_rgba_u8();
            assert_eq!(r, 255);
            assert_eq!(g, 136);
            assert_eq!(b, 0);
        }

        #[test]
        fn test_from_hex_4digit() {
            let c = Color::from_hex("#F808").unwrap();
            let (r, g, b, a) = c.to_rgba_u8();
            assert_eq!(r, 255);
            assert_eq!(g, 136);
            assert_eq!(b, 0);
            assert_eq!(a, 136);
        }

        #[test]
        fn test_from_hex_no_hash() {
            let c = Color::from_hex("FF8000").unwrap();
            let (r, g, b, _) = c.to_rgba_u8();
            assert_eq!(r, 255);
            assert_eq!(g, 128);
            assert_eq!(b, 0);
        }

        #[test]
        fn test_from_hex_invalid_length() {
            assert!(matches!(
                Color::from_hex("#12345"),
                Err(ColorParseError::InvalidLength(5))
            ));
        }

        #[test]
        fn test_from_hex_invalid_char() {
            assert!(matches!(
                Color::from_hex("#GGGGGG"),
                Err(ColorParseError::InvalidHexChar)
            ));
        }

        #[test]
        fn test_to_hex() {
            assert_eq!(Color::RED.to_hex(), "#FF0000");
            assert_eq!(Color::from_rgba_u8(255, 0, 0, 128).to_hex(), "#FF000080");
        }
    }

    mod parse_tests {
        use super::*;

        #[test]
        fn test_parse_transparent() {
            let c = Color::parse("transparent").unwrap();
            assert!(c.is_transparent());
        }

        #[test]
        fn test_parse_named_colors() {
            assert_eq!(Color::parse("red").unwrap(), Color::RED);
            assert_eq!(Color::parse("Red").unwrap(), Color::RED);
            assert_eq!(Color::parse("RED").unwrap(), Color::RED);
            assert_eq!(Color::parse("blue").unwrap(), Color::BLUE);
            assert_eq!(Color::parse("green").unwrap(), Color::GREEN);
        }

        #[test]
        fn test_parse_hex() {
            let c = Color::parse("#FF0000").unwrap();
            assert_eq!(c, Color::RED);
        }
    }

    mod hsv_tests {
        use super::*;

        #[test]
        fn test_from_hsv() {
            // Red at 0°
            let c = Color::from_hsv(0.0, 1.0, 1.0);
            assert!((c.r - 1.0).abs() < 0.01);
            assert!(c.g.abs() < 0.01);
            assert!(c.b.abs() < 0.01);

            // Green at 120°
            let c = Color::from_hsv(120.0, 1.0, 1.0);
            assert!(c.r.abs() < 0.01);
            assert!((c.g - 1.0).abs() < 0.01);
            assert!(c.b.abs() < 0.01);

            // Blue at 240°
            let c = Color::from_hsv(240.0, 1.0, 1.0);
            assert!(c.r.abs() < 0.01);
            assert!(c.g.abs() < 0.01);
            assert!((c.b - 1.0).abs() < 0.01);
        }

        #[test]
        fn test_to_hsv_roundtrip() {
            let colors = [Color::RED, Color::GREEN, Color::BLUE, Color::YELLOW];
            for original in colors {
                let (h, s, v) = original.to_hsv();
                let converted = Color::from_hsv(h, s, v);
                assert!(original.approx_eq(&converted, 0.01));
            }
        }
    }

    mod ansi_tests {
        use super::*;

        #[test]
        fn test_from_ansi_256_standard() {
            assert_eq!(Color::from_ansi_256(0), Color::ANSI_BLACK);
            assert_eq!(Color::from_ansi_256(1), Color::ANSI_RED);
            assert_eq!(Color::from_ansi_256(15), Color::ANSI_BRIGHT_WHITE);
        }

        #[test]
        fn test_from_ansi_256_cube() {
            // Pure red in the cube (index 196 = 16 + 5*36)
            let c = Color::from_ansi_256(196);
            let (r, g, b, _) = c.to_rgba_u8();
            assert_eq!(r, 255);
            assert_eq!(g, 0);
            assert_eq!(b, 0);
        }

        #[test]
        fn test_from_ansi_256_grayscale() {
            let c = Color::from_ansi_256(244); // Mid-gray
            let (r, g, b, _) = c.to_rgba_u8();
            assert_eq!(r, g);
            assert_eq!(g, b);
        }

        #[test]
        fn test_to_ansi_256_grayscale() {
            let gray = Color::gray(0.5);
            let idx = gray.to_ansi_256();
            // Grayscale range is 232-255
            assert!(idx >= 232);
        }

        #[test]
        fn test_to_ansi_256_color() {
            let idx = Color::RED.to_ansi_256();
            // Should map to the color cube, not grayscale
            assert!((16..=231).contains(&idx));
        }

        #[test]
        fn test_basic_color_codes() {
            assert_eq!(BasicColor::Black.fg_code(), 30);
            assert_eq!(BasicColor::Black.bg_code(), 40);
            assert_eq!(BasicColor::BrightRed.fg_code(), 91);
            assert_eq!(BasicColor::BrightRed.bg_code(), 101);
        }
    }

    mod escape_sequence_tests {
        use super::*;

        #[test]
        fn test_to_ansi_fg() {
            let c = Color::from_rgb_u8(255, 128, 64);
            assert_eq!(c.to_ansi_fg(), "\x1b[38;2;255;128;64m");
        }

        #[test]
        fn test_to_ansi_bg() {
            let c = Color::from_rgb_u8(255, 128, 64);
            assert_eq!(c.to_ansi_bg(), "\x1b[48;2;255;128;64m");
        }

        #[test]
        fn test_to_ansi_bg_transparent() {
            let c = Color::TRANSPARENT;
            assert_eq!(c.to_ansi_bg(), "\x1b[49m");
        }

        #[test]
        fn test_to_ansi_256_sequences() {
            let c = Color::from_rgb_u8(255, 0, 0);
            assert!(c.to_ansi_fg_256().starts_with("\x1b[38;5;"));
            assert!(c.to_ansi_bg_256().starts_with("\x1b[48;5;"));
        }

        #[test]
        fn test_to_ansi_16_sequences() {
            let c = Color::RED;
            assert!(c.to_ansi_fg_16().starts_with("\x1b["));
            assert!(c.to_ansi_bg_16().starts_with("\x1b["));
        }
    }

    mod alpha_tests {
        use super::*;

        #[test]
        fn test_has_alpha() {
            assert!(!Color::RED.has_alpha());
            assert!(Color::TRANSPARENT.has_alpha());
            assert!(Color::rgba(1.0, 0.0, 0.0, 0.5).has_alpha());
        }

        #[test]
        fn test_is_transparent() {
            assert!(Color::TRANSPARENT.is_transparent());
            assert!(!Color::RED.is_transparent());
            assert!(!Color::rgba(1.0, 0.0, 0.0, 0.1).is_transparent());
        }

        #[test]
        fn test_is_opaque() {
            assert!(Color::RED.is_opaque());
            assert!(!Color::TRANSPARENT.is_opaque());
            assert!(!Color::rgba(1.0, 0.0, 0.0, 0.9).is_opaque());
        }

        #[test]
        fn test_with_alpha() {
            let c = Color::RED.with_alpha(0.5);
            assert_eq!(c.r, 1.0);
            assert_eq!(c.a, 0.5);
        }

        #[test]
        fn test_multiply_alpha() {
            let c = Color::rgba(1.0, 0.0, 0.0, 0.8).multiply_alpha(0.5);
            assert!((c.a - 0.4).abs() < 0.001);
        }
    }

    mod manipulation_tests {
        use super::*;

        #[test]
        fn test_lighten() {
            let c = Color::rgb(0.5, 0.5, 0.5).lighten(0.5);
            assert!((c.r - 0.75).abs() < 0.01);
            assert!((c.g - 0.75).abs() < 0.01);
            assert!((c.b - 0.75).abs() < 0.01);
        }

        #[test]
        fn test_darken() {
            let c = Color::rgb(0.5, 0.5, 0.5).darken(0.5);
            assert!((c.r - 0.25).abs() < 0.01);
            assert!((c.g - 0.25).abs() < 0.01);
            assert!((c.b - 0.25).abs() < 0.01);
        }

        #[test]
        fn test_invert() {
            let c = Color::rgb(0.2, 0.4, 0.6).invert();
            assert!((c.r - 0.8).abs() < 0.01);
            assert!((c.g - 0.6).abs() < 0.01);
            assert!((c.b - 0.4).abs() < 0.01);
        }

        #[test]
        fn test_grayscale() {
            let c = Color::RED.grayscale();
            assert!(c.r == c.g && c.g == c.b);
        }

        #[test]
        fn test_lerp() {
            let a = Color::BLACK;
            let b = Color::WHITE;

            let mid = a.lerp(b, 0.5);
            assert!((mid.r - 0.5).abs() < 0.01);
            assert!((mid.g - 0.5).abs() < 0.01);
            assert!((mid.b - 0.5).abs() < 0.01);

            let start = a.lerp(b, 0.0);
            assert!(start.approx_eq(&a, 0.001));

            let end = a.lerp(b, 1.0);
            assert!(end.approx_eq(&b, 0.001));
        }
    }

    mod blending_tests {
        use super::*;

        #[test]
        fn test_blend_over_opaque() {
            let fg = Color::RED;
            let bg = Color::BLUE;
            let result = fg.blend_over(bg);
            assert!(result.approx_eq(&Color::RED, 0.001));
        }

        #[test]
        fn test_blend_over_transparent() {
            let fg = Color::TRANSPARENT;
            let bg = Color::BLUE;
            let result = fg.blend_over(bg);
            assert!(result.approx_eq(&Color::BLUE, 0.001));
        }

        #[test]
        fn test_blend_over_semi_transparent() {
            let fg = Color::rgba(1.0, 0.0, 0.0, 0.5);
            let bg = Color::rgba(0.0, 0.0, 1.0, 1.0);
            let result = fg.blend_over(bg);
            // Result should be a purple-ish color
            assert!(result.r > 0.0);
            assert!(result.b > 0.0);
        }

        #[test]
        fn test_premultiply_unpremultiply() {
            let original = Color::rgba(1.0, 0.5, 0.25, 0.5);
            let premultiplied = original.premultiply();
            let back = premultiplied.unpremultiply();
            assert!(original.approx_eq(&back, 0.001));
        }
    }

    mod conversion_tests {
        use super::*;

        #[test]
        fn test_to_u32_from_u32_roundtrip() {
            let colors = [Color::RED, Color::GREEN, Color::BLUE, Color::WHITE];
            for original in colors {
                let packed = original.to_u32();
                let restored = Color::from_u32(packed);
                // Due to f32 -> u8 -> f32 conversion, we allow small differences
                assert!(original.approx_eq(&restored, 0.01));
            }
        }

        #[test]
        fn test_from_tuple() {
            let c: Color = (0.5_f32, 0.25_f32, 0.75_f32).into();
            assert_eq!(c.r, 0.5);
            assert_eq!(c.g, 0.25);
            assert_eq!(c.b, 0.75);
            assert_eq!(c.a, 1.0);

            let c: Color = (128_u8, 64_u8, 192_u8).into();
            assert!((c.r - 0.502).abs() < 0.01);
        }
    }
}
