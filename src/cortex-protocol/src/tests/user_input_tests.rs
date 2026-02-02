//! Comprehensive tests for UserInput types.

use crate::user_input::UserInput;

#[test]
fn test_user_input_text_creation() {
    let input1 = UserInput::text("Hello, world!");
    let input2 = UserInput::text(String::from("From String"));

    assert!(input1.is_text());
    assert!(input2.is_text());
    assert!(!input1.is_image());
}

#[test]
fn test_user_input_text_as_text() {
    let input = UserInput::text("Test content");

    assert_eq!(input.as_text(), Some("Test content"));
}

#[test]
fn test_user_input_image_base64() {
    let input = UserInput::image("base64EncodedData...", "image/png");

    assert!(input.is_image());
    assert!(!input.is_text());
    assert_eq!(input.as_text(), None);
}

#[test]
fn test_user_input_image_url() {
    let input = UserInput::image_url("https://example.com/image.jpg");

    assert!(input.is_image());
    assert!(!input.is_text());
}

#[test]
fn test_user_input_file() {
    let input = UserInput::file("/path/to/document.pdf");

    assert!(!input.is_text());
    assert!(!input.is_image());
    assert_eq!(input.as_text(), None);
}

#[test]
fn test_user_input_serialization_text() {
    let input = UserInput::text("Serialize me");

    let json = serde_json::to_string(&input).expect("serialize");
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("Serialize me"));

    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");
    assert!(parsed.is_text());
    assert_eq!(parsed.as_text(), Some("Serialize me"));
}

#[test]
fn test_user_input_serialization_image() {
    let input = UserInput::Image {
        data: "iVBORw0KGgo=".to_string(),
        media_type: "image/png".to_string(),
    };

    let json = serde_json::to_string(&input).expect("serialize");
    assert!(json.contains("\"type\":\"image\""));
    assert!(json.contains("image/png"));

    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");
    assert!(parsed.is_image());
}

#[test]
fn test_user_input_serialization_image_url() {
    let input = UserInput::ImageUrl {
        url: "https://example.com/photo.jpg".to_string(),
        detail: Some("high".to_string()),
    };

    let json = serde_json::to_string(&input).expect("serialize");
    assert!(json.contains("\"type\":\"image_url\""));
    assert!(json.contains("https://example.com/photo.jpg"));
    assert!(json.contains("high"));

    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");
    assert!(parsed.is_image());
}

#[test]
fn test_user_input_serialization_file() {
    let input = UserInput::File {
        path: "/documents/report.txt".to_string(),
        content: Some("File contents here".to_string()),
    };

    let json = serde_json::to_string(&input).expect("serialize");
    assert!(json.contains("\"type\":\"file\""));
    assert!(json.contains("/documents/report.txt"));
    assert!(json.contains("File contents here"));

    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");
    assert!(!parsed.is_text());
    assert!(!parsed.is_image());
}

#[test]
fn test_user_input_equality() {
    let input1 = UserInput::text("Same content");
    let input2 = UserInput::text("Same content");
    let input3 = UserInput::text("Different content");

    assert_eq!(input1, input2);
    assert_ne!(input1, input3);
}

#[test]
fn test_user_input_clone() {
    let original = UserInput::Image {
        data: "data".to_string(),
        media_type: "image/jpeg".to_string(),
    };

    let cloned = original.clone();
    assert_eq!(original, cloned);
}

#[test]
fn test_user_input_image_url_without_detail() {
    let input = UserInput::image_url("https://example.com/img.png");

    let json = serde_json::to_string(&input).expect("serialize");
    // detail should not appear when None (skip_serializing_if)
    assert!(!json.contains("detail"));
}

#[test]
fn test_user_input_file_without_content() {
    let input = UserInput::file("/path/to/file.txt");

    let json = serde_json::to_string(&input).expect("serialize");
    // content should not appear when None
    assert!(!json.contains("content"));
}

#[test]
fn test_user_input_empty_text() {
    let input = UserInput::text("");

    assert!(input.is_text());
    assert_eq!(input.as_text(), Some(""));
}

#[test]
fn test_user_input_unicode_text() {
    let input = UserInput::text("Hello 世界! 🌍 مرحبا");

    let json = serde_json::to_string(&input).expect("serialize");
    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.as_text(), Some("Hello 世界! 🌍 مرحبا"));
}

#[test]
fn test_user_input_multiline_text() {
    let text = "Line 1\nLine 2\nLine 3\n\tIndented";
    let input = UserInput::text(text);

    let json = serde_json::to_string(&input).expect("serialize");
    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.as_text(), Some(text));
}

#[test]
fn test_user_input_special_characters_in_text() {
    let text = r#"Special: "quotes", 'apostrophes', \backslash, /slash"#;
    let input = UserInput::text(text);

    let json = serde_json::to_string(&input).expect("serialize");
    let parsed: UserInput = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.as_text(), Some(text));
}

#[test]
fn test_user_input_array_serialization() {
    let inputs = vec![
        UserInput::text("First"),
        UserInput::text("Second"),
        UserInput::image("data", "image/png"),
    ];

    let json = serde_json::to_string(&inputs).expect("serialize");
    let parsed: Vec<UserInput> = serde_json::from_str(&json).expect("deserialize");

    assert_eq!(parsed.len(), 3);
    assert!(parsed[0].is_text());
    assert!(parsed[1].is_text());
    assert!(parsed[2].is_image());
}

#[test]
fn test_user_input_debug() {
    let input = UserInput::text("Debug test");
    let debug_str = format!("{:?}", input);

    assert!(debug_str.contains("Text"));
    assert!(debug_str.contains("Debug test"));
}

#[test]
fn test_user_input_media_types() {
    let media_types = vec![
        "image/png",
        "image/jpeg",
        "image/gif",
        "image/webp",
        "image/svg+xml",
    ];

    for media_type in media_types {
        let input = UserInput::image("data", media_type);
        let json = serde_json::to_string(&input).expect("serialize");
        assert!(json.contains(media_type));
    }
}
