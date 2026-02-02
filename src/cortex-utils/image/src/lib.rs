//! Image utilities for Cortex.

use base64::{Engine, engine::general_purpose::STANDARD};
use std::path::Path;

/// Load an image and encode it as base64.
pub fn load_image_as_base64(path: &Path) -> anyhow::Result<(String, String)> {
    let data = std::fs::read(path)?;
    let base64_data = STANDARD.encode(&data);

    let media_type = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    };

    Ok((base64_data, media_type.to_string()))
}
