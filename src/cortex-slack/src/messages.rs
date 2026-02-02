//! Message formatting for Slack.
//!
//! Provides utilities for:
//! - Converting Markdown to Slack's mrkdwn format
//! - Building rich messages with Block Kit
//! - Formatting agent responses

use serde::{Deserialize, Serialize};

/// Convert Markdown to Slack mrkdwn format.
///
/// Slack uses a custom markup format called "mrkdwn" which differs from
/// standard Markdown:
/// - Bold: `**text**` → `*text*`
/// - Italic: `_text_` stays the same
/// - Strikethrough: `~~text~~` → `~text~`
/// - Code blocks stay the same
/// - Links: `[text](url)` → `<url|text>`
///
/// # Example
///
/// ```rust
/// use cortex_slack::markdown_to_mrkdwn;
///
/// let md = "**bold** and _italic_ with a [link](https://example.com)";
/// let mrkdwn = markdown_to_mrkdwn(md);
/// assert!(mrkdwn.contains("*bold*"));
/// ```
pub fn markdown_to_mrkdwn(md: &str) -> String {
    let result = md.to_string();

    // Track if we're inside a code block
    let mut in_code_block = false;
    let mut processed = String::with_capacity(result.len());
    let mut i = 0;

    while i < result.len() {
        let remaining = &result[i..];

        // Check for code block start/end
        if remaining.starts_with("```") {
            in_code_block = !in_code_block;
            processed.push_str("```");
            i += 3;
            continue;
        }

        // Skip conversion inside code blocks
        if in_code_block {
            if let Some(c) = result.chars().nth(i) {
                processed.push(c);
            }
            i += 1;
            continue;
        }

        // Check for inline code (backtick) - skip conversion inside
        if remaining.starts_with('`') {
            // Find the closing backtick
            if let Some(end) = remaining[1..].find('`') {
                processed.push_str(&remaining[..end + 2]);
                i += end + 2;
                continue;
            }
        }

        // Convert bold: **text** → *text*
        if remaining.starts_with("**")
            && let Some(end) = remaining[2..].find("**")
        {
            let content = &remaining[2..end + 2];
            processed.push('*');
            processed.push_str(content);
            processed.push('*');
            i += end + 4;
            continue;
        }

        // Convert strikethrough: ~~text~~ → ~text~
        if remaining.starts_with("~~")
            && let Some(end) = remaining[2..].find("~~")
        {
            let content = &remaining[2..end + 2];
            processed.push('~');
            processed.push_str(content);
            processed.push('~');
            i += end + 4;
            continue;
        }

        // Convert links: [text](url) → <url|text>
        if remaining.starts_with('[')
            && let Some(bracket_end) = remaining.find("](")
        {
            let text = &remaining[1..bracket_end];
            let url_start = bracket_end + 2;
            if let Some(url_end) = remaining[url_start..].find(')') {
                let url = &remaining[url_start..url_start + url_end];
                processed.push('<');
                processed.push_str(url);
                processed.push('|');
                processed.push_str(text);
                processed.push('>');
                i += url_start + url_end + 1;
                continue;
            }
        }

        // No special handling, copy character
        if let Some(c) = result.chars().nth(i) {
            processed.push(c);
        }
        i += 1;
    }

    processed
}

/// Slack Block Kit block types.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlackBlock {
    /// Header block.
    Header { text: SlackTextObject },
    /// Section block (main content).
    Section {
        text: SlackTextObject,
        #[serde(skip_serializing_if = "Option::is_none")]
        accessory: Option<SlackBlockElement>,
        #[serde(skip_serializing_if = "Option::is_none")]
        fields: Option<Vec<SlackTextObject>>,
    },
    /// Divider block.
    Divider {},
    /// Context block (small text/images).
    Context { elements: Vec<SlackContextElement> },
    /// Actions block (buttons, menus).
    Actions { elements: Vec<SlackBlockElement> },
    /// Image block.
    Image {
        image_url: String,
        alt_text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        title: Option<SlackTextObject>,
    },
}

/// Slack text object.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlackTextObject {
    #[serde(rename = "type")]
    pub text_type: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<bool>,
}

impl SlackTextObject {
    /// Create a plain text object.
    pub fn plain(text: impl Into<String>) -> Self {
        Self {
            text_type: "plain_text".to_string(),
            text: text.into(),
            emoji: Some(true),
        }
    }

