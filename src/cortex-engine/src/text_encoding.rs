//! Text encoding utilities.
//!
//! Provides text encoding detection and conversion for handling
//! various file encodings.

use std::borrow::Cow;
use std::io::{self, Read};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Text encoding type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Encoding {
    /// UTF-8 encoding.
    #[default]
    Utf8,
    /// UTF-8 with BOM.
    Utf8Bom,
    /// UTF-16 Little Endian.
    Utf16Le,
    /// UTF-16 Big Endian.
    Utf16Be,
    /// ASCII.
    Ascii,
    /// ISO-8859-1 (Latin-1).
    Latin1,
    /// Windows-1252.
    Windows1252,
    /// Unknown encoding.
    Unknown,
}

impl Encoding {
    /// Detect encoding from bytes.
    pub fn detect(data: &[u8]) -> Self {
        // Check for BOM
        if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
            return Self::Utf8Bom;
        }
        if data.starts_with(&[0xFF, 0xFE]) {
            return Self::Utf16Le;
        }
        if data.starts_with(&[0xFE, 0xFF]) {
            return Self::Utf16Be;
        }

        // Try UTF-8
        if std::str::from_utf8(data).is_ok() {
            // Check if it's pure ASCII
            if data.iter().all(|&b| b < 128) {
                return Self::Ascii;
            }
            return Self::Utf8;
        }

        // Check for likely Latin-1/Windows-1252
        if data.iter().all(|&b| b < 128 || (160..=255).contains(&b)) {
            return Self::Latin1;
        }

        Self::Unknown
    }

    /// Detect encoding from file.
    pub fn detect_file(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut file = std::fs::File::open(path)?;
        let mut buffer = vec![0u8; 8192];
        let n = file.read(&mut buffer)?;
        buffer.truncate(n);
        Ok(Self::detect(&buffer))
    }

    /// Get the BOM for this encoding.
    pub fn bom(&self) -> &'static [u8] {
        match self {
            Self::Utf8Bom => &[0xEF, 0xBB, 0xBF],
            Self::Utf16Le => &[0xFF, 0xFE],
            Self::Utf16Be => &[0xFE, 0xFF],
            _ => &[],
        }
    }

    /// Get encoding name.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Utf8 => "UTF-8",
            Self::Utf8Bom => "UTF-8-BOM",
            Self::Utf16Le => "UTF-16LE",
            Self::Utf16Be => "UTF-16BE",
            Self::Ascii => "ASCII",
            Self::Latin1 => "ISO-8859-1",
            Self::Windows1252 => "Windows-1252",
            Self::Unknown => "Unknown",
        }
    }

    /// Check if encoding is Unicode-based.
    pub fn is_unicode(&self) -> bool {
        matches!(
            self,
            Self::Utf8 | Self::Utf8Bom | Self::Utf16Le | Self::Utf16Be
        )
    }
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

/// Decode bytes to string.
pub fn decode(data: &[u8], encoding: Encoding) -> Result<Cow<'_, str>, EncodingError> {
    match encoding {
        Encoding::Utf8 => std::str::from_utf8(data)
            .map(Cow::Borrowed)
            .map_err(|e| EncodingError::InvalidUtf8(e.valid_up_to())),
        Encoding::Utf8Bom => {
            let data = data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(data);
            std::str::from_utf8(data)
                .map(Cow::Borrowed)
                .map_err(|e| EncodingError::InvalidUtf8(e.valid_up_to()))
        }
        Encoding::Utf16Le => decode_utf16le(data),
        Encoding::Utf16Be => decode_utf16be(data),
        Encoding::Ascii => {
            if data.iter().all(|&b| b < 128) {
                // Safe: all bytes are valid ASCII
                Ok(Cow::Owned(data.iter().map(|&b| b as char).collect()))
            } else {
                Err(EncodingError::InvalidAscii)
            }
        }
        Encoding::Latin1 | Encoding::Windows1252 => {
            // Latin-1 maps directly to Unicode code points
            Ok(Cow::Owned(data.iter().map(|&b| b as char).collect()))
        }
        Encoding::Unknown => {
            // Try UTF-8, then Latin-1
            std::str::from_utf8(data)
                .map(Cow::Borrowed)
                .or_else(|_| Ok(Cow::Owned(data.iter().map(|&b| b as char).collect())))
        }
    }
}

/// Decode UTF-16LE bytes.
fn decode_utf16le(data: &[u8]) -> Result<Cow<'static, str>, EncodingError> {
    // Skip BOM if present
    let data = data.strip_prefix(&[0xFF, 0xFE]).unwrap_or(data);

    if !data.len().is_multiple_of(2) {
        return Err(EncodingError::InvalidLength);
    }

    let chars: Result<String, _> = data
        .chunks_exact(2)
        .map(|chunk| {
            let code = u16::from_le_bytes([chunk[0], chunk[1]]);
            char::from_u32(code as u32).ok_or(EncodingError::InvalidCodePoint(code as u32))
        })
        .collect();

    chars.map(Cow::Owned)
}

/// Decode UTF-16BE bytes.
fn decode_utf16be(data: &[u8]) -> Result<Cow<'static, str>, EncodingError> {
    // Skip BOM if present
    let data = data.strip_prefix(&[0xFE, 0xFF]).unwrap_or(data);

    if !data.len().is_multiple_of(2) {
        return Err(EncodingError::InvalidLength);
    }

    let chars: Result<String, _> = data
        .chunks_exact(2)
        .map(|chunk| {
            let code = u16::from_be_bytes([chunk[0], chunk[1]]);
            char::from_u32(code as u32).ok_or(EncodingError::InvalidCodePoint(code as u32))
        })
        .collect();

    chars.map(Cow::Owned)
}

