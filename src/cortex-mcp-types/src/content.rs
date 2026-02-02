//! Content types for MCP protocol.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::resources::ResourceContent;

/// Content item.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Content {
    /// Text content.
    Text {
        /// The text content.
        text: String,
    },
    /// Image content.
    Image {
        /// Base64-encoded image data.
        data: String,
        /// MIME type of the image.
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    /// Resource content reference.
    Resource {
        /// The resource content.
        resource: ResourceContent,
    },
}

impl Content {
    /// Create text content.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// Create image content.
    pub fn image(data: impl Into<String>, mime_type: impl Into<String>) -> Self {
        Self::Image {
            data: data.into(),
            mime_type: mime_type.into(),
        }
    }

    /// Create resource content.
    pub fn resource(resource: ResourceContent) -> Self {
        Self::Resource { resource }
    }

    /// Get as text if this is text content.
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_types() {
        let text = Content::text("Hello, world!");
        assert!(matches!(&text, Content::Text { text } if text == "Hello, world!"));
        assert_eq!(text.as_text(), Some("Hello, world!"));

        let image = Content::image("base64data", "image/png");
        assert!(matches!(image, Content::Image { .. }));
        assert!(image.as_text().is_none());
    }
}
