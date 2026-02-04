//! Cortex Backend Client
//!
//! Client for the Cortex Backend API with:
//! - OAuth authentication (device code flow)
//! - Responses API (streaming SSE)
//! - Credit system with price verification

use std::time::Duration;

use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::timeout;
use tokio_stream::wrappers::ReceiverStream;

use super::{
    CompletionRequest, CompletionResponse, FinishReason, Message, MessageContent, MessageRole,
    ModelCapabilities, ModelClient, ResponseEvent, ResponseStream, TokenUsage, ToolCallEvent,
};
use crate::api_client::create_streaming_client;
use crate::error::{CortexError, Result};

const DEFAULT_CORTEX_URL: &str = "https://api.cortex.foundation";

/// Timeout in seconds for receiving individual SSE chunks during streaming.
/// If no data is received within this duration, the connection is terminated
/// to prevent indefinite hangs when connections stall mid-stream.
const CHUNK_TIMEOUT_SECS: u64 = 60;

/// Pricing information for a model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingInfo {
    pub credit_multiplier_input: f64,
    pub credit_multiplier_output: f64,
    pub price_version: i32,
}

/// Model information from Cortex API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexModel {
    pub id: String,
    #[serde(default)]
    pub object: String,
    #[serde(default)]
    pub created: i64,
    #[serde(default)]
    pub owned_by: String,
    pub display_name: String,
    pub context_length: i32,
    pub max_output_tokens: i32,
    #[serde(default)]
    pub capabilities: serde_json::Value,
    pub credit_multiplier_input: String,
    pub credit_multiplier_output: String,
    #[serde(default)]
    pub credit_multiplier_cached_input: String,
    pub price_version: i32,
}

/// Cortex Backend client.
pub struct CortexClient {
    client: Client,
    base_url: String,
    model: String,
    capabilities: ModelCapabilities,
    /// OAuth access token (for Cortex provider)
    auth_token: Option<String>,
    /// API key (for custom Responses API providers)
    api_key: Option<String>,
    /// Expected price version (for price verification)
    expected_price_version: Option<i32>,
}

impl CortexClient {
    /// Create a new Cortex client.
    pub fn new(model: String, base_url: Option<String>) -> Self {
        // Always use create_streaming_client to ensure User-Agent is set
        // Fall back to a client with explicit User-Agent if that fails
        let client = create_streaming_client().unwrap_or_else(|e| {
            tracing::warn!("Failed to create streaming client: {}, using fallback", e);
            Client::builder()
                .user_agent(crate::api_client::USER_AGENT)
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .unwrap_or_else(|_| Client::new())
        });

        Self {
            client,
            base_url: base_url.unwrap_or_else(|| {
                std::env::var("CORTEX_API_URL").unwrap_or_else(|_| DEFAULT_CORTEX_URL.to_string())
            }),
            model,
            capabilities: ModelCapabilities {
                vision: true,
                tools: true,
                reasoning: true,
                context_window: 200_000,
                max_output_tokens: Some(8192),
            },
            auth_token: None,
            api_key: None,
            expected_price_version: None,
        }
    }

    /// Set OAuth access token.
    pub fn with_auth_token(mut self, token: String) -> Self {
        self.auth_token = Some(token);
        self
    }

    /// Set API key (for custom providers using Responses API).
    pub fn with_api_key(mut self, key: String) -> Self {
        self.api_key = Some(key);
        self
    }

    /// Set expected price version for verification.
    pub fn with_expected_price_version(mut self, version: i32) -> Self {
        self.expected_price_version = Some(version);
        self
    }

    /// Get authorization header value.
    /// Uses centralized auth module with priority: instance token → api_key → env var → keyring
    fn auth_header(&self) -> Option<String> {
        // Combine instance token and api_key as override
        let instance_override = self.auth_token.as_deref().or(self.api_key.as_deref());
        crate::auth_token::auth_header(instance_override)
    }

    /// Check if the backend is available.
    pub async fn health_check(&self) -> Result<bool> {
        use crate::api_client::HEALTH_CHECK_TIMEOUT;
        let url = format!("{}/health", self.base_url);
        let resp = self
            .client
            .get(&url)
            .timeout(HEALTH_CHECK_TIMEOUT)
            .send()
            .await
            .map_err(|e| CortexError::BackendUnavailable(e.to_string()))?;
        Ok(resp.status().is_success())
    }

