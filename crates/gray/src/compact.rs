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

/// Master switch for *automatic* compaction only (`should_compact` callers and
/// `auto_compact_if_needed`). Manual entry points (`compact_with_keep`,
/// `compact_with_instructions`) always run, so an `enabled=false` session keeps
/// its manual escape hatch.
static AUTO_COMPACT_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Disables/enables automatic compaction for this session. Manual `/compact` ignores this.
pub fn set_auto_compact_enabled(on: bool) {
    AUTO_COMPACT_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn is_auto_compact_enabled() -> bool {
    AUTO_COMPACT_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Env kill-switch for automatic compaction, following the `GRAY_*` pattern in
/// `config.rs`: `GRAY_NO_AUTO_COMPACT=1` (also `true`/`yes`/`on`) disables it.
/// `0`/`false`/`no`/`off`/unset leave it enabled. Manual `/compact` still runs.
pub fn init_auto_compact_from_env() {
    let disabled = std::env::var("GRAY_NO_AUTO_COMPACT")
        .map(|s| !matches!(s.trim().to_ascii_lowercase().as_str(), "" | "0" | "false" | "no" | "off"))
        .unwrap_or(false);
    set_auto_compact_enabled(!disabled);
}

pub const DEFAULT_COMPACTION_SETTINGS: CompactionSettings = CompactionSettings {
    enabled: true,
    reserve_tokens: 16384,
    keep_recent_tokens: 20000,
};

pub fn should_compact(tokens: usize, window: usize, s: &CompactionSettings) -> bool {
    s.enabled && tokens > window.saturating_sub(s.reserve_tokens)
}

/// Effective compaction settings from user overrides (or proportional defaults).
/// Window-aware: reserve ≈ window/16, keep ≈ window/13 when no override.
pub fn compaction_settings() -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: crate::setup::user_reserve_tokens(),
        keep_recent_tokens: crate::setup::user_keep_recent_tokens(),
    }
}

/// Window-aware variant — prefer this where the model window is known.
pub fn compaction_settings_for(window: usize) -> CompactionSettings {
    CompactionSettings {
        enabled: true,
        reserve_tokens: crate::setup::user_reserve_tokens_for(window),
        keep_recent_tokens: crate::setup::user_keep_for(window),
    }
}

/// Recent tail of `messages` whose estimated tokens fit in `keep_tokens`.
/// Walks from the newest message backwards; `0` keeps nothing.
pub fn tail_messages(messages: &[Message], keep_tokens: usize) -> Vec<Message> {
    if keep_tokens == 0 {
        return Vec::new();
    }
    let mut kept = Vec::new();
    let mut acc = 0usize;
    for msg in messages.iter().rev() {
        let t = estimate_tokens(msg);
        if kept.is_empty() && acc == 0 {
            // Always keep at least the newest message when budget > 0,
            // even if that single message exceeds the budget.
            kept.push(msg.clone());
            acc += t;
            continue;
        }
        if acc + t > keep_tokens {
            break;
        }
        kept.push(msg.clone());
        acc += t;
    }
    kept.reverse();
    kept
}

pub fn calculate_context_tokens(u: &Usage) -> usize {
    if u.total() > 0 { u.total() } else { u.input_tokens + u.output_tokens }
}

pub fn estimate_tokens(msg: &Message) -> usize {
    // Must measure billable context, not displayable prose: a message whose
    // only block is a 50 KiB tool result is ~12.8k tokens, not 0. See
    // `Message::context_text`.
    (msg.context_text().len() as f64 / 4.0).ceil() as usize
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
    if !is_auto_compact_enabled() {
        return Ok(false);
    }
    let keep = crate::setup::user_keep_recent_tokens();
    compact_with_keep(agent, None, keep).await
}

