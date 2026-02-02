//! Spinner type definitions and frame sets.

/// Types of spinners for different contexts
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpinnerType {
    /// Slow thinking animation - for AI processing
    Thinking,
    /// Fast tool execution - for running commands
    Tool,
    /// Streaming response - wave animation
    Streaming,
    /// Waiting for approval - pulsing
    Approval,
    /// Generic loading
    Loading,
    /// Progress with percentage
    Progress,
}

/// Spinner frame sets - NO EMOJIS, use Unicode/ASCII only
pub struct SpinnerFrames;

impl SpinnerFrames {
    /// Braille dots animation (smooth)
    pub const DOTS: &'static [&'static str] = &[
        "\u{28F7}", "\u{28EF}", "\u{28DF}", "\u{287F}", "\u{28BF}", "\u{28FB}", "\u{28FD}",
        "\u{28FE}",
    ];

    /// Simple line spinner
    pub const LINE: &'static [&'static str] = &["-", "\\", "|", "/"];

    /// Block animation (wave effect)
    pub const BLOCKS: &'static [&'static str] = &[
        "\u{2581}", "\u{2582}", "\u{2583}", "\u{2584}", "\u{2585}", "\u{2586}", "\u{2587}",
        "\u{2588}", "\u{2587}", "\u{2586}", "\u{2585}", "\u{2584}", "\u{2583}", "\u{2582}",
    ];

    /// Bouncing dot (braille)
    pub const BOUNCE: &'static [&'static str] = &[
        "\u{2801}", "\u{2802}", "\u{2804}", "\u{2840}", "\u{2880}", "\u{2820}", "\u{2810}",
        "\u{2808}",
    ];

    /// Arc spinner
    pub const ARC: &'static [&'static str] = &["\u{25DC}", "\u{25DD}", "\u{25DE}", "\u{25DF}"];

    /// Circle quarters
    pub const CIRCLE: &'static [&'static str] = &["\u{25F4}", "\u{25F5}", "\u{25F6}", "\u{25F7}"];

    /// Get frames for spinner type
    pub fn for_type(spinner_type: SpinnerType) -> &'static [&'static str] {
        match spinner_type {
            SpinnerType::Thinking => Self::CIRCLE,
            SpinnerType::Tool => Self::DOTS,
            SpinnerType::Streaming => Self::BLOCKS,
            SpinnerType::Approval => Self::ARC,
            SpinnerType::Loading => Self::DOTS,
            SpinnerType::Progress => Self::LINE,
        }
    }

    /// Get interval in ms for spinner type
    pub fn interval_for_type(spinner_type: SpinnerType) -> u64 {
        match spinner_type {
            SpinnerType::Thinking => 150,  // Slow, contemplative
            SpinnerType::Tool => 80,       // Fast, active
            SpinnerType::Streaming => 100, // Medium, flowing
            SpinnerType::Approval => 200,  // Slow pulse
            SpinnerType::Loading => 100,
            SpinnerType::Progress => 120,
        }
    }
}
