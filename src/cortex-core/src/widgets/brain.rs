//! Animated ASCII brain logo widget.
//!
//! The Brain widget displays an ASCII art brain logo with a smooth
//! radial pulsation animation that creates a "neural activity" effect.
//!
//! # Animation Technique
//! Uses sine wave radial expansion from center:
//! - Wave expands outward from brain center
//! - 2-second cycle (slow, hypnotic)
//! - Green gradient (#00FFA3)
//!
//! # Example
//! ```
//! use cortex_engine::widgets::Brain;
//!
//! // Animated brain with frame counter
//! let brain = Brain::new()
//!     .with_frame(frame_counter)
//!     .centered();
//! ```

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

// ============================================================
// ASCII ART - New compact brain
// ============================================================

/// The ASCII art brain logo - 12 lines
const BRAIN_ART: &[&str] = &[
    "          %%#*##*****###@@          ",
    "      @%***+****+++#+**#%#@@@@      ",
    "    @%*++*%#*+*+*#+++##*%%++@@@@    ",
    " @%***+**+++***+*#++*****%%@@%*%@@  ",
    " %++****+##++*++*##******%@*#@%#@@@ ",
    "@#*#*++++@#****+++++*##++##%%%*#@@@@",
    "%*++*+++***+*#*+++**++++**%@%#*%@@@@",
    "@**+**+**+#*#+++#*+###*#####@@@@@@@@",
    " ##@@%@@#+#**###%@@%%%%#####%@@@@@@@",
    "   @@@@ @@@@@#%@@@@@@%*+**#*#@@@@   ",
    "           @@@@   @@%%@%%%%%%@@@    ",
    "                     @##@@@@@@      ",
];

/// Width of the brain ASCII art in characters
const BRAIN_WIDTH: u16 = 37;

/// Height of the brain ASCII art in lines
const BRAIN_HEIGHT: u16 = 12;

// ============================================================
// ANIMATION CONSTANTS
// ============================================================

/// Wave speed: 2π rad/s = 1 second per full cycle
const WAVE_SPEED: f32 = std::f32::consts::PI * 2.0;

/// Wave scale: controls the "wavelength" of radial rings
const WAVE_SCALE: f32 = 6.0;

/// Accent green color
const ACCENT_GREEN: (u8, u8, u8) = (0x00, 0xFF, 0xA3);

/// Dark green base color
const DARK_GREEN: (u8, u8, u8) = (0x00, 0x40, 0x30);

// ============================================================
// BRAIN WIDGET
// ============================================================

/// Animated ASCII brain logo widget with radial pulsation.
///
/// Renders the Cortex brain logo with a smooth radial wave animation
/// that pulses outward from the center, creating a hypnotic "neural activity" effect.
///
/// # Example
/// ```
/// use cortex_engine::widgets::Brain;
///
/// // Animated brain with frame counter for pulsation effect
/// let brain = Brain::new()
///     .with_frame(frame_counter)
///     .centered();
/// ```
#[derive(Debug, Clone)]
pub struct Brain {
    /// Frame counter for animation (120 FPS assumed)
    frame: u64,
    /// Whether to center the brain in the render area
    centered: bool,
    /// Animation intensity (0.0 = no animation, 1.0 = full effect)
    intensity: f32,
}

impl Default for Brain {
    fn default() -> Self {
        Self::new()
    }
}

impl Brain {
    /// Creates a new Brain widget with default settings.
    pub fn new() -> Self {
        Self {
            frame: 0,
            centered: false,
            intensity: 1.0,
        }
    }

    /// Sets the frame counter for animation.
    ///
    /// The frame number drives the radial wave animation.
    /// Assumes 120 FPS for smooth animation.
    pub fn with_frame(mut self, frame: u64) -> Self {
        self.frame = frame;
        self
    }

