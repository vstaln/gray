use serde::{Deserialize, Serialize};

/// Token usage summary for a turn or response — opencode v2 style.
///
/// Inclusive totals: `input_tokens`+`output_tokens` include cache/reasoning.
/// Breakdown (non-overlapping): `non_cached_input_tokens + cache_read + cache_write = input_tokens`,
/// `reasoning_tokens ≤ output_tokens`. Mirrors `LLM.Usage` in opencode `packages/llm/src/schema/events.ts:51`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Usage {
    /// Inclusive prompt tokens (includes cached reads/writes).
    #[serde(default)]
    pub input_tokens: usize,
    /// Inclusive output tokens (includes reasoning).
    #[serde(default)]
    pub output_tokens: usize,
    /// Tokens the model spent on reasoning / chain-of-thought (subset of
    /// `output_tokens` when reported). 0 when the provider doesn't say.
    #[serde(default)]
    pub reasoning_tokens: usize,
    /// Legacy: cache hits (equals `cache_read_input_tokens` for compat).
    #[serde(default)]
    pub cached_tokens: usize,
    /// Fresh prompt tokens (input - read - write).
    #[serde(default)]
    pub non_cached_input_tokens: usize,
    /// Input tokens served from cache (Anthropic `cache_read_input_tokens` / OpenAI `prompt_tokens_details.cached_tokens`).
    #[serde(default)]
    pub cache_read_input_tokens: usize,
    /// Input tokens written to cache (Anthropic `cache_creation_input_tokens`).
    #[serde(default)]
    pub cache_write_input_tokens: usize,
    /// Provider total if supplied, else `input+output`.
    #[serde(default)]
    pub total_tokens: usize,
}

impl Usage {
    /// Creates a new usage record (inclusive).
    pub fn new(input_tokens: usize, output_tokens: usize) -> Self {
        Self {
            input_tokens,
            output_tokens,
            reasoning_tokens: 0,
            cached_tokens: 0,
            non_cached_input_tokens: input_tokens,
            cache_read_input_tokens: 0,
            cache_write_input_tokens: 0,
            total_tokens: input_tokens + output_tokens,
        }
    }

    /// Visible output tokens = output - reasoning, clamped to 0 (opencode `visibleOutputTokens`).
    pub fn visible_output_tokens(&self) -> usize {
        self.output_tokens.saturating_sub(self.reasoning_tokens)
    }

    /// Computes the total tokens consumed — prefers provider `total_tokens` if set.
    pub fn total(&self) -> usize {
        if self.total_tokens != 0 {
            self.total_tokens
        } else {
            self.input_tokens + self.output_tokens
        }
    }

    /// Cache hit rate for prompt tokens (0.0–1.0) — uses cache_read.
    pub fn cache_hit_rate(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cache_read_input_tokens.max(self.cached_tokens) as f64 / self.input_tokens as f64
        }
    }

    /// Clamped subtract: max(0, total - sub) like opencode `ProviderShared.subtractTokens`.
    pub fn subtract_tokens(total: Option<usize>, sub: Option<usize>) -> Option<usize> {
        match (total, sub) {
            (None, _) => None,
            (Some(t), None) => Some(t),
            (Some(t), Some(s)) => Some(t.saturating_sub(s)),
        }
    }

    /// Sum optional tokens, None only if all None (opencode `sumTokens`).
    pub fn sum_tokens(vals: &[Option<usize>]) -> Option<usize> {
        if vals.iter().all(|v| v.is_none()) {
            None
        } else {
            Some(vals.iter().map(|v| v.unwrap_or(0)).sum())
        }
    }

    /// Normalize after filling breakdown fields — ensures invariants and legacy alias.
    pub fn normalize(&mut self) {
        // legacy alias
        if self.cached_tokens == 0 && self.cache_read_input_tokens != 0 {
            self.cached_tokens = self.cache_read_input_tokens;
        } else if self.cache_read_input_tokens == 0 && self.cached_tokens != 0 {
            self.cache_read_input_tokens = self.cached_tokens;
        }
        // derive non_cached if not set but input + caches known
        if self.non_cached_input_tokens == 0 && self.input_tokens != 0 {
            let known = self.cache_read_input_tokens + self.cache_write_input_tokens;
            if known <= self.input_tokens {
                self.non_cached_input_tokens = self.input_tokens - known;
            }
        }
        // derive total if not set
        if self.total_tokens == 0 && (self.input_tokens != 0 || self.output_tokens != 0) {
            self.total_tokens = self.input_tokens + self.output_tokens;
        }
    }
}

/// Reason why a turn or generation completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EndTurn => write!(f, "end_turn"),
            Self::ToolUse => write!(f, "tool_use"),
            Self::MaxTokens => write!(f, "max_tokens"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// High-level events emitted by the agent loop during turn execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// Indicates that an agent run / turn has started.
    Start,
    /// Incremental text delta generated by the model.
    TextDelta {
        #[serde(alias = "text")]
        delta: String,
    },
    /// Incremental reasoning / thinking delta generated by the model.
    ThinkingDelta {
        #[serde(alias = "text")]
        delta: String,
    },
    /// Notification that a tool call invocation has begun.
    ToolCallStart {
        id: String,
        name: String,
    },
    /// Complete tool call argument accumulation finished.
    ToolCallEnd {
        id: String,
        args: serde_json::Value,
    },
    /// Result returned after executing a tool call.
    ToolResult {
        id: String,
        #[serde(alias = "content")]
        output: String,
        #[serde(default)]
        is_error: bool,
    },
    /// Turn finished with a stop reason and token usage.
    TurnEnd {
        stop_reason: StopReason,
        usage: Usage,
    },
}

