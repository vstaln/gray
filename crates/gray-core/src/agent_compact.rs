use crate::agent::Agent;
use crate::compact::{
    PRICE_CLIFF_KEEP_DIVISOR, RETAINED_MESSAGE_TOKEN_BUDGET, SUMMARY_ENVELOPE_PREFIX,
    run_compaction_call,
};
use crate::error::CoreError;
use crate::message::{ContentBlock, Message};

pub fn summary_message(summary: &str) -> Message {
    let s = summary.trim();
    Message::user_injected(format!(
        "{SUMMARY_ENVELOPE_PREFIX}\n\n<summary>\n{s}\n</summary>"
    ))
}

impl Agent {
    pub(crate) fn ceiling_reason(&self, estimate: usize) -> crate::event::CompactReason {
        use crate::event::CompactReason;
        match self.compact_ceiling.filter(|&c| estimate >= c) {
            Some(_) => match self.price_cliff {
                Some(cliff) => CompactReason::PriceCliff { cliff },
                None => CompactReason::UserCeiling,
            },
            None => CompactReason::Window,
        }
    }

    pub(crate) async fn try_compact_budgeted(&mut self) -> Result<bool, CoreError> {
        let before = self.estimate_tokens();
        let compacted = self.compact_v2(None, self.compact_keep).await?.is_some();
        log::info!(
            target: "gray_agent",
            "compaction: ok={compacted} before={before} after={}",
            self.estimate_tokens()
        );
        Ok(compacted)
    }

    pub async fn compact_v2(
        &mut self,
        instructions: Option<&str>,
        retained_budget: Option<usize>,
    ) -> Result<Option<String>, CoreError> {
        if self.messages.is_empty() {
            return Ok(None);
        }

        let mut candidate = self.scrubbed_messages();
        crate::compact::elide_media_over_payload_budget(&mut candidate);

        let budget = retained_budget.unwrap_or(match self.context_window {
            None => RETAINED_MESSAGE_TOKEN_BUDGET,
            Some(w) => RETAINED_MESSAGE_TOKEN_BUDGET.min(w.saturating_sub(COMPACT_RESERVE_TOKENS)),
        });

        let budget = match self.compact_ceiling {
            Some(ceiling) => budget.min(ceiling / PRICE_CLIFF_KEEP_DIVISOR),
            None => budget,
        };

        if est_tokens(&candidate) <= budget {
            return Ok(None);
        }

        // The summary call carries the whole history, so a payload 413 that
        // sent us here would fail it too; the summarizer can do without pixels.
        let summary = match run_compaction_call(self, &candidate, instructions).await {
            Err(e)
                if matches!(e.class(), CoreError::ContextOverflow(_))
                    && candidate.iter().flat_map(|m| &m.content).any(|b| {
                        matches!(b, ContentBlock::Image { .. } | ContentBlock::Media { .. })
                    }) =>
            {
                let mut text_only = candidate.clone();
                crate::compact::stub_all_media(&mut text_only);
                run_compaction_call(self, &text_only, instructions).await?
            }
            other => other?,
        };
        if summary.trim().is_empty() {
            return Ok(None);
        }

        let anchor = crate::compact::anchor_message(&candidate);
        let anchor_tokens = anchor
            .as_ref()
            .map(crate::agent_compact::estimate_message_tokens)
            .unwrap_or(0);

        let pin_anchor = anchor.is_some() && anchor_tokens < budget;

        let retained_budget = budget.saturating_sub(if pin_anchor { anchor_tokens } else { 0 });

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

        let mut next = Vec::with_capacity(retained.len() + 2);
        if pin_anchor {
            next.push(anchor.expect("pin_anchor implies Some"));
        }
        next.push(summary_message(&summary));
        next.extend(retained);

        if est_tokens(&next) >= est_tokens(&self.messages) {
            return Ok(None);
        }
        self.set_messages(next);
        Ok(Some(summary))
    }
}

pub fn estimate_message_tokens(m: &Message) -> usize {
    if m.content.iter().any(|b| {
        matches!(
            b,
            crate::message::ContentBlock::Image { .. } | crate::message::ContentBlock::Media { .. }
        )
    }) {
        m.content.iter().map(crate::compact::block_tokens).sum()
    } else {
        m.context_text_len() / 4
    }
}

pub(crate) fn est_tokens(msgs: &[Message]) -> usize {
    msgs.iter().map(estimate_message_tokens).sum()
}

pub const COMPACT_RESERVE_TOKENS: usize = 16_000;

pub(crate) fn needs_pre_turn_compact(
    estimate: usize,
    window: Option<usize>,
    reserve: Option<usize>,
    ceiling: Option<usize>,
) -> bool {
    match window {
        None => false,
        Some(w) => {
            estimate.saturating_add(reserve.unwrap_or(COMPACT_RESERVE_TOKENS)) >= w
                || ceiling.is_some_and(|c| estimate >= c)
        }
    }
}

pub(crate) fn needs_soft_elide(estimate: usize, window: Option<usize>) -> bool {
    if std::env::var_os("GRAY_NO_SOFT_ELIDE").is_some() {
        return false;
    }
    window.is_some_and(|w| estimate >= crate::compact::soft_elide_threshold(w))
}

impl Agent {
    pub(crate) fn elide_media_over_budget(&mut self) -> usize {
        let elided = crate::compact::elide_media_over_payload_budget(&mut self.messages);
        // The session file keeps the originals; resume re-elides them the same way.
        if elided > 0 {
            self.history_edited_in_place("media payload budget");
        }
        elided
    }

    pub(crate) fn soft_elide(&mut self) -> usize {
        let Some(window) = self.context_window else {
            return 0;
        };
        if self.estimate_tokens() < self.soft_elide_rearm {
            return 0;
        }
        let keep = self
            .compact_keep
            .unwrap_or_else(|| crate::compact::soft_elide_keep_budget(window));
        let measured = self.estimate_tokens();
        let raw_before = est_tokens(&self.messages);
        let elided = crate::compact::elide_stale_tool_results(
            &mut self.messages,
            keep,
            self.session_id.as_deref(),
        );
        if elided > 0 {
            let saved = raw_before.saturating_sub(est_tokens(&self.messages));
            self.history_rewritten();
            // Keep the provider-measured anchor minus what was stubbed: a
            // char/4 re-estimate counts opaque thinking blobs and overshot
            // a measured 154k as 236k.
            self.context_usage = Some((measured.saturating_sub(saved), self.messages.len()));
            self.soft_elide_rearm = self
                .estimate_tokens()
                .saturating_add(crate::compact::soft_elide_rearm_step(window));
        }
        elided
    }
}

#[path = "compact_tests.rs"]
#[cfg(test)]
mod compact_tests;
