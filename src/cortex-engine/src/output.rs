//! Output formatting and rendering.
//!
//! Provides utilities for formatting output to various formats
//! including terminal, JSON, YAML, and markdown.

use std::io::Write;

use serde::{Deserialize, Serialize};

/// Output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum OutputFormat {
    /// Plain text.
    #[default]
    Text,
    /// JSON.
    Json,
    /// Pretty-printed JSON.
    JsonPretty,
    /// YAML.
    Yaml,
    /// Markdown.
    Markdown,
    /// Table format.
    Table,
    /// CSV.
    Csv,
}

impl std::str::FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" | "plain" => Ok(Self::Text),
            "json" => Ok(Self::Json),
            "json-pretty" | "jsonpretty" => Ok(Self::JsonPretty),
            "yaml" | "yml" => Ok(Self::Yaml),
            "markdown" | "md" => Ok(Self::Markdown),
            "table" => Ok(Self::Table),
            "csv" => Ok(Self::Csv),
            _ => Err(format!("Unknown output format: {s}")),
        }
    }
}

/// Output options.
#[derive(Debug, Clone, Default)]
pub struct OutputOptions {
    /// Format.
    pub format: OutputFormat,
    /// Enable colors.
    pub colors: bool,
    /// Quiet mode (minimal output).
    pub quiet: bool,
    /// Verbose mode.
    pub verbose: bool,
    /// Show headers in table.
    pub headers: bool,
    /// Maximum width.
    pub max_width: Option<usize>,
}

impl OutputOptions {
    /// Create new options.
    pub fn new() -> Self {
        Self {
            format: OutputFormat::Text,
            colors: true,
            quiet: false,
            verbose: false,
            headers: true,
            max_width: None,
        }
    }

    /// Set format.
    pub fn format(mut self, format: OutputFormat) -> Self {
        self.format = format;
        self
    }

    /// Enable/disable colors.
    pub fn colors(mut self, enabled: bool) -> Self {
        self.colors = enabled;
        self
    }

    /// Set quiet mode.
    pub fn quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    /// Set verbose mode.
    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }
}

/// Output writer.
pub struct OutputWriter<W: Write> {
    /// Inner writer.
    writer: W,
    /// Options.
    options: OutputOptions,
}

impl<W: Write> OutputWriter<W> {
    /// Create a new writer.
    pub fn new(writer: W, options: OutputOptions) -> Self {
        Self { writer, options }
    }

    /// Write a line.
    pub fn writeln(&mut self, text: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{text}")
    }

    /// Write without newline.
    pub fn write(&mut self, text: &str) -> std::io::Result<()> {
        write!(self.writer, "{text}")
    }

    /// Write a value in the configured format.
    pub fn write_value<T: Serialize>(&mut self, value: &T) -> std::io::Result<()> {
        let output = match self.options.format {
            OutputFormat::Text => {
                serde_json::to_string(value).unwrap_or_else(|_| "Error".to_string())
            }
            OutputFormat::Json => serde_json::to_string(value).unwrap_or_else(|_| "{}".to_string()),
            OutputFormat::JsonPretty => {
                serde_json::to_string_pretty(value).unwrap_or_else(|_| "{}".to_string())
            }
            OutputFormat::Yaml => serde_yaml::to_string(value).unwrap_or_else(|_| "".to_string()),
            _ => serde_json::to_string(value).unwrap_or_else(|_| "Error".to_string()),
        };

        self.writeln(&output)
    }

    /// Write with color.
    pub fn write_colored(&mut self, text: &str, color: Color) -> std::io::Result<()> {
        if self.options.colors {
            write!(self.writer, "{}{}\x1b[0m", color.code(), text)
        } else {
            write!(self.writer, "{text}")
        }
    }

    /// Flush.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }
}

/// Color for terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    /// Black.
    Black,
    /// Red.
    Red,
    /// Green.
    Green,
    /// Yellow.
    Yellow,
    /// Blue.
    Blue,
    /// Magenta.
    Magenta,
    /// Cyan.
    Cyan,
    /// White.
    White,
    /// Reset.
    Reset,
}

