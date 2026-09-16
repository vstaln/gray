//! Transcript compaction for context-overflow recovery (move-only split).
//!
//! [`summary_pair`] is the shared compaction envelope (`Agent` recovery and
//! `gray::compact` can never drift); [`Agent::compact_v2`] runs the codex-v2
//! pipeline ([`Agent::try_compact_budgeted`] is its bool-shaped delegate) via
//! [`Agent::complete_with_history`].

use crate::agent::Agent;
use crate::compact::{RETAINED_MESSAGE_TOKEN_BUDGET, build_retained, run_compaction_call};
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
    /// One-shot transcript compaction via the compaction-v2 pipeline: summarize
    /// the history with one in-band trigger call, retain the newest history
    /// within budget, then `messages = retained + summary_pair` (summary LAST).
    /// Returns `Ok(true)` iff the replacement history is strictly smaller in
    /// estimated tokens; otherwise `self.messages` is left untouched and
    /// `Ok(false)` is returned (empty, under budget, blank summary, or the
    /// summary would not shrink history). Callers can therefore retry on
    /// `Ok(true)` knowing each success makes progress, and must stop on
    /// `Ok(false)`.
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
        // Clone-then-commit: summarize/validate on a candidate so a failed
        // call (provider error, blank summary, non-shrinking result) leaves
        // the live history byte-identical — the single swap below is the
        // only write to `self.messages`.
        let candidate = self.messages.clone();
        // Retained budget first (v2 + adaptation #2): min(64k, window−16k
        // reserve); unknown window keeps the 64k ceiling. Callers (the user
        // keep-recent setting) may override it.
        let budget = retained_budget.unwrap_or(match self.context_window {
            None => RETAINED_MESSAGE_TOKEN_BUDGET,
            Some(w) => RETAINED_MESSAGE_TOKEN_BUDGET.min(w.saturating_sub(COMPACT_RESERVE_TOKENS)),
        });
        // Cheap no-gain bail before the summary call: the retained walk can
        // cost at most `budget`, so a transcript already within budget cannot
        // strictly shrink once the summary pair lands. Makes proactive
        // rolling callers (and manual /compact on tiny transcripts) free
        // instead of burning an LLM call that the shrink check would reject.
        if est_tokens(&candidate) <= budget {
            return Ok(None);
        }
        // Stage 1 (v2): one trigger call over the history with the live
        // system + tools; empty summary compacts nothing. (Proactive output
        // elision upstream already slimmed old tool results, so no separate
        // pre-trim stage: one trimmer, not two.)
        let summary = run_compaction_call(self, &candidate, instructions).await?;
        if summary.trim().is_empty() {
            return Ok(None);
        }
        // Stage 2: retained newest history within budget (computed above).
        let retained = build_retained(&candidate, budget);
        // Stage 3 (v2): summary appended LAST (order change from prepend).
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

#[path = "compact_tests.rs"]
#[cfg(test)]
mod compact_tests;
