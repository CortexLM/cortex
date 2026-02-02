//! Shared UI constants for layout and alignment within the TUI.
//!
//! These constants ensure consistent spacing and sizing across all widgets and cards.

/// Width (in terminal columns) reserved for the left gutter/prefix used by
/// live cells and aligned widgets.
///
/// Semantics:
/// - Chat composer reserves this many columns for the left border + padding.
/// - Status indicator lines begin with this many spaces for alignment.
/// - User history lines account for this many columns (e.g., "▌ ") when wrapping.
pub const LIVE_PREFIX_COLS: u16 = 2;

/// Indent columns for footer content.
pub const FOOTER_INDENT_COLS: usize = LIVE_PREFIX_COLS as usize;

/// Maximum number of visible rows in popup/card selection lists.
pub const MAX_POPUP_ROWS: usize = 10;

/// Minimum height for the input composer area.
pub const MIN_INPUT_HEIGHT: u16 = 3;

/// Height reserved for key hints line.
pub const KEY_HINTS_HEIGHT: u16 = 1;

/// Height reserved for status indicator when active.
pub const STATUS_INDICATOR_HEIGHT: u16 = 1;

/// Maximum height percentage for cards (relative to terminal height).
pub const MAX_CARD_HEIGHT_PERCENT: u16 = 70;

/// Padding between card border and content.
pub const CARD_PADDING: u16 = 1;

/// Border characters for rounded corners (Unicode box drawing).
pub mod border {
    pub const TOP_LEFT: char = '╭';
    pub const TOP_RIGHT: char = '╮';
    pub const BOTTOM_LEFT: char = '╰';
    pub const BOTTOM_RIGHT: char = '╯';
    pub const HORIZONTAL: char = '─';
    pub const VERTICAL: char = '│';

    // Selection indicators (ASCII for compatibility)
    pub const ARROW_RIGHT: char = '>';
    pub const ARROW_LEFT: char = '<';
    pub const BULLET: char = '*';
    pub const DOT: char = '.';
}

/// Spinner animation frames (braille pattern) - legacy.
pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Spinner animation frames for streaming ("breathing" pattern).
/// Ordered by visual weight for smooth animation:
/// - · (point) → ✢ (cross) → ✻ (thin asterisk) → ✽ (heavy asterisk)
///
/// Uses ping-pong pattern for fluid "breathing" effect.
/// Soft/organic variant without pointed star (✶) for smoother curves.
pub const STREAMING_SPINNER_FRAMES: &[char] = &[
    '·', // 1. Start - minimal
    '✢', // 2. Opening - 4 branches
    '✻', // 3. Expansion - 6 thin branches
    '✽', // 4. Peak - 6 heavy branches (full)
    '✻', // 3. Descending...
    '✢', // 2. Back down
         // Loop naturally returns to '·'
];

/// Spinner animation interval in milliseconds.
pub const SPINNER_INTERVAL_MS: u64 = 80;

/// Tool execution spinner frames (half-circles for smooth rotation)
pub const TOOL_SPINNER_FRAMES: &[char] = &['◐', '◑', '◒', '◓'];

/// Default cursor blink interval in milliseconds.
pub const CURSOR_BLINK_INTERVAL_MS: u64 = 500;

/// Gets the cursor blink interval respecting system accessibility settings.
///
/// On Linux/GNOME, attempts to read the `cursor-blink-time` setting.
/// On macOS, checks accessibility preferences.
/// Falls back to the default interval if the system setting cannot be read.
///
/// Returns 0 if cursor blinking should be disabled.
pub fn get_system_cursor_blink_interval() -> u64 {
    // Check CORTEX_CURSOR_BLINK environment variable for user override
    if let Ok(val) = std::env::var("CORTEX_CURSOR_BLINK") {
        if val == "0" || val.to_lowercase() == "false" {
            return 0; // Disable blinking
        }
        if let Ok(ms) = val.parse::<u64>() {
            return ms;
        }
    }

    // Try to detect system settings
    #[cfg(target_os = "linux")]
    {
        // Try GNOME settings via gsettings output parsing
        // cursor-blink-time is in milliseconds, cursor-blink is bool
        if let Ok(output) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "cursor-blink"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.trim() == "false" {
                return 0; // Blinking disabled
            }
        }

        if let Ok(output) = std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "cursor-blink-time"])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Ok(ms) = stdout.trim().parse::<u64>() {
                return ms;
            }
        }
    }

    // Default interval
    CURSOR_BLINK_INTERVAL_MS
}

/// Shimmer animation period in seconds.
pub const SHIMMER_PERIOD_SECS: f32 = 2.0;

/// Shimmer band half-width in characters.
pub const SHIMMER_BAND_WIDTH: f32 = 5.0;
