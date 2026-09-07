//! Tool-output pre-trim for compaction v2 (Task 1 of the Codex compaction-v2 port).
//!
//! Ported from openai/codex (Apache-2.0, gray is MIT — compatible) at
//! `reference/openai/codex` commit `1fb5158b`:
//! - `codex-rs/core/src/compact_remote.rs` (trim loop + placeholder const)
//! - `codex-rs/core/src/compact_remote_v2.rs`
//! - `codex-rs/core/src/compact_remote_v2_images.rs`
//!
//! Sanctioned adaptations: token math is bytes/4 (gray has no tiktoken dep);
//! trim applies to `messages` only (codex folds base instructions into its
//! estimate — gray's system prompt is constant, so it is excluded here);
//! codex's per-`ResponseItem` groups become atomic slices of gray's flat
//! `Vec<Message>` (an assistant `ToolUse` fuses with the next user message's
//! `ToolResult`s); boundary truncation is text-only (images are Task 4).
//!
//! Staged port: later compaction-v2 tasks wire this module up; until then the
//! crate-level `dead_code` allow keeps `cargo clippy -- -D warnings` green.
#![allow(dead_code)]

use crate::agent::Agent;
use crate::error::CoreError;
use crate::message::{ContentBlock, Message, Role};

/// Verbatim copy of codex's `CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE`
/// (`compact_remote.rs`): user-facing model text, kept identical for
/// behavioral parity.
pub(crate) const CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE: &str =
    "Output exceeded the available model context and was truncated";

/// One message's token estimate: delegates to the shared
/// `agent_compact::est_tokens` owner (bytes/4 over billable text).
fn message_tokens(m: &Message) -> usize {
    crate::agent_compact::est_tokens(std::slice::from_ref(m))
}

/// Replace newest-first `ToolResult` block contents with the placeholder until
/// the transcript estimate fits `window`. Mirrors codex's
/// `trim_function_call_history_to_fit_context_window` newest-first loop +
/// running-estimate update (subtract-old/add-new per rewrite).
///
/// Differences from codex, both forced by gray's flat `Vec<Message>` shape:
/// - codex splices rewritten groups back contiguously and so *breaks* on the
///   first non-rewritable group; gray mutates blocks in place, so messages
///   without a `ToolResult` are skipped and older ones are still trimmed.
/// - each `ToolResult` block is independently replaceable: `id`/`is_error`
///   are kept, so pairing and alternation are untouched.
///
/// Unknown window (`None`) returns `(0, 0)` immediately, mirroring v2's early
/// return. Returns (rewritten block count, estimated deleted tokens).
// Brief-mandated `&mut Vec` signature (matches `salvage_partial_text` precedent).
#[allow(clippy::ptr_arg)]
pub(crate) fn trim_tool_results_to_fit(
    messages: &mut Vec<Message>,
    window: Option<usize>,
) -> (usize, u64) {
    let Some(window) = window else {
        return (0, 0);
    };
    let mut estimated: usize = messages.iter().map(message_tokens).sum();
    let initial = estimated;
    let mut rewritten = 0;
    for msg in messages.iter_mut().rev() {
        if estimated <= window {
            break;
        }
        for i in (0..msg.content.len()).rev() {
            if estimated <= window {
                break;
            }
            let replaceable = matches!(
                &msg.content[i],
                ContentBlock::ToolResult { content, .. }
                if content.as_str() != CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE
            );
            if !replaceable {
                continue;
            }
            let old_msg_tokens = message_tokens(msg);
            if let ContentBlock::ToolResult { content, .. } = &mut msg.content[i] {
                *content = CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE.to_string();
            }
            estimated = estimated - old_msg_tokens + message_tokens(msg);
            rewritten += 1;
        }
    }
    (rewritten, initial.saturating_sub(estimated) as u64)
}

/// In-band compaction trigger: chat-shaped equivalent of codex v2's
/// `ResponseItem::CompactionTrigger` + summary instruction, appended to the
/// retained history for the summarization call only.
pub(crate) const COMPACTION_TRIGGER: &str = "Compact this conversation for context compaction: reply with ONLY a concise summary preserving key facts, decisions, file states, and pending work. No tool calls.";