    /// Create a mrkdwn text object.
    pub fn mrkdwn(text: impl Into<String>) -> Self {
        Self {
            text_type: "mrkdwn".to_string(),
            text: text.into(),
            emoji: None,
        }
    }
}

/// Slack context element (for context blocks).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlackContextElement {
    /// Plain text.
    PlainText { text: String },
    /// Mrkdwn text.
    Mrkdwn { text: String },
    /// Image.
    Image { image_url: String, alt_text: String },
}

/// Slack block element (buttons, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SlackBlockElement {
    /// Button element.
    Button {
        text: SlackTextObject,
        action_id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        style: Option<String>,
    },
}

/// Slack message content with blocks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SlackMessageContent {
    /// Fallback text for notifications.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Block Kit blocks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blocks: Option<Vec<SlackBlock>>,
    /// Thread timestamp (for replies).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_ts: Option<String>,
    /// Whether to also send to channel when in thread.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_broadcast: Option<bool>,
}

impl SlackMessageContent {
    /// Create a new message content.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set fallback text.
    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Set blocks.
    pub fn with_blocks(mut self, blocks: Vec<SlackBlock>) -> Self {
        self.blocks = Some(blocks);
        self
    }

    /// Set thread timestamp (for replies).
    pub fn in_thread(mut self, thread_ts: impl Into<String>) -> Self {
        self.thread_ts = Some(thread_ts.into());
        self
    }

    /// Broadcast to channel as well as thread.
    pub fn broadcast(mut self) -> Self {
        self.reply_broadcast = Some(true);
        self
    }
}

/// Builder for creating rich Slack messages.
pub struct SlackMessageBuilder {
    blocks: Vec<SlackBlock>,
    fallback_text: Option<String>,
}

impl SlackMessageBuilder {
    /// Create a new message builder.
    pub fn new() -> Self {
        Self {
            blocks: Vec::new(),
            fallback_text: None,
        }
    }

    /// Set fallback text for notifications.
    pub fn fallback(mut self, text: impl Into<String>) -> Self {
        self.fallback_text = Some(text.into());
        self
    }

    /// Add a header block.
    pub fn header(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(SlackBlock::Header {
            text: SlackTextObject::plain(text),
        });
        self
    }

    /// Add a section with mrkdwn text.
    pub fn section(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(SlackBlock::Section {
            text: SlackTextObject::mrkdwn(text),
            accessory: None,
            fields: None,
        });
        self
    }

    /// Add a section with fields.
    pub fn section_with_fields(mut self, fields: Vec<(String, String)>) -> Self {
        let field_objects: Vec<SlackTextObject> = fields
            .into_iter()
            .map(|(label, value)| SlackTextObject::mrkdwn(format!("*{}*\n{}", label, value)))
            .collect();

        self.blocks.push(SlackBlock::Section {
            text: SlackTextObject::mrkdwn(" "), // Empty placeholder
            accessory: None,
            fields: Some(field_objects),
        });
        self
    }

    /// Add a divider.
    pub fn divider(mut self) -> Self {
        self.blocks.push(SlackBlock::Divider {});
        self
    }

    /// Add a context block.
    pub fn context(mut self, text: impl Into<String>) -> Self {
        self.blocks.push(SlackBlock::Context {
            elements: vec![SlackContextElement::Mrkdwn { text: text.into() }],
        });
        self
    }

    /// Build the message content.
    pub fn build(self) -> SlackMessageContent {
        SlackMessageContent {
            text: self.fallback_text,
            blocks: Some(self.blocks),
            thread_ts: None,
            reply_broadcast: None,
        }
    }
}

impl Default for SlackMessageBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Format an agent result for Slack.
///
/// Creates a rich message with:
/// - Header with robot emoji
/// - Main content in mrkdwn
/// - Tool usage context (if any)
/// - Token usage context
pub fn format_agent_response(
    content: &str,
    tool_calls: usize,
    total_tokens: u64,
) -> SlackMessageContent {
    let mrkdwn_content = markdown_to_mrkdwn(content);

    let mut builder = SlackMessageBuilder::new()
        .fallback(content)
        .header("🤖 Cortex Response")
        .section(&mrkdwn_content);

    // Add tool usage info if any tools were called
    if tool_calls > 0 {
        builder = builder
            .divider()
            .context(format!("🔧 Used {} tools", tool_calls));
    }

    // Add token usage
    builder = builder.context(format!("📊 {} tokens", total_tokens));

    builder.build()
}

