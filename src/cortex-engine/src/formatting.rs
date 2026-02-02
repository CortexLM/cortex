//! Text formatting utilities.
//!
//! Provides utilities for formatting text output including
//! markdown rendering, syntax highlighting, and terminal output.

use serde::{Deserialize, Serialize};

/// Text style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Underline.
    pub underline: bool,
    /// Strikethrough.
    pub strikethrough: bool,
    /// Foreground color.
    pub fg: Option<Color>,
    /// Background color.
    pub bg: Option<Color>,
}

impl Style {
    /// Create a new style.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set bold.
    pub fn bold(mut self) -> Self {
        self.bold = true;
        self
    }

    /// Set italic.
    pub fn italic(mut self) -> Self {
        self.italic = true;
        self
    }

    /// Set underline.
    pub fn underline(mut self) -> Self {
        self.underline = true;
        self
    }

    /// Set foreground color.
    pub fn fg(mut self, color: Color) -> Self {
        self.fg = Some(color);
        self
    }

    /// Set background color.
    pub fn bg(mut self, color: Color) -> Self {
        self.bg = Some(color);
        self
    }

    /// Get ANSI escape codes.
    pub fn ansi_codes(&self) -> String {
        let mut codes = Vec::new();

        if self.bold {
            codes.push("1");
        }
        if self.italic {
            codes.push("3");
        }
        if self.underline {
            codes.push("4");
        }
        if self.strikethrough {
            codes.push("9");
        }
        if let Some(color) = &self.fg {
            codes.push(color.fg_code());
        }
        if let Some(color) = &self.bg {
            codes.push(color.bg_code());
        }

        if codes.is_empty() {
            String::new()
        } else {
            format!("\x1b[{}m", codes.join(";"))
        }
    }

    /// Reset ANSI codes.
    pub fn reset_codes() -> &'static str {
        "\x1b[0m"
    }
}

/// Color.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
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
    /// Bright black (gray).
    BrightBlack,
    /// Bright red.
    BrightRed,
    /// Bright green.
    BrightGreen,
    /// Bright yellow.
    BrightYellow,
    /// Bright blue.
    BrightBlue,
    /// Bright magenta.
    BrightMagenta,
    /// Bright cyan.
    BrightCyan,
    /// Bright white.
    BrightWhite,
    /// RGB color.
    Rgb(u8, u8, u8),
    /// 256-color palette.
    Indexed(u8),
}

impl Color {
    /// Get foreground ANSI code.
    pub fn fg_code(&self) -> &'static str {
        match self {
            Self::Black => "30",
            Self::Red => "31",
            Self::Green => "32",
            Self::Yellow => "33",
            Self::Blue => "34",
            Self::Magenta => "35",
            Self::Cyan => "36",
            Self::White => "37",
            Self::BrightBlack => "90",
            Self::BrightRed => "91",
            Self::BrightGreen => "92",
            Self::BrightYellow => "93",
            Self::BrightBlue => "94",
            Self::BrightMagenta => "95",
            Self::BrightCyan => "96",
            Self::BrightWhite => "97",
            Self::Rgb(_, _, _) => "38",
            Self::Indexed(_) => "38",
        }
    }

    /// Get background ANSI code.
    pub fn bg_code(&self) -> &'static str {
        match self {
            Self::Black => "40",
            Self::Red => "41",
            Self::Green => "42",
            Self::Yellow => "43",
            Self::Blue => "44",
            Self::Magenta => "45",
            Self::Cyan => "46",
            Self::White => "47",
            Self::BrightBlack => "100",
            Self::BrightRed => "101",
            Self::BrightGreen => "102",
            Self::BrightYellow => "103",
            Self::BrightBlue => "104",
            Self::BrightMagenta => "105",
            Self::BrightCyan => "106",
            Self::BrightWhite => "107",
            Self::Rgb(_, _, _) => "48",
            Self::Indexed(_) => "48",
        }
    }
}

/// Styled text span.
#[derive(Debug, Clone)]
pub struct StyledSpan {
    /// Text content.
    pub text: String,
    /// Style.
    pub style: Style,
}

