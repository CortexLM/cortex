//! Shimmer text effect for loading animations
//!
//! Creates a "shimmering" effect where a bright band moves across the text,
//! similar to a loading skeleton or highlight sweep animation.

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::colors::{blend, detect_terminal_bg};

/// Process start time for synchronized animations across the application.
static PROCESS_START: OnceLock<Instant> = OnceLock::new();

/// Period of a full shimmer sweep in seconds.
const SHIMMER_PERIOD_SECS: f32 = 2.0;

/// Half-width of the shimmer band in characters.
const SHIMMER_BAND_WIDTH: f32 = 5.0;

/// Get elapsed time since process start for synchronized animations.
///
/// This ensures all shimmer effects across the UI are in sync, creating
/// a cohesive visual experience.
pub fn elapsed_since_start() -> Duration {
    let start = PROCESS_START.get_or_init(Instant::now);
    start.elapsed()
}

/// Create shimmer-styled spans for animated text.
///
/// Returns a Vec of Span where each character has a different style based on
/// its position relative to the moving shimmer band. The effect creates a
/// smooth wave of brightness that sweeps across the text.
///
/// # Arguments
///
/// * `text` - The text to apply the shimmer effect to
///
/// # Returns
///
/// A Vec of owned Spans, one per character, with styles that create
/// the shimmer wave effect when rendered together.
///
/// # Example
///
/// ```ignore
/// use cortex_tui::ui::shimmer::shimmer_spans;
///
/// let spans = shimmer_spans("Loading...");
/// // Render spans in your TUI frame
/// ```
pub fn shimmer_spans(text: &str) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return Vec::new();
    }

    // Detect if we have TrueColor support
    let has_true_color = supports_true_color();

    // Get terminal background for color blending
    let bg = detect_terminal_bg().unwrap_or((0x1A, 0x1A, 0x1A));
    let is_light_bg = is_light_background(bg);

    // Calculate current shimmer position
    // Animation sweeps from left to right, then wraps around
    let padding = 10usize; // Extra padding at start/end for smooth wrap
    let period = chars.len() + padding * 2;
    let elapsed = elapsed_since_start().as_secs_f32();
    let progress = (elapsed % SHIMMER_PERIOD_SECS) / SHIMMER_PERIOD_SECS;
    let shimmer_pos = (progress * period as f32) as isize;

    let mut spans = Vec::with_capacity(chars.len());

    for (i, ch) in chars.iter().enumerate() {
        // Calculate distance from shimmer center
        let char_pos = i as isize + padding as isize;
        let distance = (char_pos - shimmer_pos).abs() as f32;

        // Calculate intensity using cosine interpolation for smooth falloff
        let intensity = if distance <= SHIMMER_BAND_WIDTH {
            // Cosine interpolation: 1.0 at center, 0.0 at edge
            let x = std::f32::consts::PI * (distance / SHIMMER_BAND_WIDTH);
            0.5 * (1.0 + x.cos())
        } else {
            0.0
        };

        // Apply style based on terminal capabilities
        let style = if has_true_color {
            style_for_intensity_rgb(intensity, is_light_bg, bg)
        } else {
            color_for_level(intensity)
        };

        spans.push(Span::styled(ch.to_string(), style));
    }

    spans
}

/// Calculate style for a given intensity level using RGB colors.
///
/// Blends between base and highlight colors based on intensity.
fn style_for_intensity_rgb(intensity: f32, is_light_bg: bool, bg: (u8, u8, u8)) -> Style {
    // Define base and highlight colors based on background
    let (base, highlight) = if is_light_bg {
        // Light background: dark gray to black
        ((0x80, 0x80, 0x80), (0x1A, 0x1A, 0x1A))
    } else {
        // Dark background: dim gray to bright white
        ((0x69, 0x69, 0x69), (0xFF, 0xFF, 0xFF))
    };

    // Blend colors based on intensity
    let blended = blend(highlight, base, intensity);

    // Ensure blended color has good contrast with background
    let final_color = blend(blended, bg, 0.95);

    let mut style = Style::default().fg(Color::Rgb(final_color.0, final_color.1, final_color.2));

    // Add bold modifier for high intensity
    if intensity > 0.6 {
        style = style.add_modifier(Modifier::BOLD);
    }

    style
}

/// Calculate style for terminals without TrueColor support.
///
/// Uses text modifiers (Bold, Dim) instead of RGB colors to create
/// a similar shimmer effect on limited terminals.
///
/// # Arguments
///
/// * `intensity` - Value from 0.0 to 1.0 indicating brightness level
///
/// # Returns
///
/// A Style with appropriate modifiers:
/// - intensity < 0.2: Dim modifier (darkest)
/// - intensity 0.2-0.6: No modifier (normal)
/// - intensity > 0.6: Bold modifier (brightest)
pub fn color_for_level(intensity: f32) -> Style {
    if intensity < 0.2 {
        // Low intensity: dim text
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::DIM)
    } else if intensity > 0.6 {
        // High intensity: bold bright text
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        // Medium intensity: normal gray text
        Style::default().fg(Color::Gray)
    }
}

/// Check if terminal supports TrueColor (24-bit RGB).
fn supports_true_color() -> bool {
    // Check COLORTERM environment variable
    if let Ok(colorterm) = std::env::var("COLORTERM")
        && (colorterm == "truecolor" || colorterm == "24bit")
    {
        return true;
    }

    // Check TERM for known TrueColor terminals
    if let Ok(term) = std::env::var("TERM")
        && (term.contains("256color") || term.contains("truecolor") || term.contains("24bit"))
    {
        return true;
    }

    // Check for specific terminal programs known to support TrueColor
    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let known_truecolor = [
            "iTerm.app",
            "Hyper",
            "vscode",
            "Alacritty",
            "kitty",
            "WezTerm",
            "Ghostty",
        ];
        for prog in known_truecolor {
            if term_program.contains(prog) {
                return true;
            }
        }
    }

    // Default to false for safety
    false
}

/// Check if a background color is light.
fn is_light_background(bg: (u8, u8, u8)) -> bool {
    // Use relative luminance formula (ITU-R BT.709)
    let luminance = 0.2126 * (bg.0 as f32 / 255.0)
        + 0.7152 * (bg.1 as f32 / 255.0)
        + 0.0722 * (bg.2 as f32 / 255.0);
    luminance > 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_elapsed_since_start() {
        let d1 = elapsed_since_start();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let d2 = elapsed_since_start();
        assert!(d2 > d1);
    }

    #[test]
    fn test_shimmer_spans_empty() {
        let spans = shimmer_spans("");
        assert!(spans.is_empty());
    }

    #[test]
    fn test_shimmer_spans_length() {
        let text = "Hello, World!";
        let spans = shimmer_spans(text);
        assert_eq!(spans.len(), text.chars().count());
    }

    #[test]
    fn test_color_for_level_dim() {
        let style = color_for_level(0.1);
        assert!(style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn test_color_for_level_normal() {
        let style = color_for_level(0.4);
        assert!(!style.add_modifier.contains(Modifier::DIM));
        assert!(!style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_color_for_level_bold() {
        let style = color_for_level(0.8);
        assert!(style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn test_is_light_background() {
        assert!(is_light_background((255, 255, 255))); // White
        assert!(is_light_background((200, 200, 200))); // Light gray
        assert!(!is_light_background((0, 0, 0))); // Black
        assert!(!is_light_background((30, 30, 30))); // Dark gray
    }
}
