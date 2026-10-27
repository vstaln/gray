use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: usize,

    #[serde(default)]
    pub output_tokens: usize,

    #[serde(default)]
    pub reasoning_tokens: usize,

    #[serde(default)]
    pub cached_tokens: usize,

    #[serde(default)]
    pub non_cached_input_tokens: usize,

    #[serde(default)]
    pub cache_read_input_tokens: usize,

    #[serde(default)]
    pub cache_write_input_tokens: usize,

    #[serde(default)]
    pub total_tokens: usize,
}

impl Usage {
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

    pub fn estimated_context(tokens: usize) -> Self {
        Self {
            input_tokens: tokens,
            non_cached_input_tokens: tokens,
            total_tokens: tokens,
            ..Self::default()
        }
    }

    pub fn any(&self) -> bool {
        self.input_tokens != 0
            || self.output_tokens != 0
            || self.cached_tokens != 0
            || self.non_cached_input_tokens != 0
            || self.cache_read_input_tokens != 0
            || self.cache_write_input_tokens != 0
            || self.reasoning_tokens != 0
            || self.total_tokens != 0
    }

    pub fn log_request(&self, kind: &str, session: Option<&str>) {
        let mut u = *self;
        u.normalize();
        log::info!(
            target: "gray_agent",
            "request usage: kind={kind} session={} in={} cache_read={} cache_write={} out={} reasoning={}",
            session.filter(|s| !s.is_empty()).unwrap_or("-"),
            u.input_tokens,
            u.cache_read_input_tokens,
            u.cache_write_input_tokens,
            u.output_tokens,
            u.reasoning_tokens
        );
    }

