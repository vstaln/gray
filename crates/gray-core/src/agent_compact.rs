//! Transcript compaction for context-overflow recovery (move-only split).
//!
//! [`summary_pair`] is the shared compaction envelope (`Agent` recovery and
//! `gray::compact` can never drift); [`Agent::try_compact_budgeted`] summarizes
//! history into that shape via [`Agent::complete_prompt`].

use crate::agent::Agent;
use crate::error::CoreError;
use crate::message::Message;

/// Shared compaction envelope: `[summary_user, summary_ack]` so
/// `Agent::try_compact_budgeted` and `gray::compact` can never drift.
/// Trims `summary`; byte-stable (see `summary_pair_envelope_is_byte_stable`).
pub fn summary_pair(summary: &str) -> [Message; 2] {
    let s = summary.trim();
    [
        Message::user(format!(
            "Another language model started to solve this problem and produced a summary of its thinking process. Use this to build on the work already done and avoid duplicating work. Here is the summary, use the information in it to assist with your own analysis:\n\n<s>\n{s}\n</s>"
        )),
        Message::assistant(
            "Understood. I have reviewed the conversation summary and context, and I am ready to continue.",
        ),
    ]
}

impl Agent {
    /// One-shot transcript compaction preserving a recent tail: splits history into
    /// head (summarized) + newest messages totaling ≤ `keep_tail_tokens`
    /// (kept verbatim), then `messages = summary_pair + tail`. Returns `Ok(true)`
    /// iff the replacement history is strictly smaller in estimated tokens;
    /// otherwise `self.messages` is left untouched and `Ok(false)` is returned
    /// (empty, blank, everything fits in the tail, or the summary would not
    /// shrink history). Callers can therefore retry on `Ok(true)` knowing each
    /// success makes progress, and must stop on `Ok(false)`.
    pub(crate) async fn try_compact_budgeted(
        &mut self,
        keep_tail_tokens: usize,
    ) -> Result<bool, CoreError> {
        if self.messages.is_empty() {
            return Ok(false);
        }
        // Tail walk, newest-first: the newest message is always kept verbatim and older ones join while the budget allows.
        let mut tail_count = 0usize;
        let mut tail_tokens = 0usize;
        for m in self.messages.iter().rev() {
            let t = est_token(m);
            if tail_count > 0 && tail_tokens + t > keep_tail_tokens {
                break;
            }
            tail_tokens += t;
            tail_count += 1;
        }
        let head_n = self.messages.len() - tail_count;
        if head_n == 0 {
            return Ok(false);
        }
        // Overflow recovery must summarize `context_text`, not
        // `text_content`: the latter omits every tool result — the
        // summary would lose exactly the file bodies and command
        // output the run depended on.
        let transcript = self.messages[..head_n]
            .iter()
            .map(|m| format!("{}: {}", m.role, m.context_text()))
            .collect::<Vec<_>>()
            .join("\n");
        if transcript.trim().is_empty() {
            return Ok(false);
        }
        let summary = self
            .complete_prompt(
                &format!(
                    "Summarize this conversation concisely, preserving key facts, decisions, and pending work:\n{transcript}"
                ),
                Some("You summarize conversations for context compaction."),
            )
            .await?;
        if summary.trim().is_empty() {
            return Ok(false);
        }
        let tail = self.messages[head_n..].to_vec();
        let mut next = Vec::with_capacity(2 + tail.len());
        next.extend(summary_pair(&summary));
        next.extend(tail);
        // Enforced shrink: a replacement that is not strictly smaller is not
        // a compaction — leave history untouched so both the pre-turn retry
        // and the overflow retry terminate on `false`.
        if est_tokens(&next) >= est_tokens(&self.messages) {
            return Ok(false);
        }
        self.messages = next;
        Ok(true)
    }
}

/// One message's token estimate (bytes/4 over billable text).
fn est_token(m: &Message) -> usize {
    m.context_text().len() / 4
}

/// Shared transcript estimate: the tail walk and the shrink comparison above
/// use this, as do `Agent::estimate_tokens` (agent.rs) and the compaction-v2
/// port (`compact_v2::message_tokens`) — single owner, no mirrors.
pub(crate) fn est_tokens(msgs: &[Message]) -> usize {
    msgs.iter().map(est_token).sum()
}

/// Tokens held back from compaction no matter what (Codex parity).
pub const COMPACT_RESERVE_TOKENS: usize = 16_000;
/// Recent history preserved verbatim across a compaction (Codex
/// `COMPACT_USER_MESSAGE_MAX_TOKENS` parity).
pub const KEEP_TAIL_TOKENS: usize = 20_000;

