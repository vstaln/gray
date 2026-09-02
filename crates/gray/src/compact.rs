//! Context compaction and summarization for Gray conversations.
//!
//! Ported from the gold standard compaction architecture in pi / prime-agent:
//! serializes conversation history to plain text with truncated tool outputs,
//! prompts the LLM to produce a structured summary (Goal, Constraints, Progress Done/In Progress,
//! Key Decisions, Next Steps, Critical Context), and condenses the active conversation history.

use gray_core::agent::Agent;
use gray_core::error::CoreError;
use gray_core::event::Usage;
use gray_core::message::{ContentBlock, Message, Role};

use crate::config::Config;

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are a context summarization assistant. Your task is to read a conversation between a user and an AI coding assistant, then produce a structured, concise summary.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."#;

const BASE_SUMMARIZATION_PROMPT: &str = r#"The messages below are conversation messages from the current session.

Produce a structured summary of the conversation and work completed so far.

Use this EXACT format:

## Goal
[What is the user trying to accomplish? Can be multiple goals or tasks.]

## Constraints & Preferences
- [Any constraints, rules, user preferences, or requirements mentioned]
- [Or "(none)" if none were mentioned]

## Progress
### Done
- [x] [Completed tasks, modifications, files written, bugs fixed]

### In Progress
- [ ] [Current active work or pending tasks]

### Blocked / Issues
- [Issues or errors encountered, if any]

## Key Decisions & Architecture
- **[Decision]**: [Brief rationale]

## Next Steps
1. [Ordered list of what should happen next]

## Critical Context
- [Key file paths, function/struct names, URLs, port numbers, or error details needed to continue]

Keep each section concise. Preserve exact file paths, symbol names, and error messages."#;

pub fn build_summarization_prompt(transcript: &str, custom_instructions: Option<&str>) -> String {
    let mut prompt = format!(
        "{BASE_SUMMARIZATION_PROMPT}\n\n<conversation-transcript>\n{transcript}\n</conversation-transcript>"
    );
    if let Some(instructions) = custom_instructions {
        if !instructions.trim().is_empty() {
            prompt.push_str(&format!(
                "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority while keeping the section format above:\n{}\n</user-instructions>",
                instructions.trim()
            ));
        }
    }
    prompt
}

/// Serializes LLM messages into plain-text transcript suitable for context compaction.
/// Tool results are capped at 1500 chars so massive outputs do not blow up the summarization request.
pub fn serialize_conversation(messages: &[Message]) -> String {
    const MAX_TOOL_CHARS: usize = 1500;
    let mut parts = Vec::new();

    for msg in messages {
        match msg.role {
            Role::User => {
                let text = msg.text_content();
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            Role::Assistant => {
                let mut text_parts = Vec::new();
                let mut tool_calls = Vec::new();
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => {
                            if !text.trim().is_empty() {
                                text_parts.push(text.as_str());
                            }
                        }
                        ContentBlock::ToolUse { name, args, .. } => {
                            let args_str = serde_json::to_string(args).unwrap_or_default();
                            tool_calls.push(format!("{name}({args_str})"));
                        }
                        _ => {}
                    }
                }
                if !text_parts.is_empty() {
                    parts.push(format!("[Assistant]: {}", text_parts.join("\n")));
                }
                if !tool_calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", tool_calls.join("; ")));
                }
            }
            Role::System => {
                let text = msg.text_content();
                if !text.is_empty() {
                    parts.push(format!("[System]: {text}"));
                }
            }
        }

        // Also check if any tool result blocks were stored in the message
        for block in &msg.content {
            if let ContentBlock::ToolResult { id, content, is_error } = block {
                let text = content.as_str();
                let truncated = if text.chars().count() > MAX_TOOL_CHARS {
                    let s: String = text.chars().take(MAX_TOOL_CHARS).collect();
                    format!("{s}... [truncated]")
                } else {
                    text.to_string()
                };
                let err_tag = if *is_error { " (error)" } else { "" };
                parts.push(format!("[Tool result {id}{err_tag}]: {truncated}"));
            }
        }
    }

    parts.join("\n\n")
}

pub struct CompactionSettings {
    pub enabled: bool,
    pub reserve_tokens: usize,
    pub keep_recent_tokens: usize,
}

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
};

pub fn should_compact(tokens: usize, window: usize, s: &CompactionSettings) -> bool {
    s.enabled && tokens > window.saturating_sub(s.reserve_tokens)
}

pub fn calculate_context_tokens(u: &Usage) -> usize {
    if u.total() > 0 { u.total() } else { u.input_tokens + u.output_tokens }
}

pub fn estimate_tokens(msg: &Message) -> usize {
    (msg.text_content().len() as f64 / 4.0).ceil() as usize
}

pub fn estimate_context_tokens(messages: &[Message], last: Option<Usage>) -> usize {
    if let Some(u) = last {
        if u.total() > 0 {
            return u.total();
        }
    }
    messages.iter().map(estimate_tokens).sum()
}

pub fn is_context_overflow_error(err: &CoreError) -> bool {
    if let CoreError::Provider(msg) = err {
        let lower = msg.to_lowercase();
        lower.contains("context_length")
            || lower.contains("context window")
            || lower.contains("context length")
            || lower.contains("max_tokens")
            || lower.contains("maximum context")
            || lower.contains("too many tokens")
            || lower.contains("token limit")
            || lower.contains("context overflow")
    } else {
        false
    }
}

