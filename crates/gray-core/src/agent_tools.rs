//! Tool-call plumbing for the agent loop (move-only split from `agent.rs`).
//!
//! [`PendingToolCall`] accumulates streamed argument deltas;
//! [`answer_pending_tools`] backfills synthetic results for calls that never
//! ran so history never holds an orphaned call.

use crate::agent::Agent;
use crate::message::{ContentBlock, Message, Role};

/// Synthetic tool results for calls that never ran (cancellation, loop
/// abort). History must never contain a `function_call` without its output:
/// strict providers 400 on the orphan and the session bricks permanently.
pub(crate) fn answer_pending_tools(
    agent: &mut Agent,
    tool_uses: &[(String, String, serde_json::Value)],
    from_idx: usize,
    reason: &str,
) {
    for (id, _, _) in tool_uses.iter().skip(from_idx) {
        agent.messages.push(Message {
            role: Role::User,
            content: vec![ContentBlock::ToolResult {
                id: id.clone(),
                content: format!("[{reason}]"),
                is_error: true,
            }],
        });
    }
}

/// A partially-streamed tool call awaiting its `MessageComplete`.
#[derive(Default)]
pub(crate) struct PendingToolCall {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) arguments: String,
}

impl PendingToolCall {
    /// Parses accumulated argument JSON; unparseable fragments degrade to a
    /// string payload rather than aborting the run.
    pub(crate) fn parsed_args(&self) -> serde_json::Value {
        if self.arguments.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&self.arguments)
                .unwrap_or(serde_json::Value::String(self.arguments.clone()))
        }
    }
}
