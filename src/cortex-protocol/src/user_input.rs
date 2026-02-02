//! User input types with validation support.

#![allow(clippy::map_clone, clippy::collapsible_if)]

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Maximum allowed text input size in bytes (10 MB).
pub const MAX_TEXT_SIZE: usize = 10 * 1024 * 1024;

/// Maximum allowed image data size in bytes (20 MB base64).
pub const MAX_IMAGE_DATA_SIZE: usize = 20 * 1024 * 1024;

/// Maximum allowed URL length (2048 characters).
pub const MAX_URL_LENGTH: usize = 2048;

/// Maximum allowed file path length (4096 characters).
pub const MAX_PATH_LENGTH: usize = 4096;

/// Maximum allowed file content size in bytes (50 MB).
pub const MAX_FILE_CONTENT_SIZE: usize = 50 * 1024 * 1024;

/// Validation error for user input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserInputValidationError {
    /// Text input exceeds maximum size.
    TextTooLarge { size: usize, max: usize },
    /// Image data exceeds maximum size.
    ImageDataTooLarge { size: usize, max: usize },
    /// URL exceeds maximum length.
    UrlTooLong { length: usize, max: usize },
    /// File path exceeds maximum length.
    PathTooLong { length: usize, max: usize },
    /// File content exceeds maximum size.
    FileContentTooLarge { size: usize, max: usize },
    /// Invalid media type format.
    InvalidMediaType { media_type: String },
    /// Empty text input.
    EmptyText,
    /// Empty image data.
    EmptyImageData,
    /// Empty URL.
    EmptyUrl,
    /// Empty file path.
    EmptyPath,
}

impl std::fmt::Display for UserInputValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TextTooLarge { size, max } => {
                write!(
                    f,
                    "Text input size ({} bytes) exceeds maximum ({} bytes)",
                    size, max
                )
            }
            Self::ImageDataTooLarge { size, max } => {
                write!(
                    f,
                    "Image data size ({} bytes) exceeds maximum ({} bytes)",
                    size, max
                )
            }
            Self::UrlTooLong { length, max } => {
                write!(
                    f,
                    "URL length ({} chars) exceeds maximum ({} chars)",
                    length, max
                )
            }
            Self::PathTooLong { length, max } => {
                write!(
                    f,
                    "File path length ({} chars) exceeds maximum ({} chars)",
                    length, max
                )
            }
            Self::FileContentTooLarge { size, max } => {
                write!(
                    f,
                    "File content size ({} bytes) exceeds maximum ({} bytes)",
                    size, max
                )
            }
            Self::InvalidMediaType { media_type } => {
                write!(f, "Invalid media type format: '{}'", media_type)
            }
            Self::EmptyText => write!(f, "Text input cannot be empty"),
            Self::EmptyImageData => write!(f, "Image data cannot be empty"),
            Self::EmptyUrl => write!(f, "URL cannot be empty"),
            Self::EmptyPath => write!(f, "File path cannot be empty"),
        }
    }
}

impl std::error::Error for UserInputValidationError {}

/// User input item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserInput {
    /// Text input.
    Text { text: String },

    /// Image input with base64 data.
    Image { data: String, media_type: String },

    /// Image input from URL.
    ImageUrl {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },

    /// File reference.
    File {
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        content: Option<String>,
    },
}

impl UserInput {
    /// Create a text input.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create a validated text input.
    ///
    /// Returns an error if the text is empty or exceeds the maximum size.
    pub fn text_validated(text: impl Into<String>) -> Result<Self, UserInputValidationError> {
        let text = text.into();
        Self::Text { text }.validate().map(|input| input.clone())
    }

    /// Create an image input from base64 data.
    pub fn image(data: impl Into<String>, media_type: impl Into<String>) -> Self {
        Self::Image {
            data: data.into(),
            media_type: media_type.into(),
        }
    }

    /// Create a validated image input from base64 data.
    ///
    /// Returns an error if validation fails.
    pub fn image_validated(
        data: impl Into<String>,
        media_type: impl Into<String>,
    ) -> Result<Self, UserInputValidationError> {
        let input = Self::Image {
            data: data.into(),
            media_type: media_type.into(),
        };
        input.validate().map(|i| i.clone())
    }

    /// Create an image input from URL.
    pub fn image_url(url: impl Into<String>) -> Self {
        Self::ImageUrl {
            url: url.into(),
            detail: None,
        }
    }