    /// List available models with pricing.
    pub async fn list_models(&self) -> Result<Vec<CortexModel>> {
        let url = format!("{}/v1/models", self.base_url);
        let user_agent = crate::api_client::USER_AGENT;

        tracing::debug!(url = %url, user_agent = %user_agent, has_auth = self.auth_token.is_some(), "Fetching models");

        let mut req = self
            .client
            .get(&url)
            .header(reqwest::header::USER_AGENT, user_agent);
        if let Some(auth) = self.auth_header() {
            tracing::debug!("Adding Authorization header");
            req = req.header("Authorization", auth);
        }

        let resp = req.send().await.map_err(|e| {
            tracing::error!(error = %e, url = %url, "Request failed");
            CortexError::BackendUnavailable(e.to_string())
        })?;

        tracing::debug!(status = %resp.status(), "Response received");

        if !resp.status().is_success() {
            let status = resp.status();
            let is_json = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("application/json"))
                .unwrap_or(false);

            let body = resp.text().await.unwrap_or_default();

            let message = if is_json {
                serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error")?.get("message")?.as_str().map(String::from))
                    .unwrap_or_else(|| format!("Failed to list models ({}): {}", status, body))
            } else {
                // Include response body for debugging
                let body_preview = if body.len() > 200 {
                    format!("{}...", &body[..200])
                } else {
                    body.clone()
                };
                format!("HTTP {} from {}: {}", status, self.base_url, body_preview)
            };

            tracing::error!(status = %status, url = %self.base_url, body = %body, "list_models failed");
            return Err(CortexError::BackendError { message });
        }

        #[derive(Deserialize)]
        struct ModelsResponse {
            data: Vec<CortexModel>,
        }

        let data: ModelsResponse = resp.json().await.map_err(|e| CortexError::BackendError {
            message: format!("Failed to parse models response: {}", e),
        })?;

        Ok(data.data)
    }

    /// Build the Responses API request body.
    fn build_request(&self, request: &CompletionRequest) -> ResponsesRequest {
        let input: Vec<InputMessage> = request
            .messages
            .iter()
            .map(|m| {
                let role = match m.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                };

                // Log tool messages for debugging
                if m.role == MessageRole::Tool {
                    tracing::info!(
                        role = %role,
                        tool_call_id = ?m.tool_call_id,
                        content_len = m.content.as_text().map(|s| s.len()).unwrap_or(0),
                        "Building tool result message"
                    );
                }
                if m.tool_calls.is_some() {
                    tracing::info!(
                        role = %role,
                        tool_calls_count = m.tool_calls.as_ref().map(|t| t.len()).unwrap_or(0),
                        "Building assistant message with tool_calls"
                    );
                }

                // Convert tool_calls from Message to InputToolCall format
                let tool_calls = m.tool_calls.as_ref().map(|tcs| {
                    tcs.iter()
                        .map(|tc| InputToolCall {
                            id: tc.id.clone(),
                            call_type: "function".to_string(),
                            function: InputToolCallFunction {
                                name: tc.function.name.clone(),
                                arguments: tc.function.arguments.clone(),
                            },
                        })
                        .collect()
                });

                InputMessage {
                    role: role.to_string(),
                    content: m.content.as_text().unwrap_or("").to_string(),
                    tool_call_id: m.tool_call_id.clone(),
                    tool_calls,
                }
            })
            .collect();

        // Convert tools to request format
        let tools = if request.tools.is_empty() {
            None
        } else {
            Some(
                request
                    .tools
                    .iter()
                    .map(|t| RequestToolDefinition {
                        tool_type: "function".to_string(),
                        name: t.function.name.clone(),
                        description: Some(t.function.description.clone()),
                        parameters: Some(t.function.parameters.clone()),
                    })
                    .collect(),
            )
        };

        // Set tool_choice to "auto" if tools are provided
        let tool_choice = if tools.is_some() {
            Some("auto".to_string())
        } else {
            None
        };

        ResponsesRequest {
            model: request.model.clone(),
            input: ResponsesInput::Messages(input),
            instructions: None,
            max_output_tokens: request.max_tokens.map(|t| t as i32),
            temperature: request.temperature,
            expected_price_version: self.expected_price_version,
            tools,
            tool_choice,
        }
    }
}

// =============================================================================
// RESPONSES API TYPES
// =============================================================================

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: ResponsesInput,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expected_price_version: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<RequestToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<String>,
}

