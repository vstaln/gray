pub mod agent;
mod agent_compact;
mod agent_loop;
mod agent_tools;
mod background_compact;
pub mod cache_warm;
mod compact;
pub mod credential;
pub mod error;
pub mod event;
pub mod input;
pub mod message;
pub mod overflow;
pub mod parallel;
pub mod paths;
pub mod prefix_guard;
pub mod redaction;
pub mod spill;
pub mod time;
pub mod tool_out;

/// `skip_serializing_if` predicate for `bool` fields that default to false.
pub fn is_false(b: &bool) -> bool {
    !*b
}

pub use agent::{
    Agent, CommandOutcome, PanelPage, PanelSpan, PanelSpec, PanelStyle, PickerItem, PickerSpec,
    PluginCommand, PluginHooks, Provider, ProviderError, ProviderStream, ToolBefore, ToolContext,
    ToolExecutor, ToolOutput,
};
pub use compact::price_cliff_ceiling;
pub use error::{CoreError, Result};
pub use event::{AgentEvent, StopReason, StreamEvent, Usage};
pub use message::{ChatRequest, ContentBlock, Message, Role, ToolDef};

#[cfg(test)]
#[path = "credential_tests.rs"]
mod credential_tests;

#[cfg(test)]
#[path = "prefix_guard_tests.rs"]
mod prefix_guard_tests;
