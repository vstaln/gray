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

pub const SUMMARIZATION_SYSTEM_PROMPT: &str = r#"You are performing a CONTEXT CHECKPOINT COMPACTION. Create a continuation summary for another LLM that will resume the task. Be concise, structured, and focused on helping the next LLM seamlessly continue the work.

Do NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary."#;

const BASE_SUMMARIZATION_PROMPT: &str = r#"The messages below are conversation messages from the current session.

Create a continuation summary for another LLM that will resume the task. Include:
- Current progress and key decisions made
- Important context, constraints, or user preferences
- What remains to be done (clear next steps)
- Any critical data, examples, file paths, or references needed to continue

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
    if let Some(instructions) = custom_instructions
        && !instructions.trim().is_empty()
    {
        prompt.push_str(&format!(
                "\n\n<user-instructions>\nThe user provided these instructions for this summary. Follow them with high priority while keeping the section format above:\n{}\n</user-instructions>",
                instructions.trim()
            ));
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
            if let ContentBlock::ToolResult {
                id,
                content,
                is_error,
            } = block
            {
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

pub mod policy;

pub use policy::{
    CompactionSettings, compaction_settings_for, estimate_context_tokens, estimate_tokens,
    init_auto_compact_from_env, is_auto_compact_enabled, is_context_overflow_error,
    set_auto_compact_enabled, should_compact, tail_messages,
};
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
    Ok(compact_with_keep(agent, None, keep).await?.is_some())
}

/// Collects the `ToolUse` ids in a message.
fn tool_use_ids(m: &Message) -> std::collections::HashSet<&str> {
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

/// Collects the `ToolResult` ids in a message.
fn tool_result_ids(m: &Message) -> Vec<&str> {
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ToolResult { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect()
}

/// True when any `ToolResult` in `messages` has no matching `ToolUse` in the
/// same slice — a transcript strict providers reject.
fn has_orphaned_results(messages: &[Message]) -> bool {
    let uses: std::collections::HashSet<&str> = messages.iter().flat_map(tool_use_ids).collect();
    messages
        .iter()
        .flat_map(tool_result_ids)
        .any(|id| !uses.contains(id))
}

/// Expands a token-budget `tail` (always a suffix of `messages`) backwards
/// while its oldest message holds a `ToolResult` whose call sits just outside
/// the tail — a budget cut splitting an assistant `ToolUse` from its result.
/// Pairs stay atomic, mirroring core's `atomic_groups`. A result whose call is
/// nowhere nearby is a pre-existing stray: left alone for the caller to refuse.
fn repair_tail_boundary(messages: &[Message], mut tail: Vec<Message>) -> Vec<Message> {
    loop {
        if tail.is_empty() || tail.len() >= messages.len() {
            break;
        }
        let uses: std::collections::HashSet<&str> = tail.iter().flat_map(tool_use_ids).collect();
        let orphaned_in_oldest = tool_result_ids(&tail[0])
            .into_iter()
            .any(|id| !uses.contains(id));
        if !orphaned_in_oldest {
            break;
        }
        let prev = &messages[messages.len() - tail.len() - 1];
        let prev_uses = tool_use_ids(prev);
        if tool_result_ids(&tail[0])
            .into_iter()
            .any(|id| prev_uses.contains(id))
        {
            tail.insert(0, prev.clone());
        } else {
            break;
        }
    }
    tail
}

/// Summary + recent tail: replaces history with `[tail..., summary_user,
/// summary_assistant]`. Order matches core's `try_compact_budgeted`
/// (retained history first, summary pair LAST) so the two paths never drift.
/// Refuses (`None`, history untouched) on blank summaries, non-shrinking
/// replacements, or orphaned tool results.
pub async fn compact_with_keep(
    agent: &mut Agent,
    custom_instructions: Option<&str>,
    keep_tokens: usize,
) -> Result<Option<String>, CoreError> {
    let messages = agent.messages().to_vec();
    if messages.is_empty() {
        return Ok(None);
    }
    let tail = tail_messages(&messages, keep_tokens);
    let transcript = serialize_conversation(&messages);
    let prompt = build_summarization_prompt(&transcript, custom_instructions);
    let summary = agent
        .complete_prompt(&prompt, Some(SUMMARIZATION_SYSTEM_PROMPT))
        .await?;
    if summary.trim().is_empty() {
        return Ok(None);
    }
    let [summary_user, summary_asst] = gray_core::agent::summary_pair(&summary);
    let mut next = repair_tail_boundary(&messages, tail);
    next.extend([summary_user, summary_asst]);
    if has_orphaned_results(&next) {
        return Ok(None);
    }
    if estimate_context_tokens(&next, None) >= estimate_context_tokens(&messages, None) {
        return Ok(None);
    }
    let replaced = messages.len();
    agent.set_messages(next);
    // T3.4 lifecycle: entries stay for the write guard, but no dedup stub
    // may reference a compacted-away result.
    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
        ledger.disarm_all_dedup();
    }
    // Reversible checkpoint: summary on disk, so nothing is truly lost.
    // Best-effort; compaction succeeds even if it fails.
    let path = write_continuation_checkpoint(&summary, replaced);
    if let Some(p) = path {
        eprintln!("continuation checkpoint: {}", p.display());
    }
    Ok(Some(summary))
}

/// Best-effort snapshot of the compact summary for human recovery. Carries
/// only the summary: the full pre-compact transcript stays durable in the
/// session JSONL (pre-boundary entries) and nothing ever reads this file back
/// into the model, so copying the transcript here only widened secret
/// exposure. Lives under the gray home dir (never /tmp or the workspace) with
/// an unpredictable uuid name, owner-only (0600) on unix; newest 5 kept.
pub fn write_continuation_checkpoint(
    summary: &str,
    replaced_messages: usize,
) -> Option<std::path::PathBuf> {
    let dir = crate::setup::catalog::gray_home()
        .ok()?
        .join("continuation-checkpoints");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!(
        "gray-continuation-{}.md",
        uuid::Uuid::new_v4().as_simple()
    ));
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let doc = format!(
        "# Gray continuation checkpoint ({ts})\n\nCompacted {replaced_messages} messages.\n\n## Summary\n\n{summary}\n"
    );
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&path).ok()?;
    use std::io::Write as _;
    file.write_all(doc.as_bytes()).ok()?;
    let _ = file.sync_all();
    rotate_continuation_checkpoints(&dir);
    Some(path)
}

