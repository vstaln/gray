use std::sync::Arc;

use crate::agent::{Agent, Provider, complete_isolated_detached};
use crate::compact::{
    PRICE_CLIFF_KEEP_DIVISOR, RETAINED_MESSAGE_TOKEN_BUDGET, SUMMARY_ENVELOPE_PREFIX,
    trigger_message,
};
use crate::error::CoreError;
use crate::message::{ContentBlock, Message, ToolDef};

pub fn summary_message(summary: &str) -> Message {
    let s = summary.trim();
    Message::user_injected(format!(
        "{SUMMARY_ENVELOPE_PREFIX}\n\n<summary>\n{s}\n</summary>"
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaPressure {
    Clear,
    Stub,
    Compact,
}

pub(crate) struct SummaryRequest {
    provider: Arc<dyn Provider>,
    session_id: Option<String>,
    system: Option<String>,
    tools: Vec<ToolDef>,
    shared_messages: Option<Vec<Message>>,
    candidate: Vec<Message>,
    instructions: Option<String>,
}

pub(crate) struct PreparedCompaction {
    pub(crate) request: SummaryRequest,
    pub(crate) candidate: Vec<Message>,
    pub(crate) anchor: Option<Message>,
    retained_budget: usize,
}

impl PreparedCompaction {
    pub(crate) fn retained_tail(&self, session_id: Option<&str>) -> Vec<Message> {
        let walk = if self.anchor.is_some() {
            &self.candidate[1..]
        } else {
            &self.candidate[..]
        };
        crate::compact::build_retained_with_session(walk, self.retained_budget, session_id)
    }
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

    pub(crate) fn prepare_compaction(
        &self,
        history: &[Message],
        instructions: Option<&str>,
        retained_budget: Option<usize>,
    ) -> Option<PreparedCompaction> {
        if history.is_empty() {
            return None;
        }
        let mut candidate = self.scrubbed_messages();
        candidate.truncate(history.len());
        // `shared` is computed on the un-elided wire view before media
        // stubbing — a stubbed candidate could never be wire-identical.
        let shared_messages =
            crate::compact::shared_prefix_compaction_messages(self, &candidate, instructions);
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
            return None;
        }

        let candidate_anchor = crate::compact::anchor_message(&candidate);
        let anchor_tokens = candidate_anchor
            .as_ref()
            .map(estimate_message_tokens)
            .unwrap_or(0);
        let anchor = candidate_anchor.filter(|_| anchor_tokens < budget);
        let retained_budget =
            budget.saturating_sub(if anchor.is_some() { anchor_tokens } else { 0 });
        let system = self.system_text();
        Some(PreparedCompaction {
            request: SummaryRequest {
                provider: self.provider.clone(),
                session_id: self.session_id.clone(),
                system: (!system.is_empty()).then(|| system.to_string()),
                tools: self.request_tools(),
                shared_messages,
                candidate: candidate.clone(),
                instructions: instructions.map(str::to_string),
            },
            candidate,
            anchor,
            retained_budget,
        })
    }

    pub(crate) async fn apply_compaction(
        &mut self,
        summary: &str,
        anchor: Option<Message>,
        tail: Vec<Message>,
    ) -> bool {
        if summary.trim().is_empty() {
            return false;
        }
        let mut next = Vec::with_capacity(tail.len() + 2);
        if let Some(anchor) = anchor {
            next.push(anchor);
        }
        next.push(summary_message(summary));
        next.extend(tail);
        if est_tokens(&next) >= est_tokens(&self.messages) {
            return false;
        }
        self.set_messages(next);
        true
    }

    pub(crate) async fn try_compact_budgeted(&mut self) -> Result<bool, CoreError> {
        let before = self.estimate_tokens();
        let started = std::time::Instant::now();
        let compacted = self.compact_v2(None, self.compact_keep).await?.is_some();
        log::info!(
            target: "gray_agent",
            "compaction: ok={compacted} before={before} after={} source=inline elapsed_ms={}",
            self.estimate_tokens(),
            started.elapsed().as_millis()
        );
        Ok(compacted)
    }

    pub(crate) async fn try_compact_pre_turn(&mut self) -> Result<bool, CoreError> {
        let before = self.estimate_tokens();
        if let Some(mut held) = self.held_summary.take() {
            if let Some(reason) = held.discard_reason(self) {
                self.background_failures = self.background_failures.saturating_add(1);
                log::info!(target: "gray_agent", "background compaction: discarded reason={reason}");
            } else {
                let result = (&mut held.task).await;
                log::info!(
                    target: "gray_agent",
                    "background compaction: ready ms={}",
                    held.started.elapsed().as_millis()
                );
                match result {
                    Ok(Ok(summary)) if !summary.trim().is_empty() => {
                        let apply_started = std::time::Instant::now();
                        let tail = self.messages[held.covered..].to_vec();
                        for hook in &self.hooks {
                            hook.compact_before().await;
                        }
                        if self
                            .apply_compaction(&summary, held.anchor.clone(), tail)
                            .await
                        {
                            log::info!(
                                target: "gray_agent",
                                "compaction: ok=true before={before} after={} source=background elapsed_ms={}",
                                self.estimate_tokens(),
                                apply_started.elapsed().as_millis()
                            );
                            return Ok(true);
                        }
                        self.background_failures = self.background_failures.saturating_add(1);
                        log::info!(target: "gray_agent", "background compaction: discarded reason=no_gain");
                    }
                    Ok(Ok(_)) => {
                        self.background_failures = self.background_failures.saturating_add(1);
                        log::info!(target: "gray_agent", "background compaction: discarded reason=empty");
                    }
                    Ok(Err(_)) | Err(_) => {
                        self.background_failures = self.background_failures.saturating_add(1);
                        log::info!(target: "gray_agent", "background compaction: discarded reason=error");
                    }
                }
            }
        }
        self.try_compact_budgeted().await
    }

    pub async fn compact_v2(
        &mut self,
        instructions: Option<&str>,
        retained_budget: Option<usize>,
    ) -> Result<Option<String>, CoreError> {
        self.held_summary = None;
        let Some(prepared) =
            self.prepare_compaction(&self.messages.clone(), instructions, retained_budget)
        else {
            return Ok(None);
        };
        for hook in &self.hooks {
            hook.compact_before().await;
        }
        let tail = prepared.retained_tail(self.session_id.as_deref());
        let summary = run_compaction_summary(prepared.request).await?;
        if self.apply_compaction(&summary, prepared.anchor, tail).await {
            Ok(Some(summary))
        } else {
            Ok(None)
        }
    }
}

pub(crate) async fn run_compaction_summary(request: SummaryRequest) -> Result<String, CoreError> {
    let SummaryRequest {
        provider,
        session_id,
        system,
        tools,
        shared_messages,
        candidate,
        instructions,
    } = request;
    if let Some(messages) = shared_messages {
        match complete_isolated_detached(
            provider.clone(),
            session_id.clone(),
            system.clone(),
            messages,
            tools.clone(),
            true,
        )
        .await
        {
            // A wire-identical request that still overflows needs the
            // stubbed (different, cheaper) request — normal path.
            Err(error) if matches!(error.class(), CoreError::ContextOverflow(_)) => {}
            result => return result,
        }
    }
    fallback_compaction_summary(provider, session_id, system, tools, candidate, instructions).await
}

/// The summary call carries the whole history, so a payload 413 that sent
/// us here would fail it too; the summarizer can do without pixels.
async fn fallback_compaction_summary(
    provider: Arc<dyn Provider>,
    session_id: Option<String>,
    system: Option<String>,
    tools: Vec<ToolDef>,
    candidate: Vec<Message>,
    instructions: Option<String>,
) -> Result<String, CoreError> {
    let mut messages = candidate.clone();
    messages.push(trigger_message(instructions.as_deref()));
    match complete_isolated_detached(
        provider.clone(),
        session_id.clone(),
        system.clone(),
        messages,
        tools.clone(),
        false,
    )
    .await
    {
        Err(error)
            if matches!(error.class(), CoreError::ContextOverflow(_))
                && candidate
                    .iter()
                    .flat_map(|message| &message.content)
                    .any(|block| {
                        matches!(
                            block,
                            ContentBlock::Image { .. } | ContentBlock::Media { .. }
                        )
                    }) =>
        {
            let mut text_only = candidate;
            crate::compact::stub_all_media(&mut text_only);
            text_only.push(trigger_message(instructions.as_deref()));
            complete_isolated_detached(provider, session_id, system, text_only, tools, false).await
        }
        result => result,
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
    /// Media past the provider's hard cap must shed before the next request.
    /// On a cold cache, in-place stubbing is nearly free (the prefix has not
    /// been billed yet); once warm, mutating history re-bills the whole
    /// suffix, so pressure is routed through pre-turn compaction, whose
    /// candidate already drops over-budget media. Callers stub in place only
    /// when compaction cannot run.
    pub(crate) fn media_pressure(&self) -> MediaPressure {
        if !crate::compact::media_over_budget(&self.messages) {
            return MediaPressure::Clear;
        }
        if self.prefix_rewrite_ok && !self.warm_cache_reported {
            MediaPressure::Stub
        } else {
            MediaPressure::Compact
        }
    }

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
