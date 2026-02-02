use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};

#[tokio::main]
async fn main() {
    let client = reqwest::Client::builder()
        .user_agent("cortex-cli/0.1.0")
        .timeout(std::time::Duration::from_secs(300))
        .tcp_nodelay(true)
        .build()
        .unwrap();

    // Include tools like the CLI does
    let body = serde_json::json!({
        "model": "claude-opus-4-5-20251101",
        "input": [{"role": "user", "content": "hi"}],
        "stream": true,
        "tools": [
            {
                "type": "function",
                "name": "Read",
                "description": "Read file contents",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]}
            },
            {
                "type": "function",
                "name": "Write",
                "description": "Write to file",
                "parameters": {"type": "object", "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "required": ["path", "content"]}
            }
        ],
        "tool_choice": "auto"
    });

    println!("Sending request with tools...");

    let resp = client
        .post("https://api.cortex.foundation/v1/responses")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "text/event-stream")
        .header(USER_AGENT, "cortex-cli/0.1.0")
        .header(
            AUTHORIZATION,
            format!(
                "Bearer {}",
                std::env::var("CORTEX_API_KEY")
                    .expect("CORTEX_API_KEY environment variable required")
            ),
        )
        .json(&body)
        .send()
        .await
        .unwrap();

    println!("Status: {}", resp.status());
    let text = resp.text().await.unwrap();
    println!("Body: {}", &text[..text.len().min(500)]);
}
