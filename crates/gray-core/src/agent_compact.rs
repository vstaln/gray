//! Transcript compaction for context-overflow recovery (move-only split).
//!
//! [`summary_pair`] is the shared compaction envelope (`Agent` recovery and
//! `gray::compact` can never drift); [`Agent::compact_v2`] runs the codex-v2
//! pipeline ([`Agent::try_compact_budgeted`] is its bool-shaped delegate) via
//! [`Agent::complete_with_history`].

use crate::agent::Agent;
use crate::compact::{
    RETAINED_MESSAGE_TOKEN_BUDGET, build_retained, run_compaction_call, trim_tool_results_to_fit,
};
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
    /// One-shot transcript compaction via the compaction-v2 pipeline: trim
    /// tool results to fit, summarize the full trimmed history with one
    /// in-band trigger call, retain the newest history within budget, then
    /// `messages = retained + summary_pair` (summary LAST). Returns `Ok(true)`
    /// iff the replacement history is strictly smaller in estimated tokens;
    /// otherwise `self.messages` is left untouched and `Ok(false)` is returned
    /// (empty, blank summary, or the summary would not shrink history).
    /// Callers can therefore retry on `Ok(true)` knowing each success makes
    /// progress, and must stop on `Ok(false)`.
    pub(crate) async fn try_compact_budgeted(&mut self) -> Result<bool, CoreError> {
        Ok(self.compact_v2(None, None).await?.is_some())
    }

    /// The v2 pipeline behind [`try_compact_budgeted`](Self::try_compact_budgeted),
    /// shared with the gray crate's manual `/compact` and REPL auto paths so
    /// every compaction surface runs the same codex-v2 logic.
    ///
    /// `instructions` are appended to the in-band trigger (manual `/compact
    /// <text>` focus); `retained_budget` overrides the v2 default
    /// (`min(64k, window − 16k)`, or 64k when the window is unknown) with the
    /// user's keep-recent setting. Returns `Ok(Some(summary))` when history was
    /// replaced; `Ok(None)` when nothing was gained (empty, blank summary, or
    /// the replacement would not strictly shrink) with history byte-identical.
    pub async fn compact_v2(
        &mut self,
        instructions: Option<&str>,
        retained_budget: Option<usize>,
    ) -> Result<Option<String>, CoreError> {
        if self.messages.is_empty() {
            return Ok(None);
        }
        // Clone-then-commit: trim/summarize/validate on a candidate so a
        // failed call (provider error, blank summary, non-shrinking result)
        // leaves the live history byte-identical — the single swap below is
        // the only write to `self.messages`.
        let mut candidate = self.messages.clone();
        // Stage 1 (v2): newest-first tool-output pre-trim, so the summary
        // call and the budget walk below both see the trimmed history.
        trim_tool_results_to_fit(&mut candidate, self.context_window);
        // Stage 2 (v2): one trigger call over the full trimmed history with
        // the live system + tools; empty summary compacts nothing.
        let summary = run_compaction_call(self, &candidate, instructions).await?;
        if summary.trim().is_empty() {
            return Ok(None);
        }
        // Stage 3 (v2 + adaptation #2): retained budget defaults to min(64k,
        // window−16k reserve); unknown window keeps the 64k ceiling. Callers
        // (the user keep-recent setting) may override it.
        let budget = retained_budget.unwrap_or(match self.context_window {
            None => RETAINED_MESSAGE_TOKEN_BUDGET,
            Some(w) => RETAINED_MESSAGE_TOKEN_BUDGET.min(w.saturating_sub(COMPACT_RESERVE_TOKENS)),
        });
        let retained = build_retained(&candidate, budget);
        // Stage 4 (v2): summary appended LAST (order change from prepend).
        let mut next = retained;
        next.extend(summary_pair(&summary));
        // Enforced shrink: a replacement that is not strictly smaller is not
        // a compaction — leave history untouched so both the pre-turn retry
        // and the overflow retry terminate on `None`.
        if est_tokens(&next) >= est_tokens(&self.messages) {
            return Ok(None);
        }
        self.messages = next;
        Ok(Some(summary))
    }
}

/// One message's token estimate (bytes/4 over billable text).
fn est_token(m: &Message) -> usize {
    m.context_text().len() / 4
}

/// Shared transcript estimate: the shrink comparison above uses this, as do
/// `Agent::estimate_tokens` (agent.rs) and the compaction-v2 port
/// (`compact::message_tokens`) — single owner, no mirrors.
pub(crate) fn est_tokens(msgs: &[Message]) -> usize {
    msgs.iter().map(est_token).sum()
}