    /// Create a validated image input from URL.
    ///
    /// Returns an error if the URL is empty or too long.
    pub fn image_url_validated(url: impl Into<String>) -> Result<Self, UserInputValidationError> {
        let input = Self::ImageUrl {
            url: url.into(),
            detail: None,
        };
        input.validate().map(|i| i.clone())
    }

    /// Create a file reference.
    pub fn file(path: impl Into<String>) -> Self {
        Self::File {
            path: path.into(),
            content: None,
        }
    }

    /// Create a validated file reference.
    ///
    /// Returns an error if the path is empty or too long.
    pub fn file_validated(path: impl Into<String>) -> Result<Self, UserInputValidationError> {
        let input = Self::File {
            path: path.into(),
            content: None,
        };
        input.validate().map(|i| i.clone())
    }

    /// Create a file reference with content.
    pub fn file_with_content(path: impl Into<String>, content: impl Into<String>) -> Self {
        Self::File {
            path: path.into(),
            content: Some(content.into()),
        }
    }

    /// Create a validated file reference with content.
    ///
    /// Returns an error if validation fails.
    pub fn file_with_content_validated(
        path: impl Into<String>,
        content: impl Into<String>,
    ) -> Result<Self, UserInputValidationError> {
        let input = Self::File {
            path: path.into(),
            content: Some(content.into()),
        };
        input.validate().map(|i| i.clone())
    }