/// Format a code review response for Slack.
pub fn format_review_response(
    pr_url: &str,
    summary: &str,
    issues_count: usize,
    suggestions_count: usize,
) -> SlackMessageContent {
    let mrkdwn_summary = markdown_to_mrkdwn(summary);

    SlackMessageBuilder::new()
        .fallback(summary)
        .header("📝 PR Review")
        .section(format!("*PR:* <{}|View PR>", pr_url))
        .divider()
        .section(&mrkdwn_summary)
        .divider()
        .section_with_fields(vec![
            ("Issues".to_string(), issues_count.to_string()),
            ("Suggestions".to_string(), suggestions_count.to_string()),
        ])
        .build()
}

/// Format an error response for Slack.
pub fn format_error_response(error: &str) -> SlackMessageContent {
    SlackMessageBuilder::new()
        .fallback(error)
        .header("❌ Error")
        .section(format!("```{}```", error))
        .build()
}

/// Format a processing acknowledgment for Slack.
pub fn format_processing_ack() -> SlackMessageContent {
    SlackMessageContent::new().with_text("🔄 Processing...")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_markdown_to_mrkdwn_bold() {
        assert_eq!(markdown_to_mrkdwn("**bold**"), "*bold*");
        assert_eq!(markdown_to_mrkdwn("some **bold** text"), "some *bold* text");
    }

    #[test]
    fn test_markdown_to_mrkdwn_strikethrough() {
        assert_eq!(markdown_to_mrkdwn("~~strike~~"), "~strike~");
    }

    #[test]
    fn test_markdown_to_mrkdwn_links() {
        assert_eq!(
            markdown_to_mrkdwn("[click here](https://example.com)"),
            "<https://example.com|click here>"
        );
    }

    #[test]
    fn test_markdown_to_mrkdwn_code_blocks_preserved() {
        let md = "```rust\nlet x = 1;\n```";
        let mrkdwn = markdown_to_mrkdwn(md);
        assert!(mrkdwn.contains("```"));
    }

    #[test]
    fn test_markdown_to_mrkdwn_inline_code_preserved() {
        let md = "use `**bold**` in code";
        let mrkdwn = markdown_to_mrkdwn(md);
        // The **bold** inside backticks should be preserved
        assert!(mrkdwn.contains("`**bold**`"));
    }

    #[test]
    fn test_markdown_to_mrkdwn_mixed() {
        let md = "**bold** and _italic_ with a [link](https://example.com)";
        let mrkdwn = markdown_to_mrkdwn(md);
        assert!(mrkdwn.contains("*bold*"));
        assert!(mrkdwn.contains("_italic_"));
        assert!(mrkdwn.contains("<https://example.com|link>"));
    }

    #[test]
    fn test_message_builder() {
        let message = SlackMessageBuilder::new()
            .fallback("Test message")
            .header("Test Header")
            .section("Test content")
            .divider()
            .context("Test context")
            .build();

        assert_eq!(message.text, Some("Test message".to_string()));
        assert!(message.blocks.is_some());

        let blocks = message.blocks.unwrap();
        assert_eq!(blocks.len(), 4);
    }

    #[test]
    fn test_format_agent_response() {
        let message = format_agent_response("Test response", 2, 1000);

        assert!(message.blocks.is_some());
        let blocks = message.blocks.unwrap();

        // Should have header, section, divider, context (tools), context (tokens)
        assert!(blocks.len() >= 3);
    }

    #[test]
    fn test_text_object_plain() {
        let text = SlackTextObject::plain("Hello");
        assert_eq!(text.text_type, "plain_text");
        assert_eq!(text.text, "Hello");
    }

    #[test]
    fn test_text_object_mrkdwn() {
        let text = SlackTextObject::mrkdwn("*bold*");
        assert_eq!(text.text_type, "mrkdwn");
        assert_eq!(text.text, "*bold*");
    }

    #[test]
    fn test_message_content_thread() {
        let message = SlackMessageContent::new()
            .with_text("Reply")
            .in_thread("1234567890.123456");

        assert_eq!(message.thread_ts, Some("1234567890.123456".to_string()));
    }

    #[test]
    fn test_message_content_broadcast() {
        let message = SlackMessageContent::new()
            .with_text("Reply")
            .in_thread("1234567890.123456")
            .broadcast();

        assert_eq!(message.reply_broadcast, Some(true));
    }
}
