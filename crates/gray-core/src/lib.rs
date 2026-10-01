pub mod agent;
mod agent_compact;
mod agent_loop;
mod agent_tools;
mod compact;
pub mod credential;
pub mod error;
pub mod event;
pub mod input;
pub mod message;
pub mod parallel;
pub mod paths;
pub mod redaction;
pub mod spill;
pub mod squeeze;
pub mod tool_out;

pub use agent::{
    Agent, CommandOutcome, PluginCommand, PluginHooks, Provider, ProviderError, ProviderStream,
    ToolBefore, ToolContext, ToolExecutor, ToolOutput,
};
pub use error::{CoreError, Result};
pub use event::{AgentEvent, StopReason, StreamEvent, Usage};
pub use message::{ChatRequest, ContentBlock, Message, Role, ToolDef};

#[cfg(test)]
#[path = "credential_tests.rs"]
mod credential_tests;

#[cfg(test)]
#[path = "input_tests.rs"]
mod input_tests;

#[cfg(test)]
#[path = "squeeze_tests.rs"]
mod squeeze_tests;

#[cfg(test)]
#[path = "spill_tests.rs"]
mod spill_tests;