/// Reusable auto-compact helper that mirrors manual `/compact` flow.
///
/// Serializes the whole conversation, asks the model for a structured summary
/// via `agent.complete_prompt`, then replaces history with a 2-message
/// `[summary_user, summary_assistant]` pair. YAGNI: no `findCutPoint` /
/// `prepareCompaction` tail-keeping — Task 4 will add threshold/overflow
/// callers; this is just the shared summarization primitive.
pub async fn auto_compact_if_needed(
    agent: &mut Agent,
    _config: &Config,
    _last_usage: Option<Usage>,
    _reason: &str,
) -> Result<bool, CoreError> {
    compact_with_instructions(agent, None).await
}

/// Core compaction primitive used by both manual `/compact` and auto paths.
/// `custom_instructions` is `Some` only for manual `/compact` with extra args.
pub async fn compact_with_instructions(
    agent: &mut Agent,
    custom_instructions: Option<&str>,
) -> Result<bool, CoreError> {
    let messages = agent.messages().to_vec();
    if messages.is_empty() {
        return Ok(false);
    }
    let transcript = serialize_conversation(&messages);
    let prompt = build_summarization_prompt(&transcript, custom_instructions);
    let summary = agent
        .complete_prompt(&prompt, Some(SUMMARIZATION_SYSTEM_PROMPT))
        .await?;
    let summary_trimmed = summary.trim().to_string();
    let summary_user = Message::user(format!(
        "<conversation_summary>\n{}\n</conversation_summary>\n\nPlease continue assisting based on the summary above.",
        summary_trimmed
    ));
    let summary_asst = Message::assistant(
        "Understood. I have reviewed the conversation summary and context, and I am ready to continue.",
    );
    agent.set_messages(vec![summary_user, summary_asst]);
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_compact_threshold() {
        let s = CompactionSettings {
            enabled: true,
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        };
        assert!(!should_compact(100_000, 128_000, &s));
        assert!(should_compact(115_000, 128_000, &s)); // 115k > 128k-16k
        assert!(!should_compact(
            200_000,
            128_000,
            &CompactionSettings { enabled: false, ..s }
        ));
    }

    #[test]
    fn estimate_uses_usage_when_available() {
        let msgs = vec![Message::user("hi"), Message::assistant("hello")];
        let usage = Usage {
            input_tokens: 100000,
            output_tokens: 10000,
            ..Default::default()
        };
        assert_eq!(estimate_context_tokens(&msgs, Some(usage)), 110000);
    }

    #[test]
    fn estimate_falls_back_to_chars() {
        let msgs = vec![Message::user("a".repeat(400))]; // 100 tokens
        assert_eq!(estimate_context_tokens(&msgs, None), 100);
    }

    #[test]
    fn is_context_overflow_error_detects() {
        assert!(is_context_overflow_error(&CoreError::Provider(
            "context_length_exceeded: too many tokens".into()
        )));
        assert!(is_context_overflow_error(&CoreError::Provider(
            "This model's maximum context length is 128000 tokens".into()
        )));
        assert!(is_context_overflow_error(&CoreError::Provider(
            "context window exceeded".into()
        )));
        assert!(is_context_overflow_error(&CoreError::Provider(
            "max_tokens exceeded".into()
        )));
        // pi parity: generic "length" truncation is NOT overflow via this helper (handled via stopReason in pi)
        assert!(!is_context_overflow_error(&CoreError::Provider(
            "length truncation: output was cut off".into()
        )));
        assert!(!is_context_overflow_error(&CoreError::Provider(
            "rate limit exceeded".into()
        )));
        assert!(!is_context_overflow_error(&CoreError::Cancelled));
    }

    #[test]
    fn repl_threshold_120k_128k_triggers_compact() {
        let usage = Usage {
            input_tokens: 100_000,
            output_tokens: 20_000,
            ..Default::default()
        };
        assert_eq!(usage.total(), 120_000);
        let window = 128_000;
        let tokens = estimate_context_tokens(&[Message::user("hi")], Some(usage));
        assert_eq!(tokens, 120_000);
        assert!(should_compact(tokens, window, &DEFAULT_COMPACTION_SETTINGS));
        // 100k should NOT trigger
        let usage2 = Usage {
            input_tokens: 90_000,
            output_tokens: 10_000,
            ..Default::default()
        };
        let tokens2 = estimate_context_tokens(&[Message::user("hi")], Some(usage2));
        assert!(!should_compact(tokens2, window, &DEFAULT_COMPACTION_SETTINGS));
    }

    #[tokio::test]
    async fn auto_compact_triggers_on_threshold() {
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Agent, Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;
        use crate::config::Config;

        struct FakeProvider {
            summary: String,
        }
        #[async_trait]
        impl Provider for FakeProvider {
            fn stream(&self, _req: ChatRequest) -> BoxStream<'static, Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>> {
                let summary = self.summary.clone();
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta { delta: summary },
                    gray_core::event::StreamEvent::MessageComplete { stop_reason: Some(StopReason::EndTurn), usage: Some(Usage::new(10, 5)) },
                ];
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
            }
        }

        struct NoopExecutor;
        #[async_trait]
        impl ToolExecutor for NoopExecutor {
            fn execute(&self, _ctx: &ToolContext, _name: &str, _args: serde_json::Value) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }

        let provider = FakeProvider { summary: "Test summary content".to_string() };
        let executor = NoopExecutor;
        let mut agent = Agent::new(Box::new(provider), Box::new(executor)).with_messages(vec![
            Message::user("hello"),
            Message::assistant("hi there"),
            Message::user("more context"),
        ]);
        let config = Config {
            model: None,
            base_url: "https://example.com".to_string(),
            api_key: None,
            thinking_effort: None,
            context_window: None,
        };
        let compacted = auto_compact_if_needed(&mut agent, &config, None, "threshold").await.expect("compact should succeed");
        assert!(compacted, "should have compacted");
        assert_eq!(agent.messages().len(), 2, "should be 2 messages after compact");
        assert!(agent.messages()[0].text_content().contains("Test summary content"));
    }
}

