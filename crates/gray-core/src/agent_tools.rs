//! Tool-call plumbing for the agent loop (move-only split from `agent.rs`).
//!
//! [`PendingToolCall`] accumulates streamed argument deltas;
//! [`answer_pending_tools`] backfills synthetic results for calls that never
//! ran so history never holds an orphaned call.

use crate::agent::Agent;
use crate::message::{ContentBlock, Message, Role};

/// One synthetic `is_error` tool result: the shared push behind every
/// backfill so orphaned calls never brick the transcript. Message only, no
/// event — mirrors the long-standing cancel-path convention.
pub(crate) fn push_synthetic(agent: &mut Agent, id: &str, reason: &str) {
    agent.messages.push(Message {
        role: Role::User,
        content: vec![ContentBlock::ToolResult {
            id: id.to_string(),
            content: format!("[{reason}]"),
            is_error: true,
        }],
    });
}

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
        push_synthetic(agent, id, reason);
    }
}

/// [`answer_pending_tools`] over `[from_idx, to_idx)`: backfills synthetic
/// results for one parallel run's uncompleted calls on cancellation.
pub(crate) fn answer_pending_range(
    agent: &mut Agent,
    tool_uses: &[(String, String, serde_json::Value)],
    from_idx: usize,
    to_idx: usize,
    reason: &str,
) {
    for (id, _, _) in tool_uses
        .iter()
        .skip(from_idx)
        .take(to_idx.saturating_sub(from_idx))
    {
        push_synthetic(agent, id, reason);
    }
}

/// A partially-streamed tool call awaiting its `MessageComplete`.
#[derive(Default)]
pub(crate) struct PendingToolCall {
    pub(crate) id: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) arguments: String,
    /// True once the single `tool_call_start` has been emitted (requires
    /// both id and name non-blank). Drives live progress gating so dispatch
    /// never needs a positional side table.
    pub(crate) started: bool,
}

impl PendingToolCall {
    /// Parses accumulated argument JSON; unparseable fragments degrade to a
    /// string payload rather than aborting the run.
    pub(crate) fn parsed_args(&self) -> serde_json::Value {
        if self.arguments.is_empty() {
            return serde_json::Value::Null;
        }
        let parsed = serde_json::from_str(&self.arguments)
            .or_else(|_| serde_json::from_str(&escape_raw_controls(&self.arguments)));
        match parsed {
            // Some models double-encode: the arguments are a JSON string
            // whose content is the real object.
            Ok(serde_json::Value::String(inner)) => serde_json::from_str(&inner)
                .ok()
                .filter(serde_json::Value::is_object)
                .unwrap_or(serde_json::Value::String(inner)),
            Ok(v) => v,
            Err(_) => serde_json::Value::String(self.arguments.clone()),
        }
    }
}

/// Escapes raw control characters inside JSON string literals (models often
/// emit literal newlines/tabs in a `command` value, which strict JSON rejects).
fn escape_raw_controls(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let (mut in_str, mut escaped) = (false, false);
    for c in s.chars() {
        if in_str && !escaped && (c as u32) < 0x20 {
            match c {
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                _ => out.push_str(&format!("\\u{:04x}", c as u32)),
            }
            continue;
        }
        out.push(c);
        if escaped {
            escaped = false;
        } else if in_str && c == '\\' {
            escaped = true;
        } else if c == '"' {
            in_str = !in_str;
        }
    }
    out
}

#[cfg(test)]
mod parse_tests {
    use super::PendingToolCall;

    fn parse(raw: &str) -> serde_json::Value {
        PendingToolCall {
            arguments: raw.into(),
            ..Default::default()
        }
        .parsed_args()
    }

    #[test]
    fn raw_newlines_in_strings_are_repaired() {
        let v = parse("{\"command\": \"printf 'a\\\\n'\nls\tx\"}");
        assert_eq!(v["command"], "printf 'a\\n'\nls\tx");
    }

    #[test]
    fn double_encoded_object_is_unwrapped() {
        let v = parse(r#""{\"command\":\"ls\"}""#);
        assert_eq!(v["command"], "ls");
    }

    #[test]
    fn garbage_stays_a_string() {
        assert!(parse("{\"command\": ").is_string());
    }
}