/// Summary + recent tail: replaces history with `[summary_user, summary_assistant,
/// ...tail]` where tail fits in `keep_tokens`. `keep_tokens == 0` = legacy 2-message result.
pub async fn compact_with_keep(
    agent: &mut Agent,
    custom_instructions: Option<&str>,
    keep_tokens: usize,
) -> Result<bool, CoreError> {
    let messages = agent.messages().to_vec();
    if messages.is_empty() {
        return Ok(false);
    }
    let tail = tail_messages(&messages, keep_tokens);
    let transcript = serialize_conversation(&messages);
    let prompt = build_summarization_prompt(&transcript, custom_instructions);
    let summary = agent
        .complete_prompt(&prompt, Some(SUMMARIZATION_SYSTEM_PROMPT))
        .await?;
    let [summary_user, summary_asst] = gray_core::agent::summary_pair(&summary);
    let mut next = vec![summary_user, summary_asst];
    next.extend(tail);
    agent.set_messages(next);
    Ok(true)
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
    let [summary_user, summary_asst] = gray_core::agent::summary_pair(&summary);
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
    fn summary_envelope_matches_core_helper_byte_for_byte() {
        let [u1, a1] = gray_core::agent::summary_pair("  shared summary  ");
        assert!(u1.text_content().contains("shared summary"));
        // Both compact paths build via the same helper, so envelopes are identical.
        let [u2, a2] = gray_core::agent::summary_pair("shared summary");
        assert_eq!(u1.text_content().as_bytes(), u2.text_content().as_bytes());
        assert_eq!(a1.text_content().as_bytes(), a2.text_content().as_bytes());
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

    /// A message holding one capped tool result (`truncate.rs` allows 50 KiB)
    /// must not measure as free. Regression guard for `estimate_tokens`
    /// measuring with a display accessor.
    #[test]
    fn tail_budget_counts_tool_results() {
        // 4 KiB of tool output => 4096/4 = 1024 tokens.
        let big = Message::new(
            Role::User,
            vec![ContentBlock::tool_result("t1", "x".repeat(4096), false)],
        );
        assert_eq!(
            estimate_tokens(&big),
            1024,
            "a 4 KiB tool result is ~1024 tokens; measuring 0 means the \
             estimator is reading text blocks only"
        );

        // Ten of them against a 3000-token keep budget: the walk should stop
        // after two. Before the fix each scored 0, so all ten were retained
        // and `keep_recent_tokens` was a no-op for tool-heavy history.
        let msgs: Vec<Message> = (0..10).map(|_| big.clone()).collect();
        let tail = tail_messages(&msgs, 3000);
        assert_eq!(
            tail.len(),
            2,
            "tail must stop at the 3000-token budget, kept {}",
            tail.len()
        );
    }

    /// With no provider `Usage` (resumed session, or an OpenAI-compatible
    /// endpoint that omits usage on streaming), the estimator is the only
    /// signal deciding whether to compact. A tool-heavy history must cross
    /// the reserve line.
    #[test]
    fn tool_heavy_history_trips_threshold_without_provider_usage() {
        let msgs: Vec<Message> = (0..10)
            .map(|i| {
                Message::new(
                    Role::User,
                    vec![ContentBlock::tool_result(
                        format!("t{i}"),
                        "y".repeat(8192),
                        false,
                    )],
                )
            })
            .collect();

        // 10 x 8 KiB = 80 KiB => 20480 tokens. Before the fix: 0.
        let tokens = estimate_context_tokens(&msgs, None);
        assert_eq!(tokens, 20_480, "80 KiB of tool output measured as {tokens}");

        let s = CompactionSettings {
            enabled: true,
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        };
        // 32k window, 16384 reserve => threshold 15616. 20480 is over it.
        assert!(
            should_compact(tokens, 32_000, &s),
            "tool-heavy history must auto-compact; before the fix it measured \
             zero and the session ran straight into a provider overflow"
        );
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
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
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
        crate::setup::set_user_keep_recent_tokens(Some(0));
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
            show_reasoning: None,
            context_window: None,
            context_reserve: None,
            context_keep: None,
        };
        let compacted = auto_compact_if_needed(&mut agent, &config, None, "threshold").await.expect("compact should succeed");
        crate::setup::set_user_keep_recent_tokens(None);
        assert!(compacted, "should have compacted");
        assert_eq!(agent.messages().len(), 2, "should be 2 messages after compact");
        assert!(agent.messages()[0].text_content().contains("Test summary content"));
    }

    #[test]
    fn tail_keeps_recent_within_budget() {
        let msgs = vec![
            Message::user("a".repeat(400)), // ~100 tok
            Message::user("b".repeat(400)), // ~100 tok
            Message::user("c".repeat(400)), // ~100 tok
        ];
        let tail = tail_messages(&msgs, 150);
        assert_eq!(tail.len(), 1, "only last msg fits in 150 tok budget, got {}", tail.len());
        assert!(tail[0].text_content().contains('c'));
        let tail_all = tail_messages(&msgs, 10_000);
        assert_eq!(tail_all.len(), 3);
        let tail_none = tail_messages(&msgs, 0);
        assert!(tail_none.is_empty());
    }

    /// Serializes tests that flip the global auto-compact switch; also guards
    /// the flag back to enabled even when an assertion panics.
    static COMPACT_SWITCH_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    struct EnableGuard;
    impl Drop for EnableGuard {
        fn drop(&mut self) {
            set_auto_compact_enabled(true);
        }
    }

    mod switch_tests {
        use super::*;
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Agent, Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;
        use crate::config::Config;

        struct FakeProvider;
        #[async_trait]
        impl Provider for FakeProvider {
            fn stream(&self, _req: ChatRequest) -> BoxStream<'static, Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>> {
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta { delta: "summarized".to_string() },
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

        fn agent() -> Agent {
            Agent::new(Box::new(FakeProvider), Box::new(NoopExecutor)).with_messages(vec![
                Message::user("hello"),
                Message::assistant("hi there"),
            ])
        }

        fn config() -> Config {
            Config {
                model: None,
                base_url: "https://example.com".to_string(),
                api_key: None,
                thinking_effort: None,
                show_reasoning: None,
                context_window: None,
                context_reserve: None,
                context_keep: None,
            }
        }

        #[tokio::test]
        async fn auto_compact_disabled_is_noop() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            set_auto_compact_enabled(false);
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag, &config(), None, "threshold").await.expect("must not error when disabled");
            assert!(!out);
            assert_eq!(ag.messages().len(), 2, "disabled auto-compact must leave history untouched");
        }

        #[tokio::test]
        async fn env_kill_switch_disables_auto_compact() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            let prev = std::env::var("GRAY_NO_AUTO_COMPACT").ok();
            unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", "1") };
            init_auto_compact_from_env();
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag, &config(), None, "threshold").await.expect("must not error when disabled");
            assert!(!out);
            assert_eq!(ag.messages().len(), 2, "env-disabled auto-compact must leave history untouched");
            match prev {
                Some(v) => unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", v) },
                None => unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") },
            }
        }

        #[tokio::test]
        async fn env_unset_leaves_auto_compact_enabled() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            let prev = std::env::var("GRAY_NO_AUTO_COMPACT").ok();
            unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") };
            init_auto_compact_from_env();
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag, &config(), None, "threshold").await.expect("compact should succeed");
            assert!(out);
            assert!(ag.messages()[0].text_content().contains("summarized"));
            match prev {
                Some(v) => unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", v) },
                None => unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") },
            }
        }

        #[tokio::test]
        async fn manual_compact_bypasses_disabled_switch() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            set_auto_compact_enabled(false);
            let mut ag = agent();
            let out = compact_with_keep(&mut ag, None, 0).await.expect("manual compact must run when disabled");
            assert!(out);
            assert!(ag.messages()[0].text_content().contains("summarized"));
            let mut ag2 = agent();
            let out2 = compact_with_instructions(&mut ag2, None).await.expect("manual compact must run when disabled");
            assert!(out2);
            assert!(ag2.messages()[0].text_content().contains("summarized"));
        }
    }
}

