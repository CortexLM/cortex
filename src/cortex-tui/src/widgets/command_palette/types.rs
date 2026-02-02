//! Command palette types.
//!
//! Types for items that can appear in the command palette.

use crate::commands::CommandCategory;

// ============================================================
// RECENT TYPE
// ============================================================

/// Type of recent item for categorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecentType {
    /// A recently used command
    Command,
    /// A recently opened file
    File,
    /// A recently accessed session
    Session,
}

// ============================================================
// PALETTE ITEM
// ============================================================

/// An item that can appear in the command palette.
#[derive(Debug, Clone)]
pub enum PaletteItem {
    /// A slash command
    Command {
        /// Command name (without /)
        name: String,
        /// Short description
        description: String,
        /// Keyboard shortcut, if any
        shortcut: Option<String>,
        /// Command category
        category: CommandCategory,
    },
    /// A file path
    File {
        /// File path
        path: String,
        /// Optional line number to jump to
        line: Option<usize>,
    },
    /// A session
    Session {
        /// Session ID
        id: String,
        /// Session title
        title: String,
        /// Relative time (e.g., "2h ago")
        relative_time: String,
    },
    /// A recent item
    Recent {
        /// Display text
        text: String,
        /// Type of recent item
        item_type: RecentType,
    },
}

impl PaletteItem {
    /// Returns the main display text for this item.
    pub fn display_text(&self) -> &str {
        match self {
            PaletteItem::Command { name, .. } => name,
            PaletteItem::File { path, .. } => path,
            PaletteItem::Session { title, .. } => title,
            PaletteItem::Recent { text, .. } => text,
        }
    }

    /// Returns optional detail/description text.
    pub fn detail_text(&self) -> Option<&str> {
        match self {
            PaletteItem::Command { description, .. } => Some(description),
            PaletteItem::File { .. } => None,
            PaletteItem::Session { relative_time, .. } => Some(relative_time),
            PaletteItem::Recent { .. } => None,
        }
    }

    /// Returns the keyboard shortcut if any.
    pub fn shortcut(&self) -> Option<&str> {
        match self {
            PaletteItem::Command { shortcut, .. } => shortcut.as_deref(),
            _ => None,
        }
    }

    /// Returns the category name for grouping.
    pub fn category_name(&self) -> &str {
        match self {
            PaletteItem::Command { category, .. } => category.name(),
            PaletteItem::File { .. } => "Files",
            PaletteItem::Session { .. } => "Sessions",
            PaletteItem::Recent { .. } => "Recent",
        }
    }

    /// Returns the display prefix for this item type.
    pub fn prefix(&self) -> &str {
        match self {
            PaletteItem::Command { .. } => "/",
            PaletteItem::File { .. } => "",
            PaletteItem::Session { .. } => "[s]",
            PaletteItem::Recent { item_type, .. } => match item_type {
                RecentType::Command => "/",
                RecentType::File => "",
                RecentType::Session => "[s]",
            },
        }
    }

    /// Returns a sort key for ordering items (lower = higher priority).
    pub fn sort_key(&self) -> u8 {
        match self {
            PaletteItem::Recent { .. } => 0,
            PaletteItem::Command { .. } => 1,
            PaletteItem::Session { .. } => 2,
            PaletteItem::File { .. } => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_palette_item_display_text() {
        let cmd = PaletteItem::Command {
            name: "help".to_string(),
            description: "Show help".to_string(),
            shortcut: Some("[?]".to_string()),
            category: CommandCategory::General,
        };
        assert_eq!(cmd.display_text(), "help");

        let file = PaletteItem::File {
            path: "src/main.rs".to_string(),
            line: Some(42),
        };
        assert_eq!(file.display_text(), "src/main.rs");

        let session = PaletteItem::Session {
            id: "abc123".to_string(),
            title: "Bug fix".to_string(),
            relative_time: "2h ago".to_string(),
        };
        assert_eq!(session.display_text(), "Bug fix");
    }

    #[test]
    fn test_palette_item_prefix() {
        let cmd = PaletteItem::Command {
            name: "help".to_string(),
            description: "Show help".to_string(),
            shortcut: None,
            category: CommandCategory::General,
        };
        assert_eq!(cmd.prefix(), "/");

        let file = PaletteItem::File {
            path: "test.rs".to_string(),
            line: None,
        };
        assert_eq!(file.prefix(), "");

        let session = PaletteItem::Session {
            id: "123".to_string(),
            title: "Test".to_string(),
            relative_time: "1h ago".to_string(),
        };
        assert_eq!(session.prefix(), "[s]");

        let recent_cmd = PaletteItem::Recent {
            text: "model".to_string(),
            item_type: RecentType::Command,
        };
        assert_eq!(recent_cmd.prefix(), "/");

        let recent_file = PaletteItem::Recent {
            text: "main.rs".to_string(),
            item_type: RecentType::File,
        };
        assert_eq!(recent_file.prefix(), "");

        let recent_session = PaletteItem::Recent {
            text: "Session".to_string(),
            item_type: RecentType::Session,
        };
        assert_eq!(recent_session.prefix(), "[s]");
    }

    #[test]
    fn test_palette_item_category_name() {
        let cmd = PaletteItem::Command {
            name: "help".to_string(),
            description: "Show help".to_string(),
            shortcut: None,
            category: CommandCategory::General,
        };
        assert_eq!(cmd.category_name(), "General");

        let file = PaletteItem::File {
            path: "test.rs".to_string(),
            line: None,
        };
        assert_eq!(file.category_name(), "Files");

        let session = PaletteItem::Session {
            id: "123".to_string(),
            title: "Test".to_string(),
            relative_time: "1h ago".to_string(),
        };
        assert_eq!(session.category_name(), "Sessions");
    }
}