    pub fn accumulate(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.reasoning_tokens = self.reasoning_tokens.saturating_add(other.reasoning_tokens);
        self.cached_tokens = self.cached_tokens.saturating_add(other.cached_tokens);
        self.non_cached_input_tokens = self
            .non_cached_input_tokens
            .saturating_add(other.non_cached_input_tokens);
        self.cache_read_input_tokens = self
            .cache_read_input_tokens
            .saturating_add(other.cache_read_input_tokens);
        self.cache_write_input_tokens = self
            .cache_write_input_tokens
            .saturating_add(other.cache_write_input_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }

    pub fn total(&self) -> usize {
        if self.total_tokens != 0 {
            self.total_tokens
        } else {
            self.input_tokens + self.output_tokens
        }
    }

    pub fn cache_hit_rate(&self) -> f64 {
        if self.input_tokens == 0 {
            0.0
        } else {
            self.cache_read_input_tokens.max(self.cached_tokens) as f64 / self.input_tokens as f64
        }
    }

    pub fn normalize(&mut self) {
        if self.cached_tokens == 0 && self.cache_read_input_tokens != 0 {
            self.cached_tokens = self.cache_read_input_tokens;
        } else if self.cache_read_input_tokens == 0 && self.cached_tokens != 0 {
            self.cache_read_input_tokens = self.cached_tokens;
        }

        if self.non_cached_input_tokens == 0 && self.input_tokens != 0 {
            let known = self.cache_read_input_tokens + self.cache_write_input_tokens;
            if known <= self.input_tokens {
                self.non_cached_input_tokens = self.input_tokens - known;
            }
        }

        if self.total_tokens == 0 && (self.input_tokens != 0 || self.output_tokens != 0) {
            self.total_tokens = self.input_tokens + self.output_tokens;
        }
    }
}

pub fn append_thinking_chunk(buf: &mut String, chunk: &str) {
    if let (Some(prev), Some(next)) = (buf.chars().next_back(), chunk.chars().next())
        && needs_thinking_boundary_space(prev, next)
    {
        buf.push(' ');
    }
    buf.push_str(chunk);
}

fn needs_thinking_boundary_space(prev: char, next: char) -> bool {
    if prev.is_whitespace() || next.is_whitespace() {
        return false;
    }
    if !matches!(prev, '.' | '?' | '!' | ',' | ';' | ':') {
        return false;
    }

    if matches!(
        next,
        '.' | '?' | '!' | ',' | ';' | ':' | ')' | ']' | '}' | '"' | '\'' | '`'
    ) {
        return false;
    }

    if next.is_ascii_digit() && matches!(prev, '.' | ',' | ':') {
        return false;
    }
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Start,

    TextDelta {
        #[serde(alias = "text")]
        delta: String,
    },

    ThinkingDelta {
        #[serde(alias = "text")]
        delta: String,
    },

    ToolCallStart {
        id: String,
        name: String,
    },

    ToolCallProgress {
        id: String,
        name: String,
        args_so_far: String,
    },

    ToolCallEnd {
        id: String,
        args: serde_json::Value,
    },

    ToolResult {
        id: String,
        #[serde(alias = "content")]
        output: String,
        #[serde(default)]
        is_error: bool,
    },

    StepUsage {
        usage: Usage,
    },

    /// The prompt prefix differs from the previous request's, so the
    /// provider's prompt cache cannot be reused past that point. Sent before
    /// the request; `cause` is plain language ("tool list changed").
    PrefixChanged {
        cause: String,
    },

    StreamError {
        message: String,
        #[serde(default)]
        details: String,
    },

    MessageBoundary,

    Steered {
        text: String,
    },

    Retract,

    TurnEnd {
        stop_reason: StopReason,
        usage: Usage,
    },

    Compacted {
        tokens_before: usize,
        tokens_after: usize,
        messages_before: usize,
        messages_after: usize,

        #[serde(default)]
        reason: CompactReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactReason {
    #[default]
    Window,

    Overflow,

    UserCeiling,

    PriceCliff {
        cliff: usize,
    },
}

impl AgentEvent {
    pub fn text_delta(delta: impl Into<String>) -> Self {
        Self::TextDelta {
            delta: delta.into(),
        }
    }

    pub fn thinking_delta(delta: impl Into<String>) -> Self {
        Self::ThinkingDelta {
            delta: delta.into(),
        }
    }

    pub fn tool_call_start(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self::ToolCallStart {
            id: id.into(),
            name: name.into(),
        }
    }

    pub fn tool_call_progress(
        id: impl Into<String>,
        name: impl Into<String>,
        args_so_far: impl Into<String>,
    ) -> Self {
        Self::ToolCallProgress {
            id: id.into(),
            name: name.into(),
            args_so_far: args_so_far.into(),
        }
    }

    pub fn tool_call_end(id: impl Into<String>, args: serde_json::Value) -> Self {
        Self::ToolCallEnd {
            id: id.into(),
            args,
        }
    }

    pub fn tool_result(id: impl Into<String>, output: impl Into<String>, is_error: bool) -> Self {
        Self::ToolResult {
            id: id.into(),
            output: output.into(),
            is_error,
        }
    }

    pub fn turn_end(stop_reason: StopReason, usage: Usage) -> Self {
        Self::TurnEnd { stop_reason, usage }
    }

    pub fn compacted(
        tokens_before: usize,
        tokens_after: usize,
        messages_before: usize,
        messages_after: usize,
        reason: CompactReason,
    ) -> Self {
        Self::Compacted {
            tokens_before,
            tokens_after,
            messages_before,
            messages_after,
            reason,
        }
    }

    pub fn stream_error(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self::StreamError {
            message: message.into(),
            details: details.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    TextDelta {
        #[serde(alias = "text")]
        delta: String,
    },

    ThinkingDelta {
        #[serde(alias = "text")]
        delta: String,
    },

    ReasoningItem {
        item_id: String,
        encrypted_content: String,
    },

    ToolCallDelta {
        index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, alias = "arguments", alias = "args_delta")]
        arguments_delta: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thought_signature: Option<String>,
    },

    MessageComplete {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        stop_reason: Option<StopReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
    },

    StreamError {
        message: String,
        #[serde(default)]
        details: String,
    },
}

impl StreamEvent {
    pub fn text_delta(delta: impl Into<String>) -> Self {
        Self::TextDelta {
            delta: delta.into(),
        }
    }

    pub fn thinking_delta(delta: impl Into<String>) -> Self {
        Self::ThinkingDelta {
            delta: delta.into(),
        }
    }

    pub fn reasoning_item(
        item_id: impl Into<String>,
        encrypted_content: impl Into<String>,
    ) -> Self {
        Self::ReasoningItem {
            item_id: item_id.into(),
            encrypted_content: encrypted_content.into(),
        }
    }

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
            thought_signature: None,
        }
    }

    pub fn message_complete(stop_reason: Option<StopReason>, usage: Option<Usage>) -> Self {
        Self::MessageComplete { stop_reason, usage }
    }

    pub fn stream_error(message: impl Into<String>, details: impl Into<String>) -> Self {
        Self::StreamError {
            message: message.into(),
            details: details.into(),
        }
    }
}