    /// Sets the animation intensity.
    ///
    /// # Arguments
    /// * `intensity` - Value from 0.0 (static) to 1.0 (full pulsation)
    pub fn with_intensity(mut self, intensity: f32) -> Self {
        self.intensity = intensity.clamp(0.0, 1.0);
        self
    }

    /// Centers the brain horizontally and vertically in the render area.
    pub fn centered(mut self) -> Self {
        self.centered = true;
        self
    }

    /// Returns the width of the brain ASCII art in characters.
    #[inline]
    pub fn width() -> u16 {
        BRAIN_WIDTH
    }

    /// Returns the height of the brain ASCII art in lines.
    #[inline]
    pub fn height() -> u16 {
        BRAIN_HEIGHT
    }

    /// Returns the raw ASCII art lines.
    #[inline]
    pub fn lines() -> &'static [&'static str] {
        BRAIN_ART
    }

    /// Converts a character to its base density value (0.0 to 1.0).
    fn char_to_density(ch: char) -> f32 {
        match ch {
            ' ' => 0.0,
            '.' => 0.15,
            '+' => 0.30,
            '*' => 0.45,
            '#' => 0.60,
            '%' => 0.80,
            '@' => 1.0,
            _ => 0.5,
        }
    }

    /// Calculates the wave intensity for a given position.
    ///
    /// Uses radial sine wave expanding from center.
    fn calculate_wave(&self, x: u16, y: u16) -> f32 {
        // Time in seconds (120 FPS)
        let time = self.frame as f32 / 120.0;

        // Center of the brain
        let center_x = BRAIN_WIDTH as f32 / 2.0;
        let center_y = BRAIN_HEIGHT as f32 / 2.0;

        // Distance from center (radial)
        let dx = x as f32 - center_x;
        let dy = (y as f32 - center_y) * 2.0; // Scale Y for terminal aspect ratio
        let distance = (dx * dx + dy * dy).sqrt();

        // Radial sine wave: expands outward from center
        // wave = sin(distance / scale - time * speed)
        let wave = (distance / WAVE_SCALE - time * WAVE_SPEED).sin();

        // Normalize [-1, 1] → [0, 1]
        (wave + 1.0) / 2.0
    }

    /// Gets the style for a character based on wave intensity.
    ///
    /// Creates a gradient from dark green to bright green (#00FFA3).
    fn get_wave_style(&self, base_density: f32, wave: f32) -> Style {
        // Combine base density with wave for final brightness
        let brightness = base_density * (0.4 + wave * 0.6 * self.intensity);

        // Interpolate between dark green and accent green
        let r = DARK_GREEN.0;
        let g = (DARK_GREEN.1 as f32 + (ACCENT_GREEN.1 - DARK_GREEN.1) as f32 * brightness) as u8;
        let b = (DARK_GREEN.2 as f32 + (ACCENT_GREEN.2 - DARK_GREEN.2) as f32 * brightness) as u8;

        Style::default().fg(Color::Rgb(r, g, b))
    }

    /// Gets the display character using concentric band animation.
    ///
    /// Characters are grouped into bands that pulse outward from center.
    /// All characters in the same band display the same character,
    /// creating a fluid wave effect where similar chars move together.
    fn get_display_char(&self, original_ch: char, local_x: u16, local_y: u16) -> char {
        // Masking - keep the brain shape, skip spaces
        if original_ch == ' ' {
            return ' ';
        }

        let time = self.frame as f32 / 120.0;

        // Distance from center (radial)
        let center_x = BRAIN_WIDTH as f32 / 2.0;
        let center_y = BRAIN_HEIGHT as f32 / 2.0;
        let dx = local_x as f32 - center_x;
        let dy = (local_y as f32 - center_y) * 2.0; // Aspect ratio for terminal
        let distance = (dx * dx + dy * dy).sqrt();

        // Concentric bands moving outward from center
        let band_width = 2.5; // Fine bands
        let num_bands = 7.0;
        let moving_distance = distance - time * WAVE_SPEED * 3.0;
        let band_index = ((moving_distance / band_width).rem_euclid(num_bands)) as usize;

        // Character palette - each band has its own character
        // From lightest to densest: . : - = + * #
        const BAND_CHARS: [char; 7] = ['.', ':', '-', '=', '+', '*', '#'];
        BAND_CHARS[band_index]
    }
}

