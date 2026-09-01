pub mod agent;
pub mod delegation;
pub mod error;
pub mod event;
pub mod message;
pub mod questions;

pub use agent::{
    Agent, Provider, ProviderError, ProviderStream, ToolContext, ToolExecutor, ToolOutput,
};
pub use error::{CoreError, Result};
// Also provide Error as alias for CoreError
pub use error::CoreError as Error;
pub use event::{AgentEvent, StopReason, StreamEvent, Usage};
pub use message::{ChatRequest, ContentBlock, Message, Role, ToolDef};
