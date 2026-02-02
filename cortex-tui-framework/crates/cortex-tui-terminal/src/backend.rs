//! Terminal backend abstraction and crossterm implementation.

use crate::Capabilities;
use cortex_tui_core::{Color, Error, Result, TextAttributes};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{
        DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
        EnableFocusChange, EnableMouseCapture,
    },
    execute, queue,
    style::{
        Attribute, Color as CrosstermColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
    terminal::{
        disable_raw_mode, enable_raw_mode, size, Clear, ClearType, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use std::io::{self, Stdout, Write};

/// Trait for terminal backend implementations.
///
/// This abstraction allows different terminal libraries to be used
/// interchangeably (crossterm, termwiz, etc.).
pub trait TerminalBackend {
    /// Enters raw mode for the terminal.
    fn enter_raw_mode(&mut self) -> Result<()>;

    /// Exits raw mode, restoring normal terminal behavior.
    fn exit_raw_mode(&mut self) -> Result<()>;

    /// Enables mouse capture for receiving mouse events.
    fn enable_mouse_capture(&mut self) -> Result<()>;

    /// Disables mouse capture.
    fn disable_mouse_capture(&mut self) -> Result<()>;

    /// Hides the terminal cursor.
    fn hide_cursor(&mut self) -> Result<()>;

    /// Shows the terminal cursor.
    fn show_cursor(&mut self) -> Result<()>;

    /// Moves the cursor to the specified position (0-based).
    fn move_cursor(&mut self, x: u16, y: u16) -> Result<()>;

    /// Gets the current terminal size (columns, rows).
    fn size(&self) -> Result<(u16, u16)>;

    /// Enters the alternate screen buffer.
    fn enter_alternate_screen(&mut self) -> Result<()>;

    /// Leaves the alternate screen buffer.
    fn leave_alternate_screen(&mut self) -> Result<()>;

    /// Clears the entire screen.
    fn clear(&mut self) -> Result<()>;

    /// Flushes any buffered output to the terminal.
    fn flush(&mut self) -> Result<()>;

    /// Writes raw bytes to the terminal.
    fn write_raw(&mut self, data: &[u8]) -> Result<()>;

    /// Sets the foreground color.
    fn set_foreground(&mut self, color: Color) -> Result<()>;

    /// Sets the background color.
    fn set_background(&mut self, color: Color) -> Result<()>;

    /// Sets text attributes.
    fn set_attributes(&mut self, attrs: TextAttributes) -> Result<()>;

    /// Resets all colors and attributes to default.
    fn reset_style(&mut self) -> Result<()>;

    /// Writes a string at the current cursor position.
    fn write_str(&mut self, s: &str) -> Result<()>;

    /// Enables bracketed paste mode.
    fn enable_bracketed_paste(&mut self) -> Result<()>;

    /// Disables bracketed paste mode.
    fn disable_bracketed_paste(&mut self) -> Result<()>;

    /// Enables focus change events.
    fn enable_focus_change(&mut self) -> Result<()>;

    /// Disables focus change events.
    fn disable_focus_change(&mut self) -> Result<()>;

    /// Begins synchronized output (prevents screen tearing).
    fn begin_sync_update(&mut self) -> Result<()>;

    /// Ends synchronized output.
    fn end_sync_update(&mut self) -> Result<()>;

    /// Sets the terminal cursor style.
    fn set_cursor_style(&mut self, style: CursorStyle, blinking: bool) -> Result<()>;

    /// Sets the terminal cursor color.
    fn set_cursor_color(&mut self, color: Color) -> Result<()>;

    /// Returns terminal capabilities.
    fn capabilities(&self) -> &Capabilities;
}

/// Terminal cursor style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CursorStyle {
    /// Block cursor (█).
    #[default]
    Block,
    /// Line cursor (I-beam) ( | ).
    Line,
    /// Underline cursor ( _ ).
    Underline,
}

/// Crossterm-based terminal backend.
///
/// This is the default backend implementation using the crossterm crate
/// for cross-platform terminal manipulation.
pub struct CrosstermBackend {
    stdout: Stdout,
    capabilities: Capabilities,
    in_raw_mode: bool,
    in_alternate_screen: bool,
    mouse_captured: bool,
    cursor_hidden: bool,
}

impl CrosstermBackend {
    /// Creates a new crossterm backend.
    ///
    /// Automatically detects terminal capabilities.
    pub fn new() -> Result<Self> {
        Ok(Self {
            stdout: io::stdout(),
            capabilities: Capabilities::detect(),
            in_raw_mode: false,
            in_alternate_screen: false,
            mouse_captured: false,
            cursor_hidden: false,
        })
    }

    /// Creates a backend with custom capabilities.
    pub fn with_capabilities(capabilities: Capabilities) -> Result<Self> {
        Ok(Self {
            stdout: io::stdout(),
            capabilities,
            in_raw_mode: false,
            in_alternate_screen: false,
            mouse_captured: false,
            cursor_hidden: false,
        })
    }

    /// Converts a Color to crossterm's Color type.
    #[inline]
    fn to_crossterm_color(color: Color) -> CrosstermColor {
        let (r, g, b) = color.to_rgb_u8();
        CrosstermColor::Rgb { r, g, b }
    }
}

impl TerminalBackend for CrosstermBackend {
    fn enter_raw_mode(&mut self) -> Result<()> {
        if !self.in_raw_mode {
            enable_raw_mode().map_err(Error::Io)?;
            self.in_raw_mode = true;
        }
        Ok(())
    }

    fn exit_raw_mode(&mut self) -> Result<()> {
        if self.in_raw_mode {
            disable_raw_mode().map_err(Error::Io)?;
            self.in_raw_mode = false;
        }
        Ok(())
    }

    fn enable_mouse_capture(&mut self) -> Result<()> {
        if !self.mouse_captured {
            execute!(self.stdout, EnableMouseCapture).map_err(Error::Io)?;
            self.mouse_captured = true;
        }
        Ok(())
    }

    fn disable_mouse_capture(&mut self) -> Result<()> {
        if self.mouse_captured {
            execute!(self.stdout, DisableMouseCapture).map_err(Error::Io)?;
            self.mouse_captured = false;
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<()> {
        if !self.cursor_hidden {
            execute!(self.stdout, Hide).map_err(Error::Io)?;
            self.cursor_hidden = true;
        }
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<()> {
        if self.cursor_hidden {
            execute!(self.stdout, Show).map_err(Error::Io)?;
            self.cursor_hidden = false;
        }
        Ok(())
    }

    fn move_cursor(&mut self, x: u16, y: u16) -> Result<()> {
        queue!(self.stdout, MoveTo(x, y)).map_err(Error::Io)
    }

    fn size(&self) -> Result<(u16, u16)> {
        size().map_err(Error::Io)
    }

    fn enter_alternate_screen(&mut self) -> Result<()> {
        if !self.in_alternate_screen {
            execute!(self.stdout, EnterAlternateScreen).map_err(Error::Io)?;
            self.in_alternate_screen = true;
        }
        Ok(())
    }

    fn leave_alternate_screen(&mut self) -> Result<()> {
        if self.in_alternate_screen {
            execute!(self.stdout, LeaveAlternateScreen).map_err(Error::Io)?;
            self.in_alternate_screen = false;
        }
        Ok(())
    }

    fn clear(&mut self) -> Result<()> {
        execute!(self.stdout, Clear(ClearType::All)).map_err(Error::Io)
    }

    fn flush(&mut self) -> Result<()> {
        self.stdout.flush().map_err(Error::Io)
    }

    fn write_raw(&mut self, data: &[u8]) -> Result<()> {
        self.stdout.write_all(data).map_err(Error::Io)
    }

    fn set_foreground(&mut self, color: Color) -> Result<()> {
        if color.is_transparent() {
            // Use default foreground for transparent
            queue!(self.stdout, SetForegroundColor(CrosstermColor::Reset)).map_err(Error::Io)
        } else {
            queue!(
                self.stdout,
                SetForegroundColor(Self::to_crossterm_color(color))
            )
            .map_err(Error::Io)
        }
    }

    fn set_background(&mut self, color: Color) -> Result<()> {
        if color.is_transparent() {
            // Use default background for transparent
            queue!(self.stdout, SetBackgroundColor(CrosstermColor::Reset)).map_err(Error::Io)
        } else {
            queue!(
                self.stdout,
                SetBackgroundColor(Self::to_crossterm_color(color))
            )
            .map_err(Error::Io)
        }
    }

    fn set_attributes(&mut self, attrs: TextAttributes) -> Result<()> {
        if attrs.contains(TextAttributes::BOLD) {
            queue!(self.stdout, SetAttribute(Attribute::Bold)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::DIM) {
            queue!(self.stdout, SetAttribute(Attribute::Dim)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::ITALIC) {
            queue!(self.stdout, SetAttribute(Attribute::Italic)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::UNDERLINE) {
            queue!(self.stdout, SetAttribute(Attribute::Underlined)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::BLINK) {
            queue!(self.stdout, SetAttribute(Attribute::SlowBlink)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::REVERSE) {
            queue!(self.stdout, SetAttribute(Attribute::Reverse)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::HIDDEN) {
            queue!(self.stdout, SetAttribute(Attribute::Hidden)).map_err(Error::Io)?;
        }
        if attrs.contains(TextAttributes::STRIKETHROUGH) {
            queue!(self.stdout, SetAttribute(Attribute::CrossedOut)).map_err(Error::Io)?;
        }
        Ok(())
    }

    fn reset_style(&mut self) -> Result<()> {
        queue!(self.stdout, ResetColor, SetAttribute(Attribute::Reset)).map_err(Error::Io)
    }

    fn write_str(&mut self, s: &str) -> Result<()> {
        queue!(self.stdout, Print(s)).map_err(Error::Io)
    }

    fn enable_bracketed_paste(&mut self) -> Result<()> {
        execute!(self.stdout, EnableBracketedPaste).map_err(Error::Io)
    }

    fn disable_bracketed_paste(&mut self) -> Result<()> {
        execute!(self.stdout, DisableBracketedPaste).map_err(Error::Io)
    }

    fn enable_focus_change(&mut self) -> Result<()> {
        execute!(self.stdout, EnableFocusChange).map_err(Error::Io)
    }

    fn disable_focus_change(&mut self) -> Result<()> {
        execute!(self.stdout, DisableFocusChange).map_err(Error::Io)
    }

    fn begin_sync_update(&mut self) -> Result<()> {
        if self.capabilities.has_sync_output() {
            // DEC Synchronized Output mode (mode 2026)
            self.stdout.write_all(b"\x1b[?2026h").map_err(Error::Io)?;
        }
        Ok(())
    }

    fn end_sync_update(&mut self) -> Result<()> {
        if self.capabilities.has_sync_output() {
            self.stdout.write_all(b"\x1b[?2026l").map_err(Error::Io)?;
        }
        Ok(())
    }

    fn set_cursor_style(&mut self, style: CursorStyle, blinking: bool) -> Result<()> {
        use crossterm::cursor::SetCursorStyle;
        let ct_style = match (style, blinking) {
            (CursorStyle::Block, true) => SetCursorStyle::BlinkingBlock,
            (CursorStyle::Block, false) => SetCursorStyle::SteadyBlock,
            (CursorStyle::Line, true) => SetCursorStyle::BlinkingBar,
            (CursorStyle::Line, false) => SetCursorStyle::SteadyBar,
            (CursorStyle::Underline, true) => SetCursorStyle::BlinkingUnderScore,
            (CursorStyle::Underline, false) => SetCursorStyle::SteadyUnderScore,
        };
        queue!(self.stdout, ct_style).map_err(Error::Io)
    }

    fn set_cursor_color(&mut self, color: Color) -> Result<()> {
        let (r, g, b) = color.to_rgb_u8();
        // ANSI OSC 12 sequence: ESC ] 12 ; #RRGGBB BEL
        let seq = format!("\x1b]12;#{:02x}{:02x}{:02x}\x07", r, g, b);
        self.stdout.write_all(seq.as_bytes()).map_err(Error::Io)
    }

    fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
}

impl Drop for CrosstermBackend {
    fn drop(&mut self) {
        // Best-effort cleanup on drop
        let _ = self.disable_mouse_capture();
        let _ = self.show_cursor();
        let _ = self.leave_alternate_screen();
        let _ = self.exit_raw_mode();
        let _ = self.disable_bracketed_paste();
        let _ = self.disable_focus_change();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_color_conversion() {
        let color = Color::from_rgb_u8(255, 128, 64);
        let ct_color = CrosstermBackend::to_crossterm_color(color);
        match ct_color {
            CrosstermColor::Rgb { r, g, b } => {
                assert_eq!(r, 255);
                assert_eq!(g, 128);
                assert_eq!(b, 64);
            }
            _ => panic!("Expected RGB color"),
        }
    }
}
