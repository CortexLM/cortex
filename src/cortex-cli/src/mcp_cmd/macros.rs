//! Safe print macros for handling broken pipes gracefully.
//!
//! Issue #1989: These macros prevent crashes when output is piped to commands
//! like `head` that close early.

/// Safely prints to stdout, ignoring broken pipe errors.
/// This prevents crashes when output is piped to commands like `head` that close early.
macro_rules! safe_println {
    () => {
        let _ = writeln!(std::io::stdout());
    };
    ($($arg:tt)*) => {
        let _ = writeln!(std::io::stdout(), $($arg)*);
    };
}

/// Safely prints to stdout without newline, ignoring broken pipe errors.
#[allow(unused_macros)]
macro_rules! safe_print {
    ($($arg:tt)*) => {
        let _ = write!(std::io::stdout(), $($arg)*);
    };
}

#[allow(unused_imports)]
pub(crate) use safe_print;
pub(crate) use safe_println;