impl AgentEvent {
    /// Creates a text delta event.
    pub fn text_delta(delta: impl Into<String>) -> Self {
        Self::TextDelta {
            delta: delta.into(),
        }
    }

    /// Creates a thinking delta event.
    pub fn thinking_delta(delta: impl Into<String>) -> Self {
        Self::ThinkingDelta {
            delta: delta.into(),
        }
    }

    /// Creates a tool call start event.
    pub fn tool_call_start(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::ToolCallStart {
            id: id.into(),
            name: name.into(),
        }
    }

    /// Creates a tool call end event.
    pub fn tool_call_end(id: impl Into<String>, args: serde_json::Value) -> Self {
        Self::ToolCallEnd {
            id: id.into(),
            args,
        }
    }

    /// Creates a tool result event.
    pub fn tool_result(
        id: impl Into<String>,
        output: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::ToolResult {
            id: id.into(),
            output: output.into(),
            is_error,
        }
    }

    /// Creates a turn end event.
    pub fn turn_end(stop_reason: StopReason, usage: Usage) -> Self {
        Self::TurnEnd { stop_reason, usage }
    }
}

/// Provider-side stream event received during low-level model streaming.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    /// Text chunk streamed from the provider.
    TextDelta {
        #[serde(alias = "text")]
        delta: String,
    },
    /// Reasoning chunk streamed from the provider (`reasoning_content`).
    ThinkingDelta {
        #[serde(alias = "text")]
        delta: String,
    },
    /// Partial tool call chunk streamed from the provider.
    ToolCallDelta {
        index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, alias = "arguments", alias = "args_delta")]
        arguments_delta: String,
    },
    /// Stream completed with an optional stop reason and usage metrics.
    MessageComplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },
}

impl StreamEvent {
    /// Creates a stream text delta.
    pub fn text_delta(delta: impl Into<String>) -> Self {
        Self::TextDelta {
            delta: delta.into(),
        }
    }

    /// Creates a stream thinking delta.
    pub fn thinking_delta(delta: impl Into<String>) -> Self {
        Self::ThinkingDelta {
            delta: delta.into(),
        }
    }

    /// Creates a stream tool call delta.
    pub fn tool_call_delta(
        index: usize,
        id: Option<String>,
        name: Option<String>,
        arguments_delta: impl Into<String>,
    ) -> Self {
        Self::ToolCallDelta {
            index,
            id,
            name,
            arguments_delta: arguments_delta.into(),
        }
    }

    /// Creates a message complete event.
    pub fn message_complete(
        stop_reason: Option<StopReason>,
        usage: Option<Usage>,
    ) -> Self {
        Self::MessageComplete { stop_reason, usage }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_usage_serde_roundtrip() {
        let usage = Usage::new(150, 42);
        assert_eq!(usage.total(), 192);

        let json_str = serde_json::to_string(&usage).expect("serialize usage");
        let deserialized: Usage =
            serde_json::from_str(&json_str).expect("deserialize usage");
        assert_eq!(usage, deserialized);
    }

    #[test]
    fn test_stop_reason_serde_roundtrip() {
        for reason in [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::MaxTokens,
            StopReason::Cancelled,
            StopReason::Error,
        ] {
            let json_str = serde_json::to_string(&reason).expect("serialize stop reason");
            let deserialized: StopReason =
                serde_json::from_str(&json_str).expect("deserialize stop reason");
            assert_eq!(reason, deserialized);
        }
    }

    #[test]
    fn test_agent_events_serde_roundtrip() {
        let events = vec![
            AgentEvent::Start,
            AgentEvent::text_delta("Hello from assistant"),
            AgentEvent::tool_call_start("call_1", "read"),
            AgentEvent::tool_call_end("call_1", json!({"path": "src/lib.rs"})),
            AgentEvent::tool_result("call_1", "file contents here", false),
            AgentEvent::tool_result("call_2", "file not found", true),
            AgentEvent::turn_end(StopReason::EndTurn, Usage::new(100, 50)),
        ];

        for event in &events {
            let json_str = serde_json::to_string(event).expect("serialize agent event");
            let deserialized: AgentEvent =
                serde_json::from_str(&json_str).expect("deserialize agent event");
            assert_eq!(event, &deserialized);
        }
    }

    #[test]
    fn test_agent_event_aliases() {
        // Test text alias for delta
        let json_text = r#"{"type":"text_delta","text":"hello"}"#;
        let event: AgentEvent =
            serde_json::from_str(json_text).expect("deserialize text alias");
        assert_eq!(event, AgentEvent::text_delta("hello"));

        // Test content alias for output in tool_result
        let json_content = r#"{"type":"tool_result","id":"call_1","content":"done","is_error":false}"#;
        let event: AgentEvent =
            serde_json::from_str(json_content).expect("deserialize content alias");
        assert_eq!(event, AgentEvent::tool_result("call_1", "done", false));
    }

    #[test]
    fn test_stream_events_serde_roundtrip() {
        let events = vec![
            StreamEvent::text_delta("partial token"),
            StreamEvent::tool_call_delta(
                0,
                Some("call_1".to_string()),
                Some("bash".to_string()),
                "{\"command\":",
            ),
            StreamEvent::tool_call_delta(0, None, None, "\"ls\"}"),
            StreamEvent::message_complete(
                Some(StopReason::ToolUse),
                Some(Usage::new(200, 30)),
            ),
            StreamEvent::message_complete(None, None),
        ];

        for event in &events {
            let json_str = serde_json::to_string(event).expect("serialize stream event");
            let deserialized: StreamEvent =
                serde_json::from_str(&json_str).expect("deserialize stream event");
            assert_eq!(event, &deserialized);
        }
    }
}