/// One in-band summarization call over `history`: same system prompt + same
/// tools as live turns, plus [`COMPACTION_TRIGGER`]. Identical request prefix
/// → server prefix-cache hit on providers that support it. Takes `&[Message]`
/// and builds the input locally — the caller's history is never mutated — and
/// the trigger is popped from retained history after (mirroring v2's
/// `prompt_input.pop()`), so it never pollutes the transcript.
pub(crate) async fn run_compaction_call(
    agent: &Agent,
    history: &[Message],
) -> Result<String, CoreError> {
    let mut messages = history.to_vec();
    messages.push(Message::user(COMPACTION_TRIGGER));
    // Empty system maps to `None`, exactly like a live turn (`agent_loop.rs`):
    // the trigger request then carries byte-identical prefix fields.
    let system = agent.system_text();
    agent
        .complete_with_history(
            (!system.is_empty()).then_some(system),
            messages,
            agent.tool_defs().to_vec(),
        )
        .await
}

/// Retained-history token budget, mirroring codex v2's
/// `RETAINED_MESSAGE_TOKEN_BUDGET` verbatim.
pub(crate) const RETAINED_MESSAGE_TOKEN_BUDGET: usize = 64_000;
/// Per-group cap for retained assistant-only groups, mirroring codex v2's
/// `MAX_RETAINED_AGENT_MESSAGE_TOKENS`. `i64` kept verbatim — converted at use.
/// Single assistant messages over this are progress/completion chatter.
pub(crate) const MAX_RETAINED_AGENT_MESSAGE_TOKENS: i64 = 10_000;
/// Boundary-truncation marker, inlined between the kept head+tail halves. Gray
/// has no message marker convention; documented here. Marker bytes come out of
/// the token budget so truncated output still estimates within budget.
const TRUNCATION_MARKER: &str = "[…truncated…]";

/// Build the retained history: group atomically → retention filter →
/// newest-first budget walk → chronological order. Mirrors codex v2's
/// `build_v2_compacted_history` minus the summary append (Task 5) and image
/// charging (Task 4).
pub(crate) fn build_retained(messages: &[Message], budget: usize) -> Vec<Message> {
    let mut kept_reversed: Vec<Vec<Message>> = Vec::new();
    let mut remaining = budget;
    for group in atomic_groups(messages)
        .into_iter()
        .filter(|g| group_is_retained(g))
        .rev()
    {
        if remaining == 0 {
            continue;
        }
        let cost = group_tokens(group).max(1);
        if cost <= remaining {
            remaining -= cost;
            kept_reversed.push(group.to_vec());
        } else if let Some(truncated) = truncate_group_to_budget(group, remaining) {
            kept_reversed.push(truncated);
            remaining = 0;
        }
        // Ineligible boundary group (images / no truncatable text): dropped,
        // walk continues with `remaining` untouched.
    }
    kept_reversed.into_iter().rev().flatten().collect()
}

/// Split chronological `messages` into atomic retain/drop groups: an assistant
/// message's `ToolUse` ids fuse with the NEXT user message's `ToolResult` ids
/// on id intersection; unmatched strays are singleton groups. Pairs therefore
/// retain/drop atomically — never an orphaned call or result.
fn atomic_groups(messages: &[Message]) -> Vec<&[Message]> {
    let mut groups = Vec::new();
    let mut i = 0;
    while i < messages.len() {
        let pair = messages[i].role == Role::Assistant
            && messages.get(i + 1).is_some_and(|next| {
                next.role == Role::User && tool_ids_intersect(&messages[i], next)
            });
        if pair {
            groups.push(&messages[i..i + 2]);
            i += 2;
        } else {
            groups.push(&messages[i..i + 1]);
            i += 1;
        }
    }
    groups
}

fn tool_use_ids(m: &Message) -> impl Iterator<Item = &str> {
    m.content.iter().filter_map(|b| match b {
        ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
        _ => None,
    })
}

fn tool_ids_intersect(assistant: &Message, user: &Message) -> bool {
    tool_use_ids(assistant).any(|id| {
        user.content.iter().any(|b| match b {
            ContentBlock::ToolResult { id: result_id, .. } => result_id == id,
            _ => false,
        })
    })
}

