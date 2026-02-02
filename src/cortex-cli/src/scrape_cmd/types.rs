//! Type definitions for the scrape command.

use anyhow::{Result, bail};

/// Maximum allowed header value length (8KB per HTTP spec recommendations).
pub const MAX_HEADER_VALUE_LENGTH: usize = 8192;

/// Output format for scraped content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    /// Clean markdown with headers, lists, links.
    #[default]
    Markdown,
    /// Plain text, no formatting.
    Text,
    /// Cleaned HTML.
    Html,
}

impl std::str::FromStr for OutputFormat {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "markdown" | "md" => Ok(Self::Markdown),
            "text" | "txt" | "plain" => Ok(Self::Text),
            "html" => Ok(Self::Html),
            _ => bail!("Invalid format: {s}. Use markdown, text, or html"),
        }
    }
}