impl StyledSpan {
    /// Create a new span.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: Style::default(),
        }
    }

    /// Create with style.
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }

    /// Render to ANSI string.
    pub fn render(&self) -> String {
        let codes = self.style.ansi_codes();
        if codes.is_empty() {
            self.text.clone()
        } else {
            format!("{}{}{}", codes, self.text, Style::reset_codes())
        }
    }
}

/// Styled line.
#[derive(Debug, Clone, Default)]
pub struct StyledLine {
    /// Spans.
    pub spans: Vec<StyledSpan>,
}

impl StyledLine {
    /// Create a new line.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a span.
    pub fn push(&mut self, span: StyledSpan) {
        self.spans.push(span);
    }

    /// Add plain text.
    pub fn text(&mut self, text: impl Into<String>) {
        self.spans.push(StyledSpan::new(text));
    }

    /// Add styled text.
    pub fn styled(&mut self, text: impl Into<String>, style: Style) {
        self.spans.push(StyledSpan::styled(text, style));
    }

    /// Render to string.
    pub fn render(&self) -> String {
        self.spans.iter().map(StyledSpan::render).collect()
    }

    /// Get plain text.
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// Text wrapper for word wrapping.
pub struct TextWrapper {
    /// Maximum width.
    pub max_width: usize,
    /// Initial indent.
    pub initial_indent: String,
    /// Subsequent indent.
    pub subsequent_indent: String,
    /// Break on hyphens.
    pub break_on_hyphens: bool,
}

impl TextWrapper {
    /// Create a new wrapper.
    pub fn new(max_width: usize) -> Self {
        Self {
            max_width,
            initial_indent: String::new(),
            subsequent_indent: String::new(),
            break_on_hyphens: true,
        }
    }

    /// Set initial indent.
    pub fn initial_indent(mut self, indent: impl Into<String>) -> Self {
        self.initial_indent = indent.into();
        self
    }

    /// Set subsequent indent.
    pub fn subsequent_indent(mut self, indent: impl Into<String>) -> Self {
        self.subsequent_indent = indent.into();
        self
    }

    /// Wrap text.
    pub fn wrap(&self, text: &str) -> Vec<String> {
        let mut lines = Vec::new();
        let mut current_line = self.initial_indent.clone();
        let mut first_line = true;

        for word in text.split_whitespace() {
            let space = if current_line.ends_with(' ') || current_line.is_empty() {
                ""
            } else {
                " "
            };

            let potential_len = current_line.len() + space.len() + word.len();

            if potential_len > self.max_width && !current_line.is_empty() {
                lines.push(current_line);
                current_line = if first_line {
                    first_line = false;
                    self.subsequent_indent.clone()
                } else {
                    self.subsequent_indent.clone()
                };
                current_line.push_str(word);
            } else {
                current_line.push_str(space);
                current_line.push_str(word);
            }
        }

        if !current_line.is_empty() {
            lines.push(current_line);
        }

        lines
    }

    /// Wrap and join with newlines.
    pub fn fill(&self, text: &str) -> String {
        self.wrap(text).join("\n")
    }
}

impl Default for TextWrapper {
    fn default() -> Self {
        Self::new(80)
    }
}

/// Table formatter.
pub struct TableFormatter {
    /// Column headers.
    pub headers: Vec<String>,
    /// Rows.
    pub rows: Vec<Vec<String>>,
    /// Column alignments.
    pub alignments: Vec<Alignment>,
    /// Column widths.
    widths: Vec<usize>,
}

impl TableFormatter {
    /// Create a new table.
    pub fn new(headers: Vec<impl Into<String>>) -> Self {
        let headers: Vec<String> = headers.into_iter().map(std::convert::Into::into).collect();
        let widths = headers.iter().map(std::string::String::len).collect();
        let alignments = vec![Alignment::Left; headers.len()];

        Self {
            headers,
            rows: Vec::new(),
            alignments,
            widths,
        }
    }

    /// Add a row.
    pub fn add_row(&mut self, row: Vec<impl Into<String>>) {
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
    }

