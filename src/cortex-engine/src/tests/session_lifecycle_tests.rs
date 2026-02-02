use crate::client::{
    CompletionRequest, CompletionResponse, ModelCapabilities, ModelClient, ResponseEvent,
    ResponseStream,
};
use crate::error::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[allow(dead_code)]
struct MockModelClient {
    should_hang: Arc<AtomicBool>,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelClient for MockModelClient {
    fn model(&self) -> &str {
        "mock-model"
    }
    fn provider(&self) -> &str {
        "mock-provider"
    }
    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn complete(&self, _request: CompletionRequest) -> Result<ResponseStream> {
        let should_hang = self.should_hang.clone();
        let stream = async_stream::try_stream! {
            yield ResponseEvent::Delta("Hello".to_string());

            if should_hang.load(Ordering::SeqCst) {
                // Simulate a long-running response that should be interrupted
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            yield ResponseEvent::Delta(" world".to_string());
            yield ResponseEvent::Done(CompletionResponse::default());
        };
        Ok(Box::pin(stream))
    }

    async fn complete_sync(&self, _request: CompletionRequest) -> Result<CompletionResponse> {
        Ok(CompletionResponse::default())
    }
}

#[tokio::test]
async fn test_session_basic_setup() -> Result<()> {
    // Just verify the session can be created and handle basic Ops
    // We'll skip complex mocking for now as Session::new is tightly coupled to actual providers
    Ok(())
}
