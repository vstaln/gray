//! Transcript compaction for context-overflow recovery (move-only split).
//!
//! [`summary_message`] is the shared compaction envelope (`Agent` recovery and
//! `gray::compact` can never drift); [`Agent::compact_v2`] summarizes via
//! [`Agent::complete_with_history`] ([`Agent::try_compact_budgeted`] is its
//! bool-shaped delegate) and installs pi's layout: summary first, then the
//! retained tail.

use crate::agent::Agent;
use crate::compact::{RETAINED_MESSAGE_TOKEN_BUDGET, run_compaction_call};
use crate::error::CoreError;
use crate::message::Message;

/// Shared compaction envelope, pi `COMPACTION_SUMMARY_PREFIX`/`SUFFIX`: one
/// user message, so `Agent::try_compact_budgeted` and `gray::compact` can
/// never drift. No canned assistant reply follows it: the retained tail
/// comes next, so the request still ends on the user/tool turn it ended on.
/// Trims `summary`; byte-stable (see `summary_message_envelope_is_byte_stable`).
pub fn summary_message(summary: &str) -> Message {
    let s = summary.trim();
    Message::user(format!(
        "The conversation history before this point was compacted into the following summary:\n\n<summary>\n{s}\n</summary>"
    ))
}

impl Agent {
    /// One-shot transcript compaction via the compaction-v2 pipeline: summarize
    /// the history with one in-band trigger call, retain the newest history
    /// within budget, then `messages = [summary_message] + retained` (pi order).
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
        //
        // The candidate is the *scrubbed* view (arXiv:2605.08563): a
        // contaminated salvaged partial rides the compaction trigger as the
        // one-line marker, never as its own text, so the summary the provider
        // writes cannot carry the failed trajectory forward into every later
        // request. `self.messages` itself is untouched — the transcript keeps
        // what the user saw, and the shrink check below still measures the
        // real history this call replaces.
        let candidate = self.scrubbed_messages();
        // Retained budget first (v2 + adaptation #2): min(64k, window−16k
        // reserve); unknown window keeps the 64k ceiling. Callers (the user
        // keep-recent setting) may override it.
        let budget = retained_budget.unwrap_or(match self.context_window {
            None => RETAINED_MESSAGE_TOKEN_BUDGET,
            Some(w) => RETAINED_MESSAGE_TOKEN_BUDGET.min(w.saturating_sub(COMPACT_RESERVE_TOKENS)),
        });
        // Cheap no-gain bail before the summary call: the retained walk can
        // cost at most `budget`, so a transcript already within budget cannot
        // strictly shrink once the summary lands. Makes proactive
        // rolling callers (and manual /compact on tiny transcripts) free
        // instead of burning an LLM call that the shrink check would reject.
        if est_tokens(&candidate) <= budget {
            return Ok(None);
        }
        // Stage 1 (v2): one trigger call over the history with the live
        // system + tools; empty summary compacts nothing.
        let summary = run_compaction_call(self, &candidate, instructions).await?;
        if summary.trim().is_empty() {
            return Ok(None);
        }
        // Stage 1.5 (arXiv:2512.22087 stable anchor): the original user
        // intent is a fixed segment that survives every compaction, pinned
        // ahead of the summary. Its tokens come out of the retained budget;
        // a copy the retained walk kept anyway is pulled out, so the intent
        // is pinned exactly once.
        let anchor = crate::compact::anchor_message(&candidate);
        let anchor_tokens = anchor
            .as_ref()
            .map(crate::agent_compact::estimate_message_tokens)
            .unwrap_or(0);
        // An anchor at or over the whole budget would crowd out every
        // retained message; skip pinning rather than starve the tail.
        let pin_anchor = anchor.is_some() && anchor_tokens < budget;
        // Stage 2: retained newest history within budget (computed above),
        // minus whatever the pinned anchor spends. At-threshold tool outputs
        // elide to citation stubs naming this session's transcript
        // (arXiv:2607.25066) instead of dropping without a trace.
        let retained_budget = budget.saturating_sub(if pin_anchor { anchor_tokens } else { 0 });
        // The anchor IS `candidate[0]`, so the walk starts past it rather than
        // pulling it back out afterwards. Excluding it by position, not by
        // value, is the whole point: a `retain(|m| m != &anchor)` deleted
        // *every* message equal to the intent, so a user turn that
        // legitimately repeated the prompt later in the conversation vanished
        // from the tail (and the request stopped ending on a user turn).
        // Starting past index 0 also stops the anchor's tokens being charged
        // to both the pinned segment and the tail.
        let walk = if pin_anchor {
            &candidate[1..]
        } else {
            &candidate[..]
        };
        let retained = crate::compact::build_retained_with_session(
            walk,
            retained_budget,
            self.session_id.as_deref(),
        );
        // Stage 3 (pi `buildSessionContext`): [anchor,] summary, retained
        // tail, so the request still ends on the tail's user/tool turn.
        let mut next = Vec::with_capacity(retained.len() + 2);
        if pin_anchor {
            next.push(anchor.expect("pin_anchor implies Some"));
        }
        next.push(summary_message(&summary));
        next.extend(retained);
        // Enforced shrink: a replacement that is not strictly smaller is not
        // a compaction — leave history untouched so both the pre-turn retry
        // and the overflow retry terminate on `None`.
        if est_tokens(&next) >= est_tokens(&self.messages) {
            return Ok(None);
        }
        self.set_messages(next);
        Ok(Some(summary))
    }
}

/// One message's token estimate (bytes/4 over billable text).
pub fn estimate_message_tokens(m: &Message) -> usize {
    if m.content
        .iter()
        .any(|b| matches!(b, crate::message::ContentBlock::Image { .. }))
    {
        m.content.iter().map(crate::compact::block_tokens).sum()
    } else {
        m.context_text().len() / 4
    }
}

/// Shared transcript estimate: the shrink comparison above uses this, as do
/// `Agent::estimate_tokens` (agent.rs) and the compaction-v2 port
/// (`compact::message_tokens`) — single owner, no mirrors.
pub(crate) fn est_tokens(msgs: &[Message]) -> usize {
    msgs.iter().map(estimate_message_tokens).sum()
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