    /// Set alignment for a column.
    pub fn set_alignment(&mut self, col: usize, alignment: Alignment) {
        if col < self.alignments.len() {
            self.alignments[col] = alignment;
        }
    }

    /// Format as plain text.
    pub fn format(&self) -> String {
        let mut output = String::new();

        // Header
        output.push_str(&self.format_row(&self.headers));
        output.push('\n');

        // Separator
        for (i, width) in self.widths.iter().enumerate() {
            if i > 0 {
                output.push_str(" | ");
            }
            output.push_str(&"-".repeat(*width));
        }
        output.push('\n');

        // Rows
        for row in &self.rows {
            output.push_str(&self.format_row(row));
            output.push('\n');
        }

        output
    }

    /// Format a single row.
    fn format_row(&self, row: &[String]) -> String {
        let mut cells = Vec::new();

        for (i, cell) in row.iter().enumerate() {
            let width = self.widths.get(i).copied().unwrap_or(cell.len());
            let alignment = self.alignments.get(i).copied().unwrap_or(Alignment::Left);
            cells.push(alignment.pad(cell, width));
        }

        cells.join(" | ")
    }

    /// Format as markdown.
    pub fn format_markdown(&self) -> String {
        let mut output = String::new();

        // Header
        output.push_str("| ");
        output.push_str(&self.headers.join(" | "));
        output.push_str(" |\n");

        // Separator
        output.push('|');
        for (i, width) in self.widths.iter().enumerate() {
            let alignment = self.alignments.get(i).copied().unwrap_or(Alignment::Left);
            output.push_str(&alignment.markdown_separator(*width));
            output.push('|');
        }
        output.push('\n');

        // Rows
        for row in &self.rows {
            output.push_str("| ");
            output.push_str(&row.join(" | "));
            output.push_str(" |\n");
        }

        output
    }
}

/// Text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Alignment {
    /// Left aligned.
    #[default]
    Left,
    /// Center aligned.
    Center,
    /// Right aligned.
    Right,
}

impl Alignment {
    /// Pad text to width.
    pub fn pad(&self, text: &str, width: usize) -> String {
        if text.len() >= width {
            return text.to_string();
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

    /// Get markdown separator.
    fn markdown_separator(&self, width: usize) -> String {
        let w = width.max(3);
        match self {
            Self::Left => format!(" {}-", "-".repeat(w - 1)),
            Self::Right => format!("-{} ", "-".repeat(w - 1)),
            Self::Center => format!(":{}: ", "-".repeat(w.saturating_sub(2))),
        }
    }
}

/// Progress bar formatter.
pub struct ProgressBar {
    /// Total value.
    pub total: u64,
    /// Current value.
    pub current: u64,
    /// Width in characters.
    pub width: usize,
    /// Fill character.
    pub fill_char: char,
    /// Empty character.
    pub empty_char: char,
    /// Show percentage.
    pub show_percent: bool,
}

impl ProgressBar {
    /// Create a new progress bar.
    pub fn new(total: u64) -> Self {
        Self {
            total,
            current: 0,
            width: 40,
            fill_char: '█',
            empty_char: '░',
            show_percent: true,
        }
    }

    /// Set current value.
    pub fn set(&mut self, current: u64) {
        self.current = current.min(self.total);
    }

    /// Increment.
    pub fn inc(&mut self, amount: u64) {
        self.current = (self.current + amount).min(self.total);
    }

    /// Get progress ratio.
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

        let bar = format!(
            "[{}{}]",
            self.fill_char.to_string().repeat(filled),
            self.empty_char.to_string().repeat(empty)
        );

        if self.show_percent {
            format!("{} {:>3}%", bar, (ratio * 100.0).round() as u32)
        } else {
            bar
        }
    }
}

/// Spinner for indeterminate progress.
pub struct Spinner {
    /// Current frame.
    frame: usize,
    /// Frames.
    frames: Vec<&'static str>,
    /// Message.
    message: String,
}

impl Spinner {
    /// Create a new spinner.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            frame: 0,
            frames: vec!["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"],
            message: message.into(),
        }
    }

    /// Create with custom frames.
    pub fn with_frames(frames: Vec<&'static str>, message: impl Into<String>) -> Self {
        Self {
            frame: 0,
            frames,
            message: message.into(),
        }
    }

    /// Advance to next frame.
    pub fn tick(&mut self) {
        self.frame = (self.frame + 1) % self.frames.len();
    }

    /// Render current frame.
    pub fn render(&self) -> String {
        format!("{} {}", self.frames[self.frame], self.message)
    }

    /// Set message.
    pub fn set_message(&mut self, message: impl Into<String>) {
        self.message = message.into();
    }
}

