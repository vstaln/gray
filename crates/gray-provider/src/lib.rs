//! Streaming LLM provider implementations for the Gray agent framework.

pub mod openai;
#[cfg(test)]
pub mod testutil;

pub use gray_core::agent::{Provider, ProviderError, ProviderStream};
pub use gray_core::event::{StopReason, StreamEvent, Usage};
pub use gray_core::message::{ChatRequest, ContentBlock, Message, Role, ToolDef};
pub use openai::{OpenAiProvider, OpenAiProviderBuilder};