impl Color {
    /// Get ANSI code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Black => "\x1b[30m",
            Self::Red => "\x1b[31m",
            Self::Green => "\x1b[32m",
            Self::Yellow => "\x1b[33m",
            Self::Blue => "\x1b[34m",
            Self::Magenta => "\x1b[35m",
            Self::Cyan => "\x1b[36m",
            Self::White => "\x1b[37m",
            Self::Reset => "\x1b[0m",
        }
    }
}

/// Table cell alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Align {
    /// Left align.
    #[default]
    Left,
    /// Center align.
    Center,
    /// Right align.
    Right,
}

impl Align {
    /// Pad text to width.
    pub fn pad(&self, text: &str, width: usize) -> String {
        if text.len() >= width {
            return text[..width].to_string();
        }

        let padding = width - text.len();
        match self {
            Self::Left => format!("{}{}", text, " ".repeat(padding)),
            Self::Right => format!("{}{}", " ".repeat(padding), text),
            Self::Center => {
                let left = padding / 2;
                let right = padding - left;
                format!("{}{}{}", " ".repeat(left), text, " ".repeat(right))
            }
        }
    }
}

/// Table builder.
#[derive(Debug, Clone)]
pub struct TableBuilder {
    /// Headers.
    headers: Vec<String>,
    /// Rows.
    rows: Vec<Vec<String>>,
    /// Column alignments.
    alignments: Vec<Align>,
    /// Column widths.
    widths: Vec<usize>,
    /// Border style.
    border: BorderStyle,
}

impl TableBuilder {
    /// Create a new table.
    pub fn new() -> Self {
        Self {
            headers: Vec::new(),
            rows: Vec::new(),
            alignments: Vec::new(),
            widths: Vec::new(),
            border: BorderStyle::default(),
        }
    }

    /// Set headers.
    pub fn headers(mut self, headers: Vec<impl Into<String>>) -> Self {
        self.headers = headers.into_iter().map(std::convert::Into::into).collect();
        self.widths = self.headers.iter().map(std::string::String::len).collect();
        self.alignments = vec![Align::Left; self.headers.len()];
        self
    }

    /// Add a row.
    pub fn row(mut self, row: Vec<impl Into<String>>) -> Self {
        let row: Vec<String> = row.into_iter().map(std::convert::Into::into).collect();

        // Update widths
        for (i, cell) in row.iter().enumerate() {
            if i < self.widths.len() {
                self.widths[i] = self.widths[i].max(cell.len());
            } else {
                self.widths.push(cell.len());
            }
        }

        self.rows.push(row);
        self
    }

    /// Set alignment for a column.
    pub fn align(mut self, col: usize, align: Align) -> Self {
        if col < self.alignments.len() {
            self.alignments[col] = align;
        }
        self
    }

    /// Set border style.
    pub fn border(mut self, style: BorderStyle) -> Self {
        self.border = style;
        self
    }

    /// Build the table.
    pub fn build(&self) -> String {
        let mut output = String::new();

        // Header
        if !self.headers.is_empty() {
            output.push_str(&self.format_row(&self.headers));
            output.push('\n');
            output.push_str(&self.format_separator());
            output.push('\n');
        }

        // Rows
        for row in &self.rows {
            output.push_str(&self.format_row(row));
            output.push('\n');
        }

        output
    }