/// Code block formatter.
pub struct CodeBlock {
    /// Language.
    pub language: Option<String>,
    /// Code content.
    pub code: String,
    /// Show line numbers.
    pub line_numbers: bool,
    /// Start line number.
    pub start_line: u32,
}

impl CodeBlock {
    /// Create a new code block.
    pub fn new(code: impl Into<String>) -> Self {
        Self {
            language: None,
            code: code.into(),
            line_numbers: false,
            start_line: 1,
        }
    }

    /// Set language.
    pub fn language(mut self, lang: impl Into<String>) -> Self {
        self.language = Some(lang.into());
        self
    }

    /// Enable line numbers.
    pub fn with_line_numbers(mut self) -> Self {
        self.line_numbers = true;
        self
    }

    /// Set start line.
    pub fn start_at(mut self, line: u32) -> Self {
        self.start_line = line;
        self
    }

    /// Render as plain text.
    pub fn render(&self) -> String {
        if self.line_numbers {
            let lines: Vec<&str> = self.code.lines().collect();
            let width = (self.start_line as usize + lines.len()).to_string().len();

            lines
                .iter()
                .enumerate()
                .map(|(i, line)| format!("{:>width$} | {}", self.start_line as usize + i, line))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            self.code.clone()
        }
    }

    /// Render as markdown.
    pub fn render_markdown(&self) -> String {
        let lang = self.language.as_deref().unwrap_or("");
        format!("```{}\n{}\n```", lang, self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_style() {
        let style = Style::new().bold().fg(Color::Red);
        let codes = style.ansi_codes();
        assert!(codes.contains("1"));
        assert!(codes.contains("31"));
    }

    #[test]
    fn test_styled_span() {
        let span = StyledSpan::styled("Hello", Style::new().bold());
        let rendered = span.render();
        assert!(rendered.contains("Hello"));
        assert!(rendered.contains("\x1b[1m"));
    }

    #[test]
    fn test_text_wrapper() {
        let wrapper = TextWrapper::new(20);
        let lines = wrapper.wrap("This is a longer text that should wrap");
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line.len() <= 20);
        }
    }

    #[test]
    fn test_table() {
        let mut table = TableFormatter::new(vec!["Name", "Age"]);
        table.add_row(vec!["Alice", "30"]);
        table.add_row(vec!["Bob", "25"]);

        let output = table.format();
        assert!(output.contains("Name"));
        assert!(output.contains("Alice"));
    }

    #[test]
    fn test_alignment() {
        assert_eq!(Alignment::Left.pad("Hi", 5), "Hi   ");
        assert_eq!(Alignment::Right.pad("Hi", 5), "   Hi");
        assert_eq!(Alignment::Center.pad("Hi", 6), "  Hi  ");
    }

    #[test]
    fn test_progress_bar() {
        let mut bar = ProgressBar::new(100);
        bar.set(50);

        let rendered = bar.render();
        assert!(rendered.contains("50%"));
    }

    #[test]
    fn test_spinner() {
        let mut spinner = Spinner::new("Loading...");
        let frame1 = spinner.render();
        spinner.tick();
        let frame2 = spinner.render();

        assert!(frame1.contains("Loading..."));
        assert_ne!(frame1, frame2);
    }

    #[test]
    fn test_code_block() {
        let block = CodeBlock::new("fn main() {}")
            .language("rust")
            .with_line_numbers();

        let rendered = block.render();
        assert!(rendered.contains("fn main()"));
        assert!(rendered.contains("1 |"));
    }
}
