use reqwest::Client;
use serde_json::json;
use std::env;

pub async fn check_similarity(_video_data: &[u8]) -> Result<bool, reqwest::Error> {
    let client = Client::new();
    let api_key = env::var("OPENROUTER_API_KEY").unwrap_or_default();

    let _res = client
        .post("https://openrouter.ai/api/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", api_key))
        .header("HTTP-Referer", "https://base.intelligence")
        .json(&json!({
            "model": "deepseek/deepseek-chat-v4-flash:free",
            "messages": [{"role": "user", "content": "Check similarity"}]
        }))
        .send()
        .await?;

    // Simulate false (no duplicate found) for now
    Ok(false)
}