    /// Format a row.
    fn format_row(&self, row: &[String]) -> String {
        let cells: Vec<String> = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let width = self.widths.get(i).copied().unwrap_or(cell.len());
                let align = self.alignments.get(i).copied().unwrap_or(Align::Left);
                align.pad(cell, width)
            })
            .collect();

        match self.border {
            BorderStyle::None => cells.join(" "),
            BorderStyle::Simple => cells.join(" | "),
            BorderStyle::Ascii => format!("| {} |", cells.join(" | ")),
            BorderStyle::Unicode => format!("│ {} │", cells.join(" │ ")),
        }
    }

    /// Format separator.
    fn format_separator(&self) -> String {
        match self.border {
            BorderStyle::None | BorderStyle::Simple => self
                .widths
                .iter()
                .map(|w| "-".repeat(*w))
                .collect::<Vec<_>>()
                .join("-+-"),
            BorderStyle::Ascii => {
                let sep = self
                    .widths
                    .iter()
                    .map(|w| "-".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("-+-");
                format!("+-{sep}-+")
            }
            BorderStyle::Unicode => {
                let sep = self
                    .widths
                    .iter()
                    .map(|w| "─".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("─┼─");
                format!("├─{sep}─┤")
            }
        }
    }
}

impl Default for TableBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Border style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BorderStyle {
    /// No border.
    None,
    /// Simple border.
    #[default]
    Simple,
    /// ASCII border.
    Ascii,
    /// Unicode border.
    Unicode,
}

/// CSV builder.
#[derive(Debug, Clone)]
pub struct CsvBuilder {
    /// Headers.
    headers: Vec<String>,
    /// Rows.
    rows: Vec<Vec<String>>,
    /// Delimiter.
    delimiter: char,
    /// Quote character.
    quote: char,
}

impl CsvBuilder {
    /// Create a new CSV builder.
    pub fn new() -> Self {
        Self {
            headers: Vec::new(),
            rows: Vec::new(),
            delimiter: ',',
            quote: '"',
        }
    }

    /// Set headers.
    pub fn headers(mut self, headers: Vec<impl Into<String>>) -> Self {
        self.headers = headers.into_iter().map(std::convert::Into::into).collect();
        self
    }

    /// Add a row.
    pub fn row(mut self, row: Vec<impl Into<String>>) -> Self {
        self.rows
            .push(row.into_iter().map(std::convert::Into::into).collect());
        self
    }

    /// Set delimiter.
    pub fn delimiter(mut self, delim: char) -> Self {
        self.delimiter = delim;
        self
    }

    /// Build CSV string.
    pub fn build(&self) -> String {
        let mut output = String::new();

        if !self.headers.is_empty() {
            output.push_str(&self.format_row(&self.headers));
            output.push('\n');
        }

        for row in &self.rows {
            output.push_str(&self.format_row(row));
            output.push('\n');
        }

        output
    }

    /// Format a row.
    fn format_row(&self, row: &[String]) -> String {
        row.iter()
            .map(|cell| self.escape_cell(cell))
            .collect::<Vec<_>>()
            .join(&self.delimiter.to_string())
    }

    /// Escape a cell value.
    fn escape_cell(&self, cell: &str) -> String {
        if cell.contains(self.delimiter) || cell.contains(self.quote) || cell.contains('\n') {
            let escaped = cell.replace(self.quote, &format!("{}{}", self.quote, self.quote));
            format!("{}{}{}", self.quote, escaped, self.quote)
        } else {
            cell.to_string()
        }
    }
}

impl Default for CsvBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Progress indicator.
#[derive(Debug, Clone)]
pub struct Progress {
    /// Total items.
    total: u64,
    /// Current item.
    current: u64,
    /// Width.
    width: usize,
    /// Message.
    message: String,
    /// Show percentage.
    show_percent: bool,
    /// Show count.
    show_count: bool,
}

impl Progress {
    /// Create a new progress indicator.
    pub fn new(total: u64) -> Self {
        Self {
            total,
            current: 0,
            width: 40,
            message: String::new(),
            show_percent: true,
            show_count: true,
        }
    }

    /// Set current value.
    pub fn set(&mut self, current: u64) {
        self.current = current.min(self.total);
    }

    /// Increment.
    pub fn inc(&mut self) {
        self.set(self.current + 1);
    }

    /// Set message.
    pub fn message(&mut self, msg: impl Into<String>) {
        self.message = msg.into();
    }

    /// Get ratio (0.0 - 1.0).
    pub fn ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.current as f64 / self.total as f64
        }
    }

    /// Render progress bar.
    pub fn render(&self) -> String {
        let ratio = self.ratio();
        let filled = (ratio * self.width as f64).round() as usize;
        let empty = self.width - filled;

        let bar = format!("[{}{}]", "█".repeat(filled), "░".repeat(empty));

        let mut parts = vec![bar];

        if self.show_percent {
            parts.push(format!("{:>3}%", (ratio * 100.0).round() as u32));
        }

        if self.show_count {
            parts.push(format!("{}/{}", self.current, self.total));
        }

        if !self.message.is_empty() {
            parts.push(self.message.clone());
        }

        parts.join(" ")
    }
}