/// Encode string to bytes.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => {
            let mut result = vec![0xEF, 0xBB, 0xBF];
            result.extend_from_slice(text.as_bytes());
            result
        }
        Encoding::Utf16Le => {
            let mut result = vec![0xFF, 0xFE];
            for c in text.chars() {
                let code = c as u32;
                if code <= 0xFFFF {
                    result.extend_from_slice(&(code as u16).to_le_bytes());
                }
            }
            result
        }
        Encoding::Utf16Be => {
            let mut result = vec![0xFE, 0xFF];
            for c in text.chars() {
                let code = c as u32;
                if code <= 0xFFFF {
                    result.extend_from_slice(&(code as u16).to_be_bytes());
                }
            }
            result
        }
        Encoding::Ascii => text
            .chars()
            .filter(|&c| (c as u32) < 128)
            .map(|c| c as u8)
            .collect(),
        Encoding::Latin1 | Encoding::Windows1252 => text
            .chars()
            .filter(|&c| (c as u32) < 256)
            .map(|c| c as u8)
            .collect(),
        Encoding::Unknown => text.as_bytes().to_vec(),
    }
}

/// Encoding error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EncodingError {
    /// Invalid UTF-8 at position.
    InvalidUtf8(usize),
    /// Invalid ASCII byte.
    InvalidAscii,
    /// Invalid code point.
    InvalidCodePoint(u32),
    /// Invalid length for encoding.
    InvalidLength,
    /// IO error.
    Io(String),
}

impl std::fmt::Display for EncodingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidUtf8(pos) => write!(f, "Invalid UTF-8 at position {pos}"),
            Self::InvalidAscii => write!(f, "Invalid ASCII byte"),
            Self::InvalidCodePoint(cp) => write!(f, "Invalid code point: U+{cp:04X}"),
            Self::InvalidLength => write!(f, "Invalid length for encoding"),
            Self::Io(msg) => write!(f, "IO error: {msg}"),
        }
    }
}

impl std::error::Error for EncodingError {}

impl From<io::Error> for EncodingError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Normalize line endings.
pub fn normalize_line_endings(text: &str, style: LineEnding) -> String {
    let normalized = text.replace("\r\n", "\n").replace("\r", "\n");
    match style {
        LineEnding::Lf => normalized,
        LineEnding::CrLf => normalized.replace("\n", "\r\n"),
        LineEnding::Cr => normalized.replace("\n", "\r"),
    }
}

/// Line ending style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineEnding {
    /// Unix-style (LF).
    Lf,
    /// Windows-style (CRLF).
    CrLf,
    /// Old Mac-style (CR).
    Cr,
}

impl LineEnding {
    /// Detect line ending style.
    pub fn detect(text: &str) -> Self {
        if text.contains("\r\n") {
            Self::CrLf
        } else if text.contains("\r") {
            Self::Cr
        } else {
            Self::Lf
        }
    }

    /// Get the line ending string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::CrLf => "\r\n",
            Self::Cr => "\r",
        }
    }

    /// Get system default.
    pub fn system_default() -> Self {
        #[cfg(windows)]
        {
            Self::CrLf
        }
        #[cfg(not(windows))]
        {
            Self::Lf
        }
    }
}

impl Default for LineEnding {
    fn default() -> Self {
        Self::system_default()
    }
}

/// Check if text is valid UTF-8.
pub fn is_valid_utf8(data: &[u8]) -> bool {
    std::str::from_utf8(data).is_ok()
}

/// Check if text is binary (contains null bytes or too many non-printable characters).
pub fn is_binary(data: &[u8]) -> bool {
    // Check for null bytes
    if data.contains(&0) {
        return true;
    }

    // Check ratio of non-printable characters
    let non_printable = data
        .iter()
        .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
        .count();

    let threshold = (data.len() as f64 * 0.1) as usize;
    non_printable > threshold
}

/// Sanitize text by removing or replacing invalid characters.
pub fn sanitize(text: &str) -> String {
    text.chars()
        .filter(|&c| !c.is_control() || c == '\n' || c == '\r' || c == '\t')
        .collect()
}

/// Truncate text to fit within byte limit while respecting UTF-8 boundaries.
pub fn truncate_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    &text[..end]
}

/// Count lines in text.
pub fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_encoding() {
        assert_eq!(Encoding::detect(b"hello"), Encoding::Ascii);
        assert_eq!(Encoding::detect("héllo".as_bytes()), Encoding::Utf8);
        assert_eq!(
            Encoding::detect(&[0xEF, 0xBB, 0xBF, 0x68, 0x65, 0x6C, 0x6C, 0x6F]),
            Encoding::Utf8Bom
        );
    }

    #[test]
    fn test_decode_utf8() {
        let text = decode(b"hello", Encoding::Utf8).unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_encode_utf8() {
        let bytes = encode("hello", Encoding::Utf8);
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn test_line_endings() {
        assert_eq!(LineEnding::detect("hello\nworld"), LineEnding::Lf);
        assert_eq!(LineEnding::detect("hello\r\nworld"), LineEnding::CrLf);

        let normalized = normalize_line_endings("hello\r\nworld", LineEnding::Lf);
        assert_eq!(normalized, "hello\nworld");
    }

    #[test]
    fn test_is_binary() {
        assert!(!is_binary(b"hello world"));
        assert!(is_binary(&[0x00, 0x01, 0x02]));
    }

    #[test]
    fn test_truncate_bytes() {
        let text = "héllo";
        let truncated = truncate_bytes(text, 3);
        assert_eq!(truncated, "hé");
    }
}
