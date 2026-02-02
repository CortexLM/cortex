//! User input methods for SubmissionBuilder.

use cortex_protocol::{Op, UserInput};

use super::SubmissionBuilder;

impl SubmissionBuilder {
    /// Create a simple text message submission.
    ///
    /// This is the most common way to send user input to the agent.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let submission = SubmissionBuilder::user_message("What is 2 + 2?")
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn user_message(text: impl Into<String>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::UserInput {
            items: vec![UserInput::Text { text: text.into() }],
        });
        builder
    }

    /// Create a user input with multiple items (text, files, images).
    ///
    /// Use this when you need to send multiple input items in a single
    /// submission, such as text with attached files.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let items = vec![
    ///     SubmissionBuilder::text_input("Analyze this file:"),
    ///     SubmissionBuilder::file_input("/path/to/file.rs"),
    /// ];
    /// let submission = SubmissionBuilder::user_input(items)
    ///     .build()
    ///     .unwrap();
    /// ```
    pub fn user_input(items: Vec<UserInput>) -> Self {
        let mut builder = Self::new();
        builder.op = Some(Op::UserInput { items });
        builder
    }

    /// Create a text input item.
    ///
    /// Helper method for constructing `UserInput::Text` variants.
    pub fn text_input(text: impl Into<String>) -> UserInput {
        UserInput::Text { text: text.into() }
    }

    /// Create a file input item.
    ///
    /// The file will be read and its content sent to the agent.
    ///
    /// # Note
    ///
    /// The path should be a valid file path string. The file content
    /// is not read at builder time - it's handled by cortex-core.
    pub fn file_input(path: impl Into<String>) -> UserInput {
        UserInput::File {
            path: path.into(),
            content: None,
        }
    }

    /// Create a file input item with pre-loaded content.
    ///
    /// Use this when you've already read the file content.
    pub fn file_input_with_content(
        path: impl Into<String>,
        content: impl Into<String>,
    ) -> UserInput {
        UserInput::File {
            path: path.into(),
            content: Some(content.into()),
        }
    }

    /// Create an image input item from base64 data.
    ///
    /// # Arguments
    ///
    /// * `data` - Base64-encoded image data
    /// * `media_type` - MIME type (e.g., "image/png", "image/jpeg")
    pub fn image_input(data: impl Into<String>, media_type: impl Into<String>) -> UserInput {
        UserInput::Image {
            data: data.into(),
            media_type: media_type.into(),
        }
    }

    /// Create an image input item from a URL.
    ///
    /// The image will be fetched from the URL by cortex-core.
    pub fn image_url_input(url: impl Into<String>) -> UserInput {
        UserInput::ImageUrl {
            url: url.into(),
            detail: None,
        }
    }

    /// Create an image URL input with detail level.
    ///
    /// # Arguments
    ///
    /// * `url` - URL to the image
    /// * `detail` - Detail level (e.g., "low", "high", "auto")
    pub fn image_url_input_with_detail(
        url: impl Into<String>,
        detail: impl Into<String>,
    ) -> UserInput {
        UserInput::ImageUrl {
            url: url.into(),
            detail: Some(detail.into()),
        }
    }
}