/// Pre-turn budget check: true when the estimate plus reserve reaches the
/// window. `None` window never fires (fail-safe).
pub(crate) fn needs_pre_turn_compact(estimate: usize, window: Option<usize>) -> bool {
    match window {
        None => false,
        Some(w) => estimate.saturating_add(COMPACT_RESERVE_TOKENS) >= w,
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;
    use crate::agent::{Agent, Provider, ProviderStream, ToolContext, ToolExecutor, ToolOutput};
    use crate::event::StreamEvent;
    use crate::message::ChatRequest;
    use async_trait::async_trait;
    use futures::future::BoxFuture;
    use std::sync::Arc;

    /// Scripted provider for `complete_prompt`: returns one fixed summary
    /// text as `TextDelta` + `MessageComplete`, mirroring `complete_prompt`'s
    /// consumption of `StreamEvent::TextDelta`/`MessageComplete`.
    struct SummaryProvider {
        text: String,
    }

    #[async_trait]
    impl Provider for SummaryProvider {
        fn stream(&self, _req: ChatRequest) -> ProviderStream {
            let text = self.text.clone();
            Box::pin(futures::stream::iter(vec![
                Ok(StreamEvent::text_delta(text)),
                Ok(StreamEvent::message_complete(None, None)),
            ]))
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
        ) -> BoxFuture<'static, ToolOutput> {
            Box::pin(async move { ToolOutput::ok("unused") })
        }
    }

    fn test_agent(summary_text: &str) -> Agent {
        Agent::new(
            Box::new(SummaryProvider {
                text: summary_text.to_string(),
            }),
            Arc::new(NoopExecutor),
        )
    }

    /// One ~100-token message by the estimator (bytes/4): 400 chars.
    fn sized_msg(tag: &str) -> Message {
        Message::user(format!("{tag}:{}", "x".repeat(400 - tag.len() - 1)))
    }

    #[tokio::test]
    async fn budgeted_compact_keeps_recent_tail() {
        let mut agent = test_agent("FIXED-SUMMARY-123");
        let bodies: Vec<String> = (1..=6).map(|i| format!("msg{i}")).collect();
        agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
        // Last 2 messages ≈ 200 tokens: budget covers them but not a third.
        let ok = agent.try_compact_budgeted(250).await.unwrap();
        assert!(ok);
        let msgs = agent.messages();
        assert_eq!(msgs.len(), 4, "summary_pair + 2 tail, got {}", msgs.len());
        assert!(
            msgs[0].text_content().contains("FIXED-SUMMARY-123"),
            "summary_user carries the provider summary"
        );
        assert!(
            msgs[2].text_content().contains("msg5"),
            "tail order: {}",
            msgs[2].text_content().chars().take(20).collect::<String>()
        );
        assert!(msgs[3].text_content().contains("msg6"), "tail order");
    }

    #[tokio::test]
    async fn budgeted_compact_false_when_nothing_to_gain() {
        let mut agent = test_agent("UNUSED");
        agent.set_messages(vec![Message::user("hi")]);
        let before = agent.messages().to_vec();
        let ok = agent.try_compact_budgeted(20_000).await.unwrap();
        assert!(!ok);
        assert_eq!(agent.messages(), &before);
    }

    #[tokio::test]
    async fn budgeted_compact_false_when_summary_would_not_shrink() {
        // Already-compact 2-message history: head is message 1, tail is
        // message 2, and the provider returns a summary LONGER than the head
        // it replaces (the 2→2 spin). The enforced shrink invariant must
        // return Ok(false) with history byte-identical.
        let long = "z".repeat(2000);
        let mut agent = test_agent(&long);
        agent.set_messages(summary_pair("prior summary").into());
        let before = agent.messages().to_vec();
        let ok = agent.try_compact_budgeted(50).await.unwrap();
        assert!(!ok);
        assert_eq!(
            agent.messages(),
            &before,
            "non-shrinking compact must leave history untouched"
        );
    }

    #[tokio::test]
    async fn budget_math() {
        assert!(needs_pre_turn_compact(120_000, Some(128_000))); // 120k+16k >= 128k
        assert!(!needs_pre_turn_compact(100_000, Some(128_000)));
        assert!(!needs_pre_turn_compact(999_999_999, None)); // unknown window: never
    }
}
