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
pub mod kit;
pub mod kit_bridge;
pub mod message;
pub mod overflow;
pub mod parallel;
pub mod paths;
pub mod prefix_guard;
pub mod records;
pub mod redaction;
pub mod spill;
pub mod time;
pub mod tool_out;
pub use kit as toolbox;

/// `skip_serializing_if` predicate for `bool` fields that default to false.
pub fn is_false(b: &bool) -> bool {
    !*b
}

pub fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

pub use agent::{
    Agent, CommandOutcome, KeyAction, KeyPress, KeyReply, PanelPage, PanelSpan, PanelSpec,
    PanelStyle, PickerItem, PickerSpec, PluginCommand, PluginHooks, Provider, ProviderError,
    ProviderStream, ToolBefore, ToolContext, ToolExecutor, ToolOutput,
};
pub use compact::price_cliff_ceiling;
pub use error::CoreError;
pub use event::{AgentEvent, StopReason, StreamEvent, Usage};
pub use message::{ChatRequest, ContentBlock, Message, Role, ToolDef};

#[cfg(test)]
#[path = "credential_tests.rs"]
mod credential_tests;

#[cfg(test)]
#[path = "prefix_guard_tests.rs"]
mod prefix_guard_tests;