fn has_text(m: &Message) -> bool {
    m.content.iter().any(|b| match b {
        ContentBlock::Text { text } => !text.is_empty(),
        _ => false,
    })
}

fn has_image(m: &Message) -> bool {
    m.content
        .iter()
        .any(|b| matches!(b, ContentBlock::Image { .. }))
}

/// Assistant message with ≥1 `ToolUse` and zero text: v2's
/// descendant-progress/completion chatter equivalent.
fn is_tool_use_only(m: &Message) -> bool {
    m.role == Role::Assistant && tool_use_ids(m).next().is_some() && !has_text(m)
}

fn group_tokens(group: &[Message]) -> usize {
    group.iter().map(message_tokens).sum()
}

/// Retention filter per group, mirroring v2's
/// `is_retained_for_remote_compaction_v2`: `ToolUse`-only chatter drops
/// together with its paired results; user/system groups are always retained
/// (the budget walk sizes them); text-carrying assistant-only groups survive
/// iff within the per-group cap.
fn group_is_retained(group: &[Message]) -> bool {
    if group.iter().any(is_tool_use_only) {
        return false;
    }
    if group
        .iter()
        .any(|m| matches!(m.role, Role::User | Role::System))
    {
        return true;
    }
    i64::try_from(group_tokens(group)).unwrap_or(i64::MAX) <= MAX_RETAINED_AGENT_MESSAGE_TOKENS
}

/// Middle-truncate a group's `Text` blocks to `budget` tokens, walking blocks
/// in order like v2's `truncate_message_text_to_token_budget` (fits → keep;
/// first overflow → head+tail truncate; later text dropped). Non-text blocks
/// pass through (tool results own the Task 1 trim budget; thinking/tool-use
/// args are small). `Image` blocks make a group ineligible here — image
/// charging is Task 4 — so `None` (caller drops the whole group). `None` also
/// when the group holds no truncatable text.
fn truncate_group_to_budget(group: &[Message], budget: usize) -> Option<Vec<Message>> {
    if group.iter().any(has_image) {
        return None;
    }
    let mut out = group.to_vec();
    let mut remaining = budget;
    let mut saw_text = false;
    for msg in &mut out {
        for block in &mut msg.content {
            let ContentBlock::Text { text } = block else {
                continue;
            };
            if text.is_empty() {
                continue;
            }
            saw_text = true;
            if remaining == 0 {
                text.clear();
            } else if text.len() / 4 <= remaining {
                remaining -= text.len() / 4;
            } else {
                *text = truncate_text_to_budget(text, remaining);
                remaining = 0;
            }
        }
        msg.content.retain(|b| match b {
            ContentBlock::Text { text } => !text.is_empty(),
            _ => true,
        });
    }
    saw_text.then_some(out)
}

/// Middle-truncate `text` to `max_tokens`: keep head+tail halves with
/// [`TRUNCATION_MARKER`] inline. Already-small text returns untouched (no
/// marker). Splits on char boundaries.
fn truncate_text_to_budget(text: &str, max_tokens: usize) -> String {
    if text.len() / 4 <= max_tokens {
        return text.to_string();
    }
    let usable = max_tokens
        .saturating_mul(4)
        .saturating_sub(TRUNCATION_MARKER.len());
    let (head, tail) = split_head_tail(text, usable);
    format!("{head}{TRUNCATION_MARKER}{tail}")
}