/// Result formatter.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ResultFormatter {
    /// Show success messages.
    show_success: bool,
    /// Show errors.
    show_errors: bool,
    /// Verbose.
    verbose: bool,
}

impl ResultFormatter {
    /// Create a new formatter.
    pub fn new() -> Self {
        Self {
            show_success: true,
            show_errors: true,
            verbose: false,
        }
    }

    /// Format a success result.
    pub fn success(&self, message: &str) -> String {
        if self.show_success {
            format!("[OK] {message}")
        } else {
            String::new()
        }
    }

    /// Format an error result.
    pub fn error(&self, message: &str) -> String {
        if self.show_errors {
            format!("[ERROR] {message}")
        } else {
            String::new()
        }
    }

    /// Format a warning.
    pub fn warning(&self, message: &str) -> String {
        format!("[WARN] {message}")
    }

    /// Format info.
    pub fn info(&self, message: &str) -> String {
        format!("[INFO] {message}")
    }
}

impl Default for ResultFormatter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_output_format_parse() {
        assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
        assert_eq!("yaml".parse::<OutputFormat>().unwrap(), OutputFormat::Yaml);
        assert_eq!("text".parse::<OutputFormat>().unwrap(), OutputFormat::Text);
    }

    #[test]
    fn test_align() {
        assert_eq!(Align::Left.pad("Hi", 5), "Hi   ");
        assert_eq!(Align::Right.pad("Hi", 5), "   Hi");
        assert_eq!(Align::Center.pad("Hi", 6), "  Hi  ");
    }

    #[test]
    fn test_table_builder() {
        let table = TableBuilder::new()
            .headers(vec!["Name", "Age"])
            .row(vec!["Alice", "30"])
            .row(vec!["Bob", "25"])
            .build();

        assert!(table.contains("Name"));
        assert!(table.contains("Alice"));
        assert!(table.contains("30"));
    }

    #[test]
    fn test_csv_builder() {
        let csv = CsvBuilder::new()
            .headers(vec!["Name", "Age"])
            .row(vec!["Alice", "30"])
            .row(vec!["Bob", "25"])
            .build();

        assert!(csv.contains("Name,Age"));
        assert!(csv.contains("Alice,30"));
    }

    #[test]
    fn test_csv_escape() {
        let csv = CsvBuilder::new().row(vec!["Hello, World", "Test"]).build();

        assert!(csv.contains("\"Hello, World\""));
    }

    #[test]
    fn test_progress() {
        let mut progress = Progress::new(100);
        progress.set(50);

        let rendered = progress.render();
        assert!(rendered.contains("50%"));
        assert!(rendered.contains("50/100"));
    }

    #[test]
    fn test_result_formatter() {
        let formatter = ResultFormatter::new();

        assert!(formatter.success("Done").contains("[OK]"));
        assert!(formatter.error("Failed").contains("[ERROR]"));
        assert!(formatter.warning("Warn").contains("[WARN]"));
    }

    #[test]
    fn test_color() {
        assert_eq!(Color::Red.code(), "\x1b[31m");
        assert_eq!(Color::Green.code(), "\x1b[32m");
        assert_eq!(Color::Reset.code(), "\x1b[0m");
    }
}
