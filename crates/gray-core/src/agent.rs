//! Shared execution contracts: the seams between gray-core and the
//! provider/tools leaves. The binary wires implementations together.

use std::path::PathBuf;

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::CoreError;
use crate::event::StreamEvent;
use crate::message::ChatRequest;

/// Errors surfaced by a provider implementation.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("stream broken: {0}")]
    Stream(String),
}

/// Output of a tool execution. Errors are data for the model, not crashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }
}

/// Per-execution context handed to tools.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancel: CancellationToken,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self { cwd: PathBuf::from("."), cancel: CancellationToken::new() }
    }
}

/// A streaming LLM provider (wire protocol behind this seam).
#[async_trait]
pub trait Provider: Send + Sync {
    fn stream(
        &self,
        req: ChatRequest,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>>;
}

/// Executes named tools. The registry lives behind this seam so core
/// never knows what tools exist.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput>;
}

/// Convenience alias used by Agent wiring.
pub type ProviderStream = BoxStream<'static, Result<StreamEvent, ProviderError>>;

impl From<ProviderError> for CoreError {
    fn from(e: ProviderError) -> Self {
        CoreError::Provider(e.to_string())
    }
}
