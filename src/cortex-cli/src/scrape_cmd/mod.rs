//! Web scraping command for Cortex CLI.
//!
//! Scrapes web content and converts it to clean markdown for AI consumption.

mod command;
mod html;
mod http;
#[cfg(test)]
mod tests;
mod types;
mod url;
mod xml;

pub use command::ScrapeCommand;
pub use types::OutputFormat;