/// Tokens held back from compaction no matter what (Codex parity).
pub const COMPACT_RESERVE_TOKENS: usize = 16_000;

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
    use crate::message::{ChatRequest, ContentBlock, Role};
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
        // v2 order: [retained..., summary_user, summary_ack] (summary LAST).
        let mut agent = test_agent("FIXED-SUMMARY-123").with_context_window(Some(16_200));
        let bodies: Vec<String> = (1..=6).map(|i| format!("msg{i}")).collect();
        agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
        // Retained budget = min(64k, 16_200−16_000) = 200 ≈ last 2 messages.
        let ok = agent.try_compact_budgeted().await.unwrap();
        assert!(ok);
        let msgs = agent.messages();
        assert_eq!(
            msgs.len(),
            4,
            "2 retained + summary_pair, got {}",
            msgs.len()
        );
        assert!(
            msgs[0].text_content().contains("msg5"),
            "retained order: {}",
            msgs[0].text_content().chars().take(20).collect::<String>()
        );
        assert!(msgs[1].text_content().contains("msg6"), "retained order");
        assert!(
            msgs[2].text_content().contains("FIXED-SUMMARY-123"),
            "summary_user carries the provider summary, LAST"
        );
        assert!(
            msgs[3].text_content().contains("Understood"),
            "summary_ack closes history"
        );
    }

    #[tokio::test]
    async fn v2_pipeline_retained_then_summary_last() {
        // 4 messages over a small window, trigger call returns "S":
        // assembly is [retained..., summary_user, summary_ack], trigger
        // nowhere in history.
        let mut agent = test_agent("S").with_context_window(Some(16_200));
        let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
        agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
        let ok = agent.try_compact_budgeted().await.unwrap();
        assert!(ok);
        let msgs = agent.messages();
        assert_eq!(
            msgs.len(),
            4,
            "2 retained + summary_pair, got {}",
            msgs.len()
        );
        assert!(msgs[0].text_content().contains("msg3"), "retained oldest");
        assert!(msgs[1].text_content().contains("msg4"), "retained newest");
        assert!(
            msgs[2]
                .text_content()
                .contains("Another language model started"),
            "summary_user appended LAST"
        );
        assert!(
            msgs[3].text_content().contains("Understood"),
            "summary_ack last"
        );
        assert!(
            msgs.iter().all(|m| !m
                .text_content()
                .contains(crate::compact::COMPACTION_TRIGGER)),
            "trigger must never leak into history"
        );
    }

    #[tokio::test]
    async fn compact_v2_returns_summary_and_honors_zero_budget() {
        let mut agent = test_agent("FIXED-SUMMARY-123");
        let bodies: Vec<String> = (1..=4).map(|i| format!("msg{i}")).collect();
        agent.set_messages(bodies.iter().map(|t| sized_msg(t)).collect());
        let out = agent.compact_v2(None, Some(0)).await.unwrap();
        assert_eq!(out.as_deref(), Some("FIXED-SUMMARY-123"));
        assert_eq!(
            agent.messages().len(),
            2,
            "keep=0 → summary_pair only, got {}",
            agent.messages().len()
        );
        assert!(
            agent.messages()[0]
                .text_content()
                .contains("FIXED-SUMMARY-123"),
            "returned summary is the installed summary"
        );
    }

    #[tokio::test]
    async fn budgeted_compact_false_when_nothing_to_gain() {
        let mut agent = test_agent("UNUSED");
        agent.set_messages(vec![Message::user("hi")]);
        let before = agent.messages().to_vec();
        let ok = agent.try_compact_budgeted().await.unwrap();
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
        let ok = agent.try_compact_budgeted().await.unwrap();
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

    /// Provider whose stream immediately fails: exercises the provider-error
    /// path of `try_compact_budgeted` (same pre-swap early return as cancel).
    struct FailingProvider;

    #[async_trait]
    impl Provider for FailingProvider {
        fn stream(&self, _req: ChatRequest) -> ProviderStream {
            Box::pin(futures::stream::iter(vec![Err(
                crate::agent::ProviderError::Connection("boom".to_string()),
            )]))
        }
    }

    /// Trimmable history: 3 × 2500-token tool results over a 5100 window, so
    /// an in-place pre-trim would rewrite the newest result before the
    /// summary call runs.
    fn trimmable_history() -> Vec<Message> {
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

    fn transcript_bytes(msgs: &[Message]) -> String {
        serde_json::to_string(msgs).expect("transcript serializes")
    }

    #[tokio::test]
    async fn compact_error_leaves_history_byte_identical() {
        let mut agent = Agent::new(Box::new(FailingProvider), Arc::new(NoopExecutor))
            .with_context_window(Some(5100));
        agent.set_messages(trimmable_history());
        let before = transcript_bytes(agent.messages());
        let err = agent.try_compact_budgeted().await;
        assert!(err.is_err(), "provider failure must propagate");
        assert_eq!(
            transcript_bytes(agent.messages()),
            before,
            "failed compact must leave history byte-identical"
        );
    }

    #[tokio::test]
    async fn compact_blank_summary_leaves_history_byte_identical() {
        let mut agent = test_agent("   ").with_context_window(Some(5100));
        agent.set_messages(trimmable_history());
        let before = transcript_bytes(agent.messages());
        let ok = agent.try_compact_budgeted().await.unwrap();
        assert!(!ok);
        assert_eq!(
            transcript_bytes(agent.messages()),
            before,
            "blank-summary compact must leave history byte-identical"
        );
    }
}