fn split_head_tail(s: &str, usable: usize) -> (&str, &str) {
    let right = usable - usable / 2;
    let mut head_end = (usable / 2).min(s.len());
    while !s.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = s.len().saturating_sub(right);
    while tail_start < s.len() && !s.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    if tail_start < head_end {
        tail_start = head_end;
    }
    (&s[..head_end], &s[tail_start..])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
    use crate::event::{StopReason, StreamEvent};
    use crate::message::{ChatRequest, Role, ToolDef};
    use async_trait::async_trait;
    use futures::future::BoxFuture;
    use std::sync::{Arc, Mutex};

    fn tool_msgs() -> Vec<Message> {
        ["c1", "c2", "c3"]
            .into_iter()
            .map(|id| Message {
                role: Role::User,
                content: vec![ContentBlock::ToolResult {
                    id: id.to_string(),
                    content: "x".repeat(10_000),
                    is_error: false,
                }],
            })
            .collect()
    }

    #[test]
    fn trim_replaces_newest_tool_results_first_keeping_ids() {
        let mut msgs = tool_msgs();
        // Total estimate: 3 × 2500 = 7500. One trim lands at 5000 + 14 = 5014.
        let (rewritten, _deleted) = trim_tool_results_to_fit(&mut msgs, Some(5100));
        assert_eq!(rewritten, 1);
        for (msg, id) in msgs.iter().zip(["c1", "c2", "c3"]) {
            let ContentBlock::ToolResult {
                id: got_id,
                content,
                ..
            } = &msg.content[0]
            else {
                panic!("expected ToolResult block");
            };
            assert_eq!(got_id, id);
            if id == "c3" {
                assert_eq!(content, CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE);
            } else {
                assert_eq!(content.len(), 10_000);
            }
        }
    }

    #[test]
    fn trim_unknown_window_is_noop() {
        let mut msgs = tool_msgs();
        let before = msgs.clone();
        assert_eq!(trim_tool_results_to_fit(&mut msgs, None), (0, 0));
        assert_eq!(msgs, before);
    }

    #[test]
    fn trim_reports_deleted_tokens() {
        let mut msgs = tool_msgs();
        let (_rewritten, deleted) = trim_tool_results_to_fit(&mut msgs, Some(5100));
        let expected = (10_000 - CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE.len()) as u64 / 4;
        assert!(
            deleted.abs_diff(expected) <= 2,
            "deleted {deleted} ≈ expected {expected}"
        );
        assert!(deleted > 0);
    }

    // --- Task 2 (RED): retention grouping + budget walk --------------------

    fn assistant_tool_use(id: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::tool_use(id, "sh", serde_json::json!({}))],
        }
    }

    fn user_tool_result(id: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::tool_result(id, "ok", false)],
        }
    }

    fn is_subsequence(hay: &[Message], needle: &[Message]) -> bool {
        let mut j = 0;
        for m in hay {
            if j < needle.len() && *m == needle[j] {
                j += 1;
            }
        }
        j == needle.len()
    }

    #[test]
    fn retention_drops_tool_chatter_atomically() {
        let msgs = vec![
            Message::assistant("thinking out loud"),
            assistant_tool_use("c1"),
            user_tool_result("c1"),
            Message::user("done?"),
        ];
        let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
        let mut uses: Vec<&str> = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let mut results: Vec<&str> = out
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        uses.sort();
        results.sort();
        assert_eq!(uses, results, "no orphaned calls either direction");
        assert!(
            is_subsequence(&msgs, &out),
            "output preserves chronological order"
        );
        assert_eq!(out.last().unwrap(), &Message::user("done?"));
    }

    #[test]
    fn oversized_single_assistant_dropped() {
        let big = Message::assistant("y".repeat(12_000 * 4));
        assert!(
            build_retained(std::slice::from_ref(&big), RETAINED_MESSAGE_TOKEN_BUDGET).is_empty()
        );
    }

    #[test]
    fn boundary_group_middle_truncated_not_dropped() {
        let old = Message::user("a".repeat(4000));
        let new = Message::user("b".repeat(4000));
        let out = build_retained(&[old, new.clone()], 1500);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1], new, "newest group kept verbatim");
        let text = out[0].text_content();
        assert!(text.contains(TRUNCATION_MARKER), "boundary group truncated");
        assert!(
            crate::agent_compact::est_tokens(&out) <= 1500,
            "truncated output fits budget"
        );
    }

    // --- Task 3 (RED): in-band trigger call -------------------------------

    /// Test fake mirroring `agent::agent_tests::FakeProvider`'s `Provider`
    /// impl shape: records the `ChatRequest` it receives, replays one
    /// scripted event list.
    struct CapturingProvider {
        script: Mutex<Vec<StreamEvent>>,
        seen: Arc<Mutex<Vec<ChatRequest>>>,
    }

    impl CapturingProvider {
        fn new(script: Vec<StreamEvent>, seen: Arc<Mutex<Vec<ChatRequest>>>) -> Self {
            Self {
                script: Mutex::new(script),
                seen,
            }
        }
    }

    #[async_trait]
    impl Provider for CapturingProvider {
        fn stream(&self, req: ChatRequest) -> ProviderStream {
            self.seen.lock().expect("seen lock poisoned").push(req);
            let script = std::mem::take(&mut *self.script.lock().expect("script lock poisoned"));
            Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
        }
    }

    /// Executor that records calls instead of running them (the trigger-call
    /// drain must never reach it).
    struct RecordingExecutor {
        calls: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ToolExecutor for RecordingExecutor {
        fn execute(
            &self,
            _ctx: &ToolContext,
            name: &str,
            _args: serde_json::Value,
        ) -> BoxFuture<'static, ToolOutput> {
            self.calls
                .lock()
                .expect("calls lock poisoned")
                .push(name.to_string());
            Box::pin(async move { ToolOutput::ok("must-not-run") })
        }
    }

    fn trigger_test_agent(
        script: Vec<StreamEvent>,
        seen: Arc<Mutex<Vec<ChatRequest>>>,
        calls: Arc<Mutex<Vec<String>>>,
        tools: Vec<ToolDef>,
    ) -> Agent {
        Agent::new(
            Box::new(CapturingProvider::new(script, seen)),
            Arc::new(RecordingExecutor { calls }),
        )
        .with_system("S")
        .with_tools(tools)
    }

    #[tokio::test]
    async fn compaction_call_reuses_system_tools_and_appends_trigger() {
        let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let tools = vec![ToolDef::new(
            "read",
            "r",
            serde_json::json!({"type": "object"}),
        )];
        let agent = trigger_test_agent(
            vec![
                StreamEvent::text_delta("SUMMARY"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
            seen.clone(),
            calls,
            tools.clone(),
        );
        let m1 = Message::user("m1");
        let m2 = Message::assistant("m2");
        let history = vec![m1.clone(), m2.clone()];

        let out = run_compaction_call(&agent, &history).await.unwrap();

        assert_eq!(out, "SUMMARY");
        let reqs = seen.lock().expect("seen lock poisoned");
        assert_eq!(reqs.len(), 1, "one trigger call, got {reqs:?}");
        let req = &reqs[0];
        assert_eq!(req.system, Some("S".to_string()));
        assert_eq!(req.tools, tools);
        let mut expected = history.clone();
        expected.push(Message::user(COMPACTION_TRIGGER));
        assert_eq!(req.messages, expected);
        assert_eq!(
            history,
            vec![m1, m2],
            "trigger must not leak into caller history"
        );
    }

    #[tokio::test]
    async fn compaction_call_drops_tool_use_blocks_from_reply() {
        let seen: Arc<Mutex<Vec<ChatRequest>>> = Arc::default();
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let agent = trigger_test_agent(
            vec![
                StreamEvent::text_delta("S"),
                StreamEvent::tool_call_delta(0, Some("c1".into()), Some("read".into()), "{}"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
            seen,
            calls.clone(),
            Vec::new(),
        );

        let out = run_compaction_call(&agent, &[Message::user("hi")])
            .await
            .unwrap();

        assert_eq!(out, "S");
        assert!(
            calls.lock().expect("calls lock poisoned").is_empty(),
            "no tool execution attempted"
        );
    }

    #[test]
    fn budget_walk_newest_first() {
        // Exact-fit sizing: 3 × 32k tokens at a 64k budget leaves
        // `remaining == 0` after the two newest, so the oldest is dropped (a
        // 30k sizing would boundary-truncate it instead — covered above).
        let mk =
            |tag: &str| Message::user(format!("{tag}:{}", "z".repeat(32_000 * 4 - tag.len() - 1)));
        let msgs = vec![mk("m1"), mk("m2"), mk("m3")];
        let out = build_retained(&msgs, RETAINED_MESSAGE_TOKEN_BUDGET);
        assert_eq!(out.len(), 2);
        assert!(out[0].text_content().starts_with("m2:"));
        assert!(out[1].text_content().starts_with("m3:"));
    }
}
