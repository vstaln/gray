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
//! estimate — gray's system prompt is constant, so it is excluded here).
//!
//! Staged port: later compaction-v2 tasks wire this module up; until then the
//! crate-level `dead_code` allow keeps `cargo clippy -- -D warnings` green.
#![allow(dead_code)]

use crate::message::{ContentBlock, Message};

/// Verbatim copy of codex's `CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE`
/// (`compact_remote.rs`): user-facing model text, kept identical for
/// behavioral parity.
pub(crate) const CONTEXT_WINDOW_TRUNCATED_OUTPUT_MESSAGE: &str =
    "Output exceeded the available model context and was truncated";

/// One message's token estimate (bytes/4 over billable text — same one-liner
/// as `agent_compact::est_tokens`, duplicated so this module stays
/// self-contained).
fn message_tokens(m: &Message) -> usize {
    m.context_text().len() / 4
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Role;

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
}
