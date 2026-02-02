//! Color interpolation and easing functions.

use ratatui::style::Color;

/// Interpolates between two colors based on a factor t (0.0 to 1.0).
///
/// # Arguments
/// * `from` - Starting color (at t=0.0)
/// * `to` - Ending color (at t=1.0)
/// * `t` - Interpolation factor (clamped to 0.0-1.0)
///
/// # Example
/// ```
/// use cortex_engine::animation::interpolate_color;
/// use ratatui::style::Color;
///
/// let from = Color::Rgb(0, 255, 255);
/// let to = Color::Rgb(125, 249, 255);
/// let mid = interpolate_color(from, to, 0.5);
/// ```
pub fn interpolate_color(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);

    // Extract RGB components, defaulting to white if not RGB
    let (r1, g1, b1) = extract_rgb(from);
    let (r2, g2, b2) = extract_rgb(to);

    // Linear interpolation
    let r = lerp_u8(r1, r2, t);
    let g = lerp_u8(g1, g2, t);
    let b = lerp_u8(b1, b2, t);

    Color::Rgb(r, g, b)
}

/// Extracts RGB components from a Color, defaulting to white for non-RGB colors.
pub(crate) fn extract_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Rgb(r, g, b) => (r, g, b),
        _ => (255, 255, 255), // Default to white
    }
}

/// Linear interpolation between two u8 values.
#[inline]
pub(crate) fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let a = a as f32;
    let b = b as f32;
    (a + (b - a) * t).round() as u8
}

/// Ease-in-out curve for smooth animation.
///
/// Uses a cubic bezier approximation for natural-feeling motion.
#[inline]
pub fn ease_in_out(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}