impl Widget for Brain {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Calculate positioning
        let (start_x, start_y) = if self.centered {
            let x_offset = area.width.saturating_sub(BRAIN_WIDTH) / 2;
            let y_offset = area.height.saturating_sub(BRAIN_HEIGHT) / 2;
            (area.x + x_offset, area.y + y_offset)
        } else {
            (area.x, area.y)
        };

        // Render each line of the brain
        for (line_idx, line) in BRAIN_ART.iter().enumerate() {
            let y = start_y + line_idx as u16;

            // Skip if outside the render area
            if y >= area.y + area.height {
                break;
            }

            // Render each character in the line
            for (char_idx, ch) in line.chars().enumerate() {
                let x = start_x + char_idx as u16;

                // Skip if outside the render area
                if x >= area.x + area.width {
                    break;
                }

                // Skip spaces (transparent - masking)
                if ch == ' ' {
                    continue;
                }

                // Calculate wave intensity for this position (for color)
                let wave = self.calculate_wave(char_idx as u16, line_idx as u16);

                // Get display character (concentric bands) and style
                let display_ch = self.get_display_char(ch, char_idx as u16, line_idx as u16);
                let base_density = Self::char_to_density(display_ch); // Use display char for color
                let style = self.get_wave_style(base_density, wave);

                // Render the character
                if display_ch != ' ' {
                    if let Some(cell) = buf.cell_mut((x, y)) {
                        cell.set_char(display_ch).set_style(style);
                    }
                }
            }
        }
    }
}

// ============================================================
// TESTS
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_brain_dimensions() {
        assert_eq!(Brain::width(), 37);
        assert_eq!(Brain::height(), 12);
        assert_eq!(Brain::lines().len(), 12);
    }

    #[test]
    fn test_brain_builder() {
        let brain = Brain::new();
        assert_eq!(brain.frame, 0);
        assert!(!brain.centered);

        let brain = Brain::new().with_frame(42).centered();
        assert_eq!(brain.frame, 42);
        assert!(brain.centered);
    }

    #[test]
    fn test_brain_default() {
        let brain = Brain::default();
        assert_eq!(brain.frame, 0);
        assert!(!brain.centered);
    }

    #[test]
    fn test_char_to_density() {
        assert_eq!(Brain::char_to_density(' '), 0.0);
        assert_eq!(Brain::char_to_density('@'), 1.0);
        assert!(Brain::char_to_density('#') > Brain::char_to_density('+'));
    }

    #[test]
    fn test_wave_calculation() {
        let brain = Brain::new().with_frame(0);
        let wave1 = brain.calculate_wave(0, 0);
        let wave2 = brain.calculate_wave(18, 6); // Center

        // Wave should be in [0, 1] range
        assert!(wave1 >= 0.0 && wave1 <= 1.0);
        assert!(wave2 >= 0.0 && wave2 <= 1.0);
    }

    #[test]
    fn test_brain_render_no_panic() {
        let brain = Brain::new().with_frame(100).centered();
        let area = Rect::new(0, 0, 80, 24);
        let mut buf = Buffer::empty(area);
        brain.render(area, &mut buf);
    }

    #[test]
    fn test_brain_animation_varies() {
        let brain1 = Brain::new().with_frame(0);
        let brain2 = Brain::new().with_frame(60); // 0.5 seconds later

        let wave1 = brain1.calculate_wave(10, 5);
        let wave2 = brain2.calculate_wave(10, 5);

        // Waves should differ between frames
        assert!((wave1 - wave2).abs() > 0.01);
    }
}
