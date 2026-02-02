//! Collaboration Modes for TUI
//!
//! Supports different interaction modes for the AI agent:
//! - **Plan**: Read-only analysis and planning mode
//! - **Code**: Default coding mode with full access
//! - **Pair**: Interactive pair programming mode
//! - **Execute**: Direct execution mode

use serde::{Deserialize, Serialize};

/// Kind of collaboration mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModeKind {
    /// Plan mode - read-only analysis and planning.
    Plan,

    /// Code mode - default, full coding access.
    #[default]
    Code,

    /// Pair programming mode - interactive collaboration.
    Pair,

    /// Execute mode - direct execution with minimal interaction.
    Execute,
}

impl ModeKind {
    /// Get the display name.
    pub fn name(&self) -> &str {
        match self {
            ModeKind::Plan => "Plan",
            ModeKind::Code => "Code",
            ModeKind::Pair => "Pair",
            ModeKind::Execute => "Execute",
        }
    }

    /// Get a short description.
    pub fn description(&self) -> &str {
        match self {
            ModeKind::Plan => "Read-only analysis and planning",
            ModeKind::Code => "Full coding access (default)",
            ModeKind::Pair => "Interactive pair programming",
            ModeKind::Execute => "Direct execution mode",
        }
    }

    /// Get the icon/symbol for this mode.
    pub fn icon(&self) -> &str {
        match self {
            ModeKind::Plan => "[P]",
            ModeKind::Code => "[C]",
            ModeKind::Pair => "[2]",
            ModeKind::Execute => "[E]",
        }
    }

    /// Check if this mode allows code modifications.
    pub fn allows_write(&self) -> bool {
        matches!(self, ModeKind::Code | ModeKind::Pair | ModeKind::Execute)
    }

    /// Check if this mode is read-only.
    pub fn is_read_only(&self) -> bool {
        matches!(self, ModeKind::Plan)
    }

    /// Get the system prompt modifier for this mode.
    pub fn system_prompt_modifier(&self) -> &str {
        match self {
            ModeKind::Plan => {
                "\n\n[PLAN MODE]\nYou are in planning mode. Focus on analysis and planning.\n\
                 - Explore and understand the codebase\n\
                 - Create detailed plans and specifications\n\
                 - DO NOT modify any files\n\
                 - DO NOT execute destructive commands"
            }
            ModeKind::Code => "", // Default mode, no modifier
            ModeKind::Pair => {
                "\n\n[PAIR MODE]\nYou are in pair programming mode.\n\
                 - Explain your reasoning as you work\n\
                 - Ask for feedback on significant decisions\n\
                 - Suggest multiple approaches when appropriate\n\
                 - Work incrementally with user validation"
            }
            ModeKind::Execute => {
                "\n\n[EXECUTE MODE]\nYou are in execution mode.\n\
                 - Focus on completing the task efficiently\n\
                 - Minimize back-and-forth questions\n\
                 - Make reasonable assumptions\n\
                 - Execute commands directly when safe"
            }
        }
    }

    /// Get all available modes.
    pub fn all() -> &'static [ModeKind] {
        &[
            ModeKind::Plan,
            ModeKind::Code,
            ModeKind::Pair,
            ModeKind::Execute,
        ]
    }

    /// Get modes suitable for TUI display.
    pub fn tui_modes() -> &'static [ModeKind] {
        &[ModeKind::Plan, ModeKind::Code]
    }
}

impl std::fmt::Display for ModeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl std::str::FromStr for ModeKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "plan" => Ok(ModeKind::Plan),
            "code" => Ok(ModeKind::Code),
            "pair" => Ok(ModeKind::Pair),
            "execute" | "exec" => Ok(ModeKind::Execute),
            _ => Err(format!("Unknown mode: {}", s)),
        }
    }
}

/// Configuration mask for a collaboration mode.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CollaborationModeMask {
    /// The mode kind.
    pub mode: Option<ModeKind>,

    /// Override for temperature.
    pub temperature: Option<f32>,

    /// Override for max tokens.
    pub max_tokens: Option<u32>,

    /// Custom tools configuration.
    pub tools_override: Option<Vec<String>>,
}

impl CollaborationModeMask {
    /// Create a mask for a specific mode.
    pub fn for_mode(mode: ModeKind) -> Self {
        Self {
            mode: Some(mode),
            ..Default::default()
        }
    }

    /// Create with temperature override.
    pub fn with_temperature(mut self, temp: f32) -> Self {
        self.temperature = Some(temp);
        self
    }

    /// Create with max tokens override.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.max_tokens = Some(tokens);
        self
    }

    /// Get the effective mode kind.
    pub fn effective_mode(&self) -> ModeKind {
        self.mode.unwrap_or_default()
    }
}

/// Get presets filtered for TUI (only Plan and Code modes).
pub fn presets_for_tui() -> Vec<CollaborationModeMask> {
    ModeKind::tui_modes()
        .iter()
        .map(|&mode| CollaborationModeMask::for_mode(mode))
        .collect()
}

/// Cycle to the next collaboration mode preset.
pub fn next_mode(current: Option<ModeKind>) -> ModeKind {
    let modes = ModeKind::tui_modes();
    let current_idx = current
        .and_then(|c| modes.iter().position(|&m| m == c))
        .unwrap_or(0);
    let next_idx = (current_idx + 1) % modes.len();
    modes[next_idx]
}

/// Cycle to the previous collaboration mode preset.
pub fn prev_mode(current: Option<ModeKind>) -> ModeKind {
    let modes = ModeKind::tui_modes();
    let current_idx = current
        .and_then(|c| modes.iter().position(|&m| m == c))
        .unwrap_or(0);
    let prev_idx = if current_idx == 0 {
        modes.len() - 1
    } else {
        current_idx - 1
    };
    modes[prev_idx]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_properties() {
        assert!(ModeKind::Plan.is_read_only());
        assert!(!ModeKind::Plan.allows_write());

        assert!(!ModeKind::Code.is_read_only());
        assert!(ModeKind::Code.allows_write());

        assert!(ModeKind::Execute.allows_write());
    }

    #[test]
    fn test_mode_cycling() {
        assert_eq!(next_mode(Some(ModeKind::Plan)), ModeKind::Code);
        assert_eq!(next_mode(Some(ModeKind::Code)), ModeKind::Plan);
        assert_eq!(next_mode(None), ModeKind::Code);
    }

    #[test]
    fn test_mode_from_str() {
        assert_eq!("plan".parse::<ModeKind>().unwrap(), ModeKind::Plan);
        assert_eq!("code".parse::<ModeKind>().unwrap(), ModeKind::Code);
        assert_eq!("exec".parse::<ModeKind>().unwrap(), ModeKind::Execute);
    }
}