/// Tool definition for the Responses API request
#[derive(Debug, Serialize)]
struct RequestToolDefinition {
    #[serde(rename = "type")]
    tool_type: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
#[allow(dead_code)]
enum ResponsesInput {
    Text(String),
    Messages(Vec<InputMessage>),
}

#[derive(Debug, Serialize)]
struct InputMessage {
    role: String,
    content: String,
    /// Tool call ID - required for tool result messages
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
    /// Tool calls - for assistant messages that made function calls
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<InputToolCall>>,
}

/// Tool call for serialization in InputMessage
#[derive(Debug, Serialize)]
struct InputToolCall {
    id: String,
    #[serde(rename = "type")]
    call_type: String,
    function: InputToolCallFunction,
}

#[derive(Debug, Serialize)]
struct InputToolCallFunction {
    name: String,
    arguments: String,
}

/// Output item from the response (message or function call)
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum OutputItem {
    #[serde(rename = "message")]
    Message {
        id: String,
        role: String,
        status: String,
    },
    #[serde(rename = "function_call")]
    FunctionCall {
        id: String,
        call_id: String,
        name: String,
        arguments: String,
        status: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
#[allow(dead_code)]
enum CortexResponseEvent {
    #[serde(rename = "response.created")]
    ResponseCreated { response: ResponseObject },

    #[serde(rename = "response.output_item.added")]
    OutputItemAdded { output_index: u32, item: OutputItem },

    #[serde(rename = "response.output_item.done")]
    OutputItemDone { output_index: u32, item: OutputItem },

    #[serde(rename = "response.output_text.delta")]
    OutputTextDelta {
        output_index: u32,
        content_index: u32,
        delta: String,
    },

    #[serde(rename = "response.output_text.done")]
    OutputTextDone {
        output_index: u32,
        content_index: u32,
        text: String,
    },

    #[serde(rename = "response.completed")]
    ResponseCompleted {
        response: ResponseObject,
        credits_used: i64,
        pricing: PricingInfo,
    },

    #[serde(rename = "response.failed")]
    ResponseFailed {
        response: ResponseObject,
        error: ResponseErrorInfo,
    },

    #[serde(rename = "response.price_changed")]
    PriceChanged {
        expected_version: i32,
        current_version: i32,
        current_pricing: PricingInfo,
    },

    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ResponseObject {
    id: String,
    status: String,
    #[serde(default)]
    usage: Option<UsageInfo>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct UsageInfo {
    input_tokens: i32,
    output_tokens: i32,
    total_tokens: i32,
    #[serde(default)]
    credits_used: i64,
}

#[derive(Debug, Deserialize)]
struct ResponseErrorInfo {
    code: String,
    message: String,
}

// =============================================================================
// MODEL CLIENT IMPLEMENTATION
// =============================================================================

#[async_trait]
impl ModelClient for CortexClient {
    fn model(&self) -> &str {
        &self.model
    }

    fn provider(&self) -> &str {
        "cortex"
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn complete(&self, request: CompletionRequest) -> Result<ResponseStream> {
        let url = format!("{}/v1/responses", self.base_url);
        let body = self.build_request(&request);

        // Explicitly set all required headers including User-Agent
        let user_agent = crate::api_client::USER_AGENT;
        let mut req = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .header(reqwest::header::USER_AGENT, user_agent);

        let auth = self.auth_header();

        // Log everything we're sending
        tracing::info!(
            url = %url,
            user_agent = %user_agent,
            content_type = "application/json",
            accept = "text/event-stream",
            has_auth = auth.is_some(),
            auth_prefix = auth.as_ref().map(|a| &a[..20.min(a.len())]),
            body_model = %request.model,
            ">>> OUTGOING REQUEST"
        );

        if let Some(auth) = auth {
            req = req.header("Authorization", auth);
        } else {
            tracing::warn!("No authorization header - request will likely fail with 401");
        }

        // Log the full request body for debugging
        if let Ok(body_json) = serde_json::to_string(&body) {
            tracing::info!(body = %body_json, "Request body");
        }

        let resp = req.json(&body).send().await.map_err(|e| {
            tracing::error!(error = %e, "Failed to send request");
            CortexError::from_reqwest_with_proxy_check(e, &url)
        })?;

        tracing::info!(
            status = %resp.status(),
            headers = ?resp.headers(),
            "<<< RESPONSE RECEIVED"
        );

        if !resp.status().is_success() {
            let status = resp.status();
            let is_json = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .map(|ct| ct.contains("application/json"))
                .unwrap_or(false);

            let body = resp.text().await.unwrap_or_default();

            let message = if is_json {
                serde_json::from_str::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|v| v.get("error")?.get("message")?.as_str().map(String::from))
                    .unwrap_or_else(|| format!("API error {}: {}", status, body))
            } else {
                // Include response body for debugging
                let body_preview = if body.len() > 200 {
                    format!("{}...", &body[..200])
                } else {
                    body.clone()
                };
                format!("HTTP {} from {}: {}", status, self.base_url, body_preview)
            };

            tracing::error!(status = %status, url = %self.base_url, body = %body, "API request failed");
            return Err(CortexError::BackendError { message });
        }

        // Create channel for streaming events
        let (tx, rx) = mpsc::channel::<Result<ResponseEvent>>(100);

        // Spawn task to process SSE stream
        let stream = resp.bytes_stream().eventsource();
        tokio::spawn(async move {
            let mut stream = std::pin::pin!(stream);
            let mut accumulated_text = String::new();
            let mut usage = TokenUsage::default();
            let chunk_timeout = Duration::from_secs(CHUNK_TIMEOUT_SECS);

            loop {
                // Apply per-chunk timeout to prevent indefinite hangs when connections stall
                let event_result = match timeout(chunk_timeout, stream.next()).await {
                    Ok(Some(result)) => result,
                    Ok(None) => break, // Stream ended normally
                    Err(_) => {
                        // Timeout elapsed - no data received within CHUNK_TIMEOUT_SECS
                        let _ = tx
                            .send(Err(CortexError::BackendError {
                                message: format!(
                                    "SSE chunk timeout - no data received for {} seconds",
                                    CHUNK_TIMEOUT_SECS
                                ),
                            }))
                            .await;
                        break;
                    }
                };
                match event_result {
                    Ok(event) => {
                        if event.data.is_empty() || event.data == "[DONE]" {
                            continue;
                        }

                        match serde_json::from_str::<CortexResponseEvent>(&event.data) {
                            Ok(cortex_event) => {
                                tracing::debug!(event_type = ?std::mem::discriminant(&cortex_event), "Received SSE event");
                                let response_event = match cortex_event {
                                    CortexResponseEvent::OutputTextDelta { delta, .. } => {
                                        tracing::info!(delta_len = delta.len(), "Got text delta");
                                        accumulated_text.push_str(&delta);
                                        Some(ResponseEvent::Delta(delta))
                                    }
                                    CortexResponseEvent::OutputItemDone { item, .. } => {
                                        // Handle function calls from output_item.done
                                        match item {
                                            OutputItem::FunctionCall {
                                                call_id,
                                                name,
                                                arguments,
                                                ..
                                            } => {
                                                tracing::debug!(
                                                    name = %name,
                                                    call_id = %call_id,
                                                    "Received function call from Cortex"
                                                );
                                                Some(ResponseEvent::ToolCall(ToolCallEvent {
                                                    id: call_id,
                                                    name,
                                                    arguments,
                                                }))
                                            }
                                            OutputItem::Message { .. } => None,
                                        }
                                    }
                                    CortexResponseEvent::ResponseCompleted {
                                        response,
                                        credits_used,
                                        ..
                                    } => {
                                        if let Some(u) = response.usage {
                                            usage = TokenUsage {
                                                input_tokens: u.input_tokens as i64,
                                                output_tokens: u.output_tokens as i64,
                                                total_tokens: u.total_tokens as i64,
                                            };
                                        }

                                        let completion = CompletionResponse {
                                            message: Some(Message {
                                                role: MessageRole::Assistant,
                                                content: MessageContent::Text(
                                                    accumulated_text.clone(),
                                                ),
                                                tool_call_id: None,
                                                tool_calls: None,
                                            }),
                                            usage: usage.clone(),
                                            finish_reason: FinishReason::Stop,
                                            tool_calls: vec![],
                                        };

                                        tracing::debug!(
                                            "Cortex response completed: {} credits used",
                                            credits_used
                                        );
                                        Some(ResponseEvent::Done(completion))
                                    }
                                    CortexResponseEvent::ResponseFailed { error, .. } => {
                                        Some(ResponseEvent::Error(format!(
                                            "{}: {}",
                                            error.code, error.message
                                        )))
                                    }
                                    CortexResponseEvent::PriceChanged {
                                        expected_version,
                                        current_version,
                                        current_pricing,
                                    } => Some(ResponseEvent::Error(format!(
                                        "PRICE_CHANGED:{}:{}:{}:{}",
                                        expected_version,
                                        current_version,
                                        current_pricing.credit_multiplier_input,
                                        current_pricing.credit_multiplier_output
                                    ))),
                                    _ => None,
                                };

                                if let Some(event) = response_event {
                                    if tx.send(Ok(event)).await.is_err() {
                                        break;
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!(
                                    "Failed to parse Cortex event: {} - {}",
                                    e,
                                    event.data
                                );
                            }
                        }
                    }
                    Err(e) => {
                        let _ = tx
                            .send(Err(CortexError::BackendError {
                                message: format!("Stream error: {}", e),
                            }))
                            .await;
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    async fn complete_sync(&self, request: CompletionRequest) -> Result<CompletionResponse> {
        let mut stream = self.complete(request).await?;
        let mut response = CompletionResponse::default();
        let mut text = String::new();

        while let Some(event_result) = stream.next().await {
            match event_result? {
                ResponseEvent::Delta(delta) => {
                    text.push_str(&delta);
                }
                ResponseEvent::Done(completion) => {
                    response = completion;
                }
                ResponseEvent::Error(err) => {
                    return Err(CortexError::BackendError { message: err });
                }
                _ => {}
            }
        }

        if response.message.is_none() && !text.is_empty() {
            response.message = Some(Message {
                role: MessageRole::Assistant,
                content: MessageContent::Text(text),
                tool_call_id: None,
                tool_calls: None,
            });
        }

        Ok(response)
    }
}
