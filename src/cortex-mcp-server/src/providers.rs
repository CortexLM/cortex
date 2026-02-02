//! Resource and Prompt provider traits and implementations.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use cortex_mcp_types::{GetPromptResult, Prompt, Resource, ResourceContent};

// ============================================================================
// Resource Provider Trait
// ============================================================================

/// Trait for implementing resource providers.
#[async_trait::async_trait]
pub trait ResourceProvider: Send + Sync {
    /// List available resources.
    async fn list(&self) -> Result<Vec<Resource>>;

    /// Read a resource by URI.
    async fn read(&self, uri: &str) -> Result<ResourceContent>;
}

/// A static resource provider with predefined resources.
pub struct StaticResourceProvider {
    resources: HashMap<String, (Resource, String)>,
}

impl StaticResourceProvider {
    /// Create a new static resource provider.
    pub fn new() -> Self {
        Self {
            resources: HashMap::new(),
        }
    }

    /// Add a text resource.
    pub fn add_text(&mut self, resource: Resource, content: impl Into<String>) {
        let uri = resource.uri.clone();
        self.resources.insert(uri, (resource, content.into()));
    }
}

impl Default for StaticResourceProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl ResourceProvider for StaticResourceProvider {
    async fn list(&self) -> Result<Vec<Resource>> {
        Ok(self.resources.values().map(|(r, _)| r.clone()).collect())
    }

    async fn read(&self, uri: &str) -> Result<ResourceContent> {
        self.resources
            .get(uri)
            .map(|(r, content)| ResourceContent::text(&r.uri, content))
            .ok_or_else(|| anyhow!("Resource not found: {uri}"))
    }
}

// ============================================================================
// Prompt Provider Trait
// ============================================================================

/// Trait for implementing prompt providers.
#[async_trait::async_trait]
pub trait PromptProvider: Send + Sync {
    /// List available prompts.
    async fn list(&self) -> Result<Vec<Prompt>>;

    /// Get a prompt with arguments.
    async fn get(
        &self,
        name: &str,
        arguments: Option<HashMap<String, String>>,
    ) -> Result<GetPromptResult>;
}

/// A static prompt provider with predefined prompts.
pub struct StaticPromptProvider {
    prompts: HashMap<
        String,
        (
            Prompt,
            Box<dyn Fn(Option<HashMap<String, String>>) -> GetPromptResult + Send + Sync>,
        ),
    >,
}

impl StaticPromptProvider {
    /// Create a new static prompt provider.
    pub fn new() -> Self {
        Self {
            prompts: HashMap::new(),
        }
    }

    /// Add a prompt with a handler.
    pub fn add<F>(&mut self, prompt: Prompt, handler: F)
    where
        F: Fn(Option<HashMap<String, String>>) -> GetPromptResult + Send + Sync + 'static,
    {
        let name = prompt.name.clone();
        self.prompts.insert(name, (prompt, Box::new(handler)));
    }
}

impl Default for StaticPromptProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl PromptProvider for StaticPromptProvider {
    async fn list(&self) -> Result<Vec<Prompt>> {
        Ok(self.prompts.values().map(|(p, _)| p.clone()).collect())
    }

    async fn get(
        &self,
        name: &str,
        arguments: Option<HashMap<String, String>>,
    ) -> Result<GetPromptResult> {
        self.prompts
            .get(name)
            .map(|(_, handler)| handler(arguments))
            .ok_or_else(|| anyhow!("Prompt not found: {name}"))
    }
}
