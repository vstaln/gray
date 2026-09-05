pub mod agent;
mod agent_compact;
mod agent_loop;
mod agent_tools;
pub mod error;
pub mod event;
pub mod message;
pub mod questions;
pub mod tool_out;

pub use agent::{
    Agent, PluginCommand, PluginHooks, Provider, ProviderError, ProviderStream, ToolBefore,
    ToolContext, ToolExecutor, ToolOutput,
};
pub use error::{CoreError, Result};
pub use event::{AgentEvent, StopReason, StreamEvent, Usage};
pub use message::{ChatRequest, ContentBlock, Message, Role, ToolDef};