/// Keeps the newest `KEEP` checkpoints; best-effort, failures ignored.
fn rotate_continuation_checkpoints(dir: &std::path::Path) {
    const KEEP: usize = 5;
    let mut files: Vec<(std::time::SystemTime, std::path::PathBuf)> = std::fs::read_dir(dir)
        .ok()
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with("gray-continuation-")
                })
                .filter_map(|e| {
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    Some((mtime, e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_key(|(mtime, _)| *mtime);
    files.reverse();
    for (_, path) in files.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(path);
    }
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
    if summary.trim().is_empty() {
        return Ok(false);
    }
    // The pair is Text-only by construction, so no orphan check applies.
    let next = Vec::from(gray_core::agent::summary_pair(&summary));
    if estimate_context_tokens(&next, None) >= estimate_context_tokens(&messages, None) {
        return Ok(false);
    }
    agent.set_messages(next);
    // T3.4 lifecycle: see compact_with_keep.
    if let Some(ledger) = gray_plugin::builder::current_file_ledger() {
        ledger.disarm_all_dedup();
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn should_compact_threshold() {
        let s = CompactionSettings {
            reserve_tokens: 16384,
            keep_recent_tokens: 20000,
        };
        assert!(!should_compact(100_000, 128_000, &s));
        assert!(should_compact(115_000, 128_000, &s)); // 115k > 128k-16k
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
        // Generic "length" truncation is NOT overflow via this helper (handled via stopReason by the caller)
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
        let s = CompactionSettings {
            reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
        };
        assert!(should_compact(tokens, window, &s));
        // 100k should NOT trigger
        let usage2 = Usage {
            input_tokens: 90_000,
            output_tokens: 10_000,
            ..Default::default()
        };
        let tokens2 = estimate_context_tokens(&[Message::user("hi")], Some(usage2));
        assert!(!should_compact(tokens2, window, &s));
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serializes against switch-flipping tests for the whole test
    async fn auto_compact_triggers_on_threshold() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        use crate::config::Config;
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Agent, Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;

        struct FakeProvider {
            summary: String,
        }
        #[async_trait]
        impl Provider for FakeProvider {
            fn stream(
                &self,
                _req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                let summary = self.summary.clone();
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta { delta: summary },
                    gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    },
                ];
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
            }
        }

        struct NoopExecutor;
        #[async_trait]
        impl ToolExecutor for NoopExecutor {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }

        let provider = FakeProvider {
            summary: "Test summary content".to_string(),
        };
        let executor = NoopExecutor;
        crate::setup::set_user_keep_recent_tokens(Some(0));
        // Large enough that the summary pair strictly shrinks history (the
        // enforced shrink invariant refuses toy histories).
        let mut agent = Agent::new(Box::new(provider), Arc::new(executor)).with_messages(vec![
            Message::user("hello ".repeat(500)),
            Message::assistant("hi there ".repeat(500)),
            Message::user("more context ".repeat(500)),
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
            permissions: None,
        };
        let compacted = auto_compact_if_needed(&mut agent, &config, None, "threshold")
            .await
            .expect("compact should succeed");
        crate::setup::set_user_keep_recent_tokens(None);
        assert!(compacted, "should have compacted");
        assert_eq!(
            agent.messages().len(),
            2,
            "should be 2 messages after compact"
        );
        assert!(
            agent.messages()[0]
                .text_content()
                .contains("Test summary content")
        );
    }

    #[test]
    fn tail_keeps_recent_within_budget() {
        let msgs = vec![
            Message::user("a".repeat(400)), // ~100 tok
            Message::user("b".repeat(400)), // ~100 tok
            Message::user("c".repeat(400)), // ~100 tok
        ];
        let tail = tail_messages(&msgs, 150);
        assert_eq!(
            tail.len(),
            1,
            "only last msg fits in 150 tok budget, got {}",
            tail.len()
        );
        assert!(tail[0].text_content().contains('c'));
        let tail_all = tail_messages(&msgs, 10_000);
        assert_eq!(tail_all.len(), 3);
        let tail_none = tail_messages(&msgs, 0);
        assert!(tail_none.is_empty());
    }

    /// Redirects `GRAY_HOME` at a test-private dir (restored on drop) so
    /// checkpoint writes never touch the real home. Mutates process env:
    /// hold `COMPACT_SWITCH_SERIAL` for the whole test.
    struct TempGrayHome {
        prev: Option<String>,
        _dir: tempfile::TempDir,
    }
    impl TempGrayHome {
        fn set() -> Self {
            let prev = std::env::var("GRAY_HOME").ok();
            let dir = tempfile::TempDir::new().expect("temp gray home");
            unsafe { std::env::set_var("GRAY_HOME", dir.path()) };
            Self { prev, _dir: dir }
        }
    }
    impl Drop for TempGrayHome {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var("GRAY_HOME", v) },
                None => unsafe { std::env::remove_var("GRAY_HOME") },
            }
        }
    }

    /// Scripted `complete_prompt` provider returning one fixed summary text.
    fn scripted_agent(summary_text: &str) -> Agent {
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;
        struct P {
            text: String,
        }
        #[async_trait]
        impl Provider for P {
            fn stream(
                &self,
                _req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                let text = self.text.clone();
                Box::pin(futures::stream::iter(vec![
                    Ok(gray_core::event::StreamEvent::TextDelta { delta: text }),
                    Ok(gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    }),
                ]))
            }
        }
        struct E;
        #[async_trait]
        impl ToolExecutor for E {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }
        Agent::new(
            Box::new(P {
                text: summary_text.to_string(),
            }),
            Arc::new(E),
        )
    }

    /// One ~100-token message by the estimator (bytes/4): 400 chars.
    fn sized(tag: &str) -> Message {
        Message::user(format!("{tag}:{}", "x".repeat(400 - tag.len() - 1)))
    }

    #[tokio::test]
    async fn compact_with_keep_rejects_blank_summary() {
        let mut ag = scripted_agent("   \n  ");
        ag.set_messages(vec![Message::user("hello"), Message::assistant("hi")]);
        let before = ag.messages().to_vec();
        let out = compact_with_keep(&mut ag, None, 0)
            .await
            .expect("must not error on blank summary");
        assert!(out.is_none(), "blank summary must refuse, not wipe history");
        assert_eq!(ag.messages(), &before);
    }

    #[tokio::test]
    async fn compact_with_instructions_rejects_blank_summary() {
        let mut ag = scripted_agent("  ");
        ag.set_messages(vec![Message::user("hello")]);
        let before = ag.messages().to_vec();
        let ok = compact_with_instructions(&mut ag, None)
            .await
            .expect("must not error on blank summary");
        assert!(!ok);
        assert_eq!(ag.messages(), &before);
    }

    #[tokio::test]
    async fn compact_refuses_non_shrinking_replacement() {
        // One tiny message vs a huge summary: the replacement is bigger, so
        // both paths must leave history byte-identical (mirrors core's
        // `budgeted_compact_false_when_summary_would_not_shrink`).
        let long = "z".repeat(2000);
        let mut ag = scripted_agent(&long);
        ag.set_messages(vec![Message::user("hi")]);
        let before = ag.messages().to_vec();
        assert!(
            compact_with_keep(&mut ag, None, 0)
                .await
                .expect("must not error")
                .is_none()
        );
        assert_eq!(ag.messages(), &before);
        let mut ag2 = scripted_agent(&long);
        ag2.set_messages(vec![Message::user("hi")]);
        assert!(!compact_with_instructions(&mut ag2, None).await.expect("ok"));
        assert_eq!(ag2.messages(), &before);
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // serial guard must cover the whole test (env + gray home)
    async fn compact_order_is_tail_then_summary_last() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let mut ag = scripted_agent("S");
        ag.set_messages(vec![sized("m1"), sized("m2"), sized("m3")]);
        let out = compact_with_keep(&mut ag, None, 250)
            .await
            .expect("compact must succeed");
        assert!(out.is_some());
        // Core order (try_compact_budgeted): retained tail first, summary LAST.
        let msgs = ag.messages();
        assert_eq!(msgs.len(), 4, "2 tail + summary pair, got {}", msgs.len());
        assert!(msgs[0].text_content().contains("m2"), "retained oldest");
        assert!(msgs[1].text_content().contains("m3"), "retained newest");
        assert!(msgs[2].text_content().contains('S'), "summary_user last");
        assert!(msgs[3].text_content().contains("Understood"), "ack closes");
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // see compact_order_is_tail_then_summary_last
    async fn compact_repairs_split_tool_pair() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let mut ag = scripted_agent("S");
        // Budget keeps result+new but not the 500-token ToolUse: the naive
        // tail would orphan the result; repair must pull the call back in.
        let use_msg = Message::new(
            Role::Assistant,
            vec![ContentBlock::tool_use(
                "t1",
                "sh",
                serde_json::json!({"cmd": "x".repeat(2000)}),
            )],
        );
        let res_msg = Message::new(
            Role::User,
            vec![ContentBlock::tool_result("t1", "ok", false)],
        );
        ag.set_messages(vec![
            Message::user("o".repeat(2000)),
            use_msg,
            res_msg,
            Message::user("new"),
        ]);
        let out = compact_with_keep(&mut ag, None, 250)
            .await
            .expect("compact must succeed");
        assert!(out.is_some(), "repaired tail still shrinks, must compact");
        let msgs = ag.messages();
        assert_eq!(msgs.len(), 5, "[use, result, new, summary, ack]");
        assert!(
            msgs[0].content.iter().any(|b| matches!(b,
                ContentBlock::ToolUse { id, .. } if id == "t1")),
            "repaired tail must carry the call"
        );
        assert!(
            msgs[1].content.iter().any(|b| matches!(b,
                ContentBlock::ToolResult { id, .. } if id == "t1")),
            "result keeps its mate"
        );
        assert!(msgs[3].text_content().contains('S'), "summary last");
    }

    #[tokio::test]
    async fn compact_refuses_orphaned_tool_result() {
        let mut ag = scripted_agent("short summary here");
        // Stray result: no ToolUse "t9" anywhere in history (pre-existing
        // corruption, not a boundary split) — refuse, don't brick providers.
        ag.set_messages(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::tool_result("t9", "orphan output", false)],
            ),
            Message::user("hello"),
        ]);
        let before = ag.messages().to_vec();
        let out = compact_with_keep(&mut ag, None, 10_000)
            .await
            .expect("must not error");
        assert!(out.is_none(), "orphaned result must refuse compaction");
        assert_eq!(ag.messages(), &before);
    }

    #[test]
    fn checkpoint_is_private_unpredictable_and_rotated() {
        let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
        let _home = TempGrayHome::set();
        let a = write_continuation_checkpoint("summary A", 10).expect("write");
        let b = write_continuation_checkpoint("summary B", 20).expect("write");
        assert_ne!(a.file_name(), b.file_name(), "names must be unpredictable");
        let home = std::env::var("GRAY_HOME").expect("test sets GRAY_HOME");
        assert!(a.starts_with(&home), "under gray home, got {}", a.display());
        assert_eq!(
            a.parent().expect("parent"),
            std::path::Path::new(&home).join("continuation-checkpoints"),
            "per gray-home dir, never temp_dir(), got {}",
            a.display()
        );
        let body = std::fs::read_to_string(&a).expect("read back");
        assert!(body.contains("summary A"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&a).expect("stat").permissions().mode() & 0o777,
                0o600,
                "owner-only"
            );
        }
        for i in 0..7 {
            write_continuation_checkpoint(&format!("s{i}"), i);
        }
        let count = std::fs::read_dir(a.parent().expect("parent"))
            .expect("read dir")
            .count();
        assert_eq!(count, 5, "rotation keeps newest 5, kept {count}");
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
        #![allow(clippy::await_holding_lock)] // serial guard must cover each whole test (global switch + env)
        use super::*;
        use crate::config::Config;
        use async_trait::async_trait;
        use futures::stream::BoxStream;
        use gray_core::agent::{Agent, Provider, ToolContext, ToolExecutor};
        use gray_core::event::{StopReason, Usage};
        use gray_core::message::ChatRequest;

        struct FakeProvider;
        #[async_trait]
        impl Provider for FakeProvider {
            fn stream(
                &self,
                _req: ChatRequest,
            ) -> BoxStream<
                'static,
                Result<gray_core::event::StreamEvent, gray_core::agent::ProviderError>,
            > {
                let events = vec![
                    gray_core::event::StreamEvent::TextDelta {
                        delta: "summarized".to_string(),
                    },
                    gray_core::event::StreamEvent::MessageComplete {
                        stop_reason: Some(StopReason::EndTurn),
                        usage: Some(Usage::new(10, 5)),
                    },
                ];
                Box::pin(futures::stream::iter(events.into_iter().map(Ok)))
            }
        }

        struct NoopExecutor;
        #[async_trait]
        impl ToolExecutor for NoopExecutor {
            fn execute(
                &self,
                _ctx: &ToolContext,
                _name: &str,
                _args: serde_json::Value,
            ) -> futures::future::BoxFuture<'static, gray_core::agent::ToolOutput> {
                Box::pin(async { gray_core::agent::ToolOutput::ok("") })
            }
        }

        fn agent() -> Agent {
            // Large enough that the summary pair strictly shrinks history
            // (the enforced shrink invariant refuses toy histories).
            Agent::new(Box::new(FakeProvider), Arc::new(NoopExecutor)).with_messages(vec![
                Message::user("hello ".repeat(500)),
                Message::assistant("hi there ".repeat(500)),
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
                permissions: None,
            }
        }

        #[tokio::test]
        async fn auto_compact_disabled_is_noop() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            set_auto_compact_enabled(false);
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag, &config(), None, "threshold")
                .await
                .expect("must not error when disabled");
            assert!(!out);
            assert_eq!(
                ag.messages().len(),
                2,
                "disabled auto-compact must leave history untouched"
            );
        }

        #[tokio::test]
        async fn env_kill_switch_disables_auto_compact() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _guard = EnableGuard;
            let prev = std::env::var("GRAY_NO_AUTO_COMPACT").ok();
            unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", "1") };
            init_auto_compact_from_env();
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag, &config(), None, "threshold")
                .await
                .expect("must not error when disabled");
            assert!(!out);
            assert_eq!(
                ag.messages().len(),
                2,
                "env-disabled auto-compact must leave history untouched"
            );
            match prev {
                Some(v) => unsafe { std::env::set_var("GRAY_NO_AUTO_COMPACT", v) },
                None => unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") },
            }
        }

        #[tokio::test]
        async fn env_unset_leaves_auto_compact_enabled() {
            let _serial = COMPACT_SWITCH_SERIAL.lock().unwrap();
            let _home = TempGrayHome::set();
            let _guard = EnableGuard;
            let prev = std::env::var("GRAY_NO_AUTO_COMPACT").ok();
            unsafe { std::env::remove_var("GRAY_NO_AUTO_COMPACT") };
            init_auto_compact_from_env();
            // Zero keep: with the default 20k keep budget the tail would hold
            // everything and the shrink invariant would (correctly) refuse.
            crate::setup::set_user_keep_recent_tokens(Some(0));
            let mut ag = agent();
            let out = auto_compact_if_needed(&mut ag, &config(), None, "threshold")
                .await
                .expect("compact should succeed");
            crate::setup::set_user_keep_recent_tokens(None);
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
            let _home = TempGrayHome::set();
            let _guard = EnableGuard;
            set_auto_compact_enabled(false);
            let mut ag = agent();
            let out = compact_with_keep(&mut ag, None, 0)
                .await
                .expect("manual compact must run when disabled");
            assert!(out.is_some());
            assert!(ag.messages()[0].text_content().contains("summarized"));
            let mut ag2 = agent();
            let out2 = compact_with_instructions(&mut ag2, None)
                .await
                .expect("manual compact must run when disabled");
            assert!(out2);
            assert!(ag2.messages()[0].text_content().contains("summarized"));
        }
    }
}