    /// Check if this is a text input.
    pub fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Check if this is an image input.
    pub fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. } | Self::ImageUrl { .. })
    }

    /// Check if this is a file input.
    pub fn is_file(&self) -> bool {
        matches!(self, Self::File { .. })
    }

    /// Get text content if this is a text input.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Validate the user input against size and format constraints.
    ///
    /// Returns `Ok(&self)` if valid, or an error describing the validation failure.
    pub fn validate(&self) -> Result<&Self, UserInputValidationError> {
        match self {
            Self::Text { text } => {
                if text.is_empty() {
                    return Err(UserInputValidationError::EmptyText);
                }
                if text.len() > MAX_TEXT_SIZE {
                    return Err(UserInputValidationError::TextTooLarge {
                        size: text.len(),
                        max: MAX_TEXT_SIZE,
                    });
                }
            }
            Self::Image { data, media_type } => {
                if data.is_empty() {
                    return Err(UserInputValidationError::EmptyImageData);
                }
                if data.len() > MAX_IMAGE_DATA_SIZE {
                    return Err(UserInputValidationError::ImageDataTooLarge {
                        size: data.len(),
                        max: MAX_IMAGE_DATA_SIZE,
                    });
                }
                // Basic media type validation: should be in format "type/subtype"
                if !media_type.contains('/') || media_type.split('/').count() != 2 {
                    return Err(UserInputValidationError::InvalidMediaType {
                        media_type: media_type.clone(),
                    });
                }
            }
            Self::ImageUrl { url, .. } => {
                if url.is_empty() {
                    return Err(UserInputValidationError::EmptyUrl);
                }
                if url.len() > MAX_URL_LENGTH {
                    return Err(UserInputValidationError::UrlTooLong {
                        length: url.len(),
                        max: MAX_URL_LENGTH,
                    });
                }
            }
            Self::File { path, content } => {
                if path.is_empty() {
                    return Err(UserInputValidationError::EmptyPath);
                }
                if path.len() > MAX_PATH_LENGTH {
                    return Err(UserInputValidationError::PathTooLong {
                        length: path.len(),
                        max: MAX_PATH_LENGTH,
                    });
                }
                if let Some(c) = content {
                    if c.len() > MAX_FILE_CONTENT_SIZE {
                        return Err(UserInputValidationError::FileContentTooLarge {
                            size: c.len(),
                            max: MAX_FILE_CONTENT_SIZE,
                        });
                    }
                }
            }
        }
        Ok(self)
    }

    /// Returns the size in bytes of this input's primary content.
    pub fn content_size(&self) -> usize {
        match self {
            Self::Text { text } => text.len(),
            Self::Image { data, .. } => data.len(),
            Self::ImageUrl { url, .. } => url.len(),
            Self::File { path, content } => path.len() + content.as_ref().map_or(0, |c| c.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_input_text() {
        let input = UserInput::text("Hello, world!");
        assert!(input.is_text());
        assert!(!input.is_image());
        assert!(!input.is_file());
        assert_eq!(input.as_text(), Some("Hello, world!"));
    }

    #[test]
    fn test_user_input_serde() {
        let input = UserInput::Image {
            data: "base64data".to_string(),
            media_type: "image/png".to_string(),
        };

        let json = serde_json::to_string(&input).expect("serialize");
        assert!(json.contains("image"));
        assert!(json.contains("image/png"));

        let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");
        assert!(parsed.is_image());
    }

    #[test]
    fn test_validation_text_valid() {
        let input = UserInput::text("Hello, world!");
        assert!(input.validate().is_ok());
    }

    #[test]
    fn test_validation_text_empty() {
        let input = UserInput::text("");
        let result = input.validate();
        assert!(matches!(result, Err(UserInputValidationError::EmptyText)));
    }

    #[test]
    fn test_validation_text_too_large() {
        // Create a string larger than MAX_TEXT_SIZE
        let large_text = "x".repeat(MAX_TEXT_SIZE + 1);
        let input = UserInput::text(large_text);
        let result = input.validate();
        assert!(matches!(
            result,
            Err(UserInputValidationError::TextTooLarge { .. })
        ));
    }

    #[test]
    fn test_validation_image_valid() {
        let input = UserInput::image("base64data", "image/png");
        assert!(input.validate().is_ok());
    }

    #[test]
    fn test_validation_image_empty_data() {
        let input = UserInput::image("", "image/png");
        let result = input.validate();
        assert!(matches!(
            result,
            Err(UserInputValidationError::EmptyImageData)
        ));
    }

    #[test]
    fn test_validation_image_invalid_media_type() {
        let input = UserInput::image("base64data", "invalid");
        let result = input.validate();
        assert!(matches!(
            result,
            Err(UserInputValidationError::InvalidMediaType { .. })
        ));
    }

    #[test]
    fn test_validation_image_url_valid() {
        let input = UserInput::image_url("https://example.com/image.png");
        assert!(input.validate().is_ok());
    }

    #[test]
    fn test_validation_image_url_empty() {
        let input = UserInput::image_url("");
        let result = input.validate();
        assert!(matches!(result, Err(UserInputValidationError::EmptyUrl)));
    }

    #[test]
    fn test_validation_image_url_too_long() {
        let long_url = "https://example.com/".to_string() + &"x".repeat(MAX_URL_LENGTH);
        let input = UserInput::image_url(long_url);
        let result = input.validate();
        assert!(matches!(
            result,
            Err(UserInputValidationError::UrlTooLong { .. })
        ));
    }

    #[test]
    fn test_validation_file_valid() {
        let input = UserInput::file("/path/to/file.txt");
        assert!(input.validate().is_ok());
    }

    #[test]
    fn test_validation_file_empty_path() {
        let input = UserInput::file("");
        let result = input.validate();
        assert!(matches!(result, Err(UserInputValidationError::EmptyPath)));
    }

    #[test]
    fn test_validation_file_with_content_valid() {
        let input = UserInput::file_with_content("/path/to/file.txt", "file content");
        assert!(input.validate().is_ok());
    }

    #[test]
    fn test_validated_constructors() {
        // Valid text
        assert!(UserInput::text_validated("Hello").is_ok());

        // Invalid text (empty)
        assert!(UserInput::text_validated("").is_err());

        // Valid image
        assert!(UserInput::image_validated("data", "image/png").is_ok());

        // Invalid image (bad media type)
        assert!(UserInput::image_validated("data", "bad").is_err());

        // Valid URL
        assert!(UserInput::image_url_validated("https://example.com").is_ok());

        // Invalid URL (empty)
        assert!(UserInput::image_url_validated("").is_err());

        // Valid file
        assert!(UserInput::file_validated("/path").is_ok());

        // Invalid file (empty)
        assert!(UserInput::file_validated("").is_err());
    }

    #[test]
    fn test_content_size() {
        let text = UserInput::text("hello");
        assert_eq!(text.content_size(), 5);

        let image = UserInput::image("base64", "image/png");
        assert_eq!(image.content_size(), 6);

        let url = UserInput::image_url("https://example.com");
        assert_eq!(url.content_size(), 19);

        let file = UserInput::file("/path");
        assert_eq!(file.content_size(), 5);

        let file_with_content = UserInput::file_with_content("/path", "content");
        assert_eq!(file_with_content.content_size(), 12); // 5 + 7
    }

    #[test]
    fn test_validation_error_display() {
        let error = UserInputValidationError::TextTooLarge { size: 100, max: 50 };
        let display = format!("{}", error);
        assert!(display.contains("100"));
        assert!(display.contains("50"));

        let error = UserInputValidationError::InvalidMediaType {
            media_type: "bad".to_string(),
        };
        let display = format!("{}", error);
        assert!(display.contains("bad"));
    }
}
