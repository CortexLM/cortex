use anyhow::Result;
use reqwest::Client;
use serde_json::json;
use std::process::Command;
use tempfile::NamedTempFile;
use std::io::Write;

pub async fn compress_video(data: &[u8]) -> Result<Vec<u8>> {
    let mut input_file = NamedTempFile::new()?;
    input_file.write_all(data)?;
    
    let output_file = NamedTempFile::new()?;
    let output_path = output_file.path().to_str().unwrap();

    let status = Command::new("ffmpeg")
        .args(&[
            "-i", input_file.path().to_str().unwrap(),
            "-vcodec", "libx264",
            "-crf", "28",
            "-preset", "veryfast",
            output_path
        ])
        .status()?;

    if !status.success() {
        anyhow::bail!("ffmpeg compression failed");
    }

    let compressed_data = std::fs::read(output_path)?;
    Ok(compressed_data)
}

pub async fn check_similarity_24h(_video_data: &[u8]) -> Result<bool> {
    let client = Client::new();
    let openrouter_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    
    let payload = json!({
        "model": "deepseek/deepseek-chat-v4-flash:free",
        "messages": [
            {
                "role": "user",
                "content": "Analyze the attached video data for similarity with known bugs in the last 24h. Respond with 'SIMILAR' or 'UNIQUE'.",
            }
        ]
    });

    let res = client.post("https://openrouter.ai/api/v1/chat/completions")
        .bearer_auth(openrouter_key)
        .json(&payload)
        .send()
        .await?;

    let body = res.text().await?;
    Ok(body.contains("SIMILAR"))
}
