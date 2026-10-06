//! Agent turn loop: [`Agent::run`] / [`run_streaming`](Agent::run_streaming).
//!
//! Split from `agent.rs` (move-only): the multi-turn stream → finalize →
//! dispatch cycle plus its loop backstop. Shared transcript helpers live in
//! `agent.rs`, tool-call plumbing in `agent_tools`, compaction in
//! `agent_compact`.

use futures::StreamExt as _;

use crate::agent::{
    Agent, ToolBefore, ToolContext, ToolOutput, salvage_partial_text, thinking_block,
};
use crate::agent_compact::needs_pre_turn_compact;
use crate::agent_tools::{PendingToolCall, answer_pending_tools};
use crate::error::CoreError;
use crate::event::{AgentEvent, StopReason, StreamEvent, Usage, append_thinking_chunk};
use crate::message::{ChatRequest, ContentBlock, Message, Role};

/// Empty-turn provider retries before nudging or ending on `(empty)`.
const MAX_EMPTY_RETRIES: u8 = 2;
/// Truncated-turn (`MaxTokens`) continuations before keeping the partial.
const MAX_CONTINUATIONS: u8 = 2;

/// Whole-turn retries after the provider's own in-request budget (5 attempts
/// with backoff) is exhausted on a retryable failure — the log shows bursts of
/// 503s outlasting exactly that budget, ending a turn the user is waiting on.
/// Each whole-turn retry sleeps a short ramp (the provider already backed off
/// ~1+2+4+8+9s across its attempts) and never fires after a visible delta:
/// once the user has read text, a replay would duplicate it — the provider's
/// partial-answer path owns that case.
const MAX_TURN_RETRIES: u8 = 2;

/// Base sleep before turn-retry 1 (attempt 2 doubles it). Zero in tests so
/// scripted-failure suites stay instant.
#[cfg(not(test))]
fn turn_retry_delay() -> std::time::Duration {
    std::time::Duration::from_secs(3)
}
#[cfg(test)]
fn turn_retry_delay() -> std::time::Duration {
    std::time::Duration::ZERO
}

/// Replaces a contaminated salvaged partial in outbound requests (never in
/// the persisted transcript — the transcript keeps the full text the user
/// saw). Tells the model the failure happened without handing it the broken
/// trajectory to anchor on (arXiv:2605.08563).
pub(crate) const CONTAMINATED_SCRUB_MARKER: &str =
    "(previous attempt was cut off by a stream error; partial output omitted — start fresh)";

/// Rounds a tool result must age out before the stale-output mask may claim
/// it. Ten rounds is past the point where the model is still reasoning from
/// the raw bytes, and well before a long session's history is anywhere near
/// the compaction trigger.
const MASK_STALE_ROUNDS: usize = 10;

impl Agent {
    /// History as the model is allowed to see it: contaminated salvaged
    /// partials (arXiv:2605.08563) replaced by the one-line scrub marker, and
    /// stale at-threshold tool outputs replaced by their ARC citation stub.
    ///
    /// `self.messages` — and so the persisted transcript — keeps the full
    /// text the user saw; only outbound requests and the compaction input
    /// use this view. Compaction has to run on the same view: summarizing the
    /// *unscrubbed* history baked the failed partial into the summary that
    /// every later request then reads, which is exactly the broken trajectory
    /// the scrub exists to keep out of the model's hands. The marker keeps
    /// the assistant role so call/result pairing and role alternation are
    /// untouched.
    pub(crate) fn scrubbed_messages(&self) -> Vec<Message> {
        let mut msgs = self.messages.clone();
        for &idx in &self.contaminated {
            if let Some(m) = msgs.get_mut(idx) {
                *m = Message::assistant(CONTAMINATED_SCRUB_MARKER);
            }
        }
        if self.masked_prefix > 0 {
            for m in msgs[..self.masked_prefix].iter_mut() {
                for b in m.content.iter_mut() {
                    if let ContentBlock::ToolResult { id, content, .. } = b
                        && content.len() >= crate::compact::ARC_STUB_MIN_BYTES
                    {
                        *content = crate::compact::tool_result_stub(
                            id,
                            content.len(),
                            self.session_id.as_deref(),
                        );
                    }
                }
            }
        }
        msgs
    }

    /// Index of the first message that has aged past [`MASK_STALE_ROUNDS`]
    /// rounds, or 0 when the transcript is younger than that.
    fn stale_boundary(&self) -> usize {
        let groups = crate::compact::atomic_groups(&self.messages);
        if groups.len() <= MASK_STALE_ROUNDS {
            return 0;
        }
        groups[..groups.len() - MASK_STALE_ROUNDS]
            .iter()
            .map(|g| g.len())
            .sum()
    }

    /// Masks every stale result now (arXiv:2607.25066). Only for a caller
    /// that knows the prompt cache is already cold: the next request re-bills
    /// the whole prefix whatever it contains, so rewriting it is free and the
    /// shorter prompt is what gets cached from here on. Never mid-turn — a
    /// prefix rewrite on a warm cache re-bills the whole prefix.
    pub fn mask_stale_tool_output(&mut self) {
        self.masked_prefix = self.masked_prefix.max(self.stale_boundary());
    }
}

/// True when a message carries no billable input: empty content, or nothing
/// but blank `Text` blocks. Any non-text block (image, tool use/result,
/// thinking) counts as input — never silently dropped.
fn is_blank_input(msg: &Message) -> bool {
    msg.content.iter().all(|b| match b {
        ContentBlock::Text { text } => text.trim().is_empty(),
        _ => false,
    })
}

impl Agent {
    /// Append observations only with all outstanding tool results settled.
    fn collect_background_notifications(&mut self, ctx: &ToolContext) -> bool {
        let notices = self.executor.drain_notifications(ctx);
        let any = !notices.is_empty();
        for notice in notices {
            self.messages.push(Message::user(format!(
                "[Background task notification]\n{notice}"
            )));
        }
        any
    }

    /// Best-effort `turn_end` fan-out: hook failures must never fail the turn
    /// (hooks are infallible by signature, same as `tool_before`).
    async fn emit_turn_end(&self, usage: &Usage) {
        for hook in &self.hooks {
            hook.turn_end(usage).await;
        }
    }

    /// Validation + `tool/before`/`pre_tool` pre-pass shared by both dispatch
    /// lanes. `Err` is a ready-to-record error result (unknown tool,
    /// non-object args, or a hook deny); `Ok` carries the possibly
    /// hook-rewritten args the executor may run.
    async fn preflight(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolOutput> {
        if !self.tools.iter().any(|t| t.name == name) {
            let list = if self.tools.is_empty() {
                "(none)".to_string()
            } else {
                self.tools
                    .iter()
                    .map(|t| t.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return Err(ToolOutput::error(format!(
                "Tool '{name}' does not exist. Available: {list}"
            )));
        }
        if !args.is_object() {
            return Err(ToolOutput::error(format!(
                "Invalid arguments for tool '{name}': expected a JSON object. Please provide a valid JSON object."
            )));
        }
        let mut effective_args = args.clone();
        for hook in &self.hooks {
            match hook.tool_before(name, &effective_args).await {
                ToolBefore::Allow => {}
                ToolBefore::Modify(rewritten) => effective_args = rewritten,
                ToolBefore::Deny(reason) => return Err(ToolOutput::error(reason)),
            }
        }
        for hook in &self.hooks {
            hook.pre_tool(name, &effective_args).await;
        }
        Ok(effective_args)
    }

    /// Runs the agent loop starting from `input`, returning every event
    /// emitted along the way.
    ///
    /// Blank input is a no-op: rejected before it touches history.
    ///
    /// Per turn: build a [`ChatRequest`], stream the response while
    /// forwarding `TextDelta`s in arrival order, finalize the assistant
    /// message, then execute any requested tools sequentially and feed their
    /// outputs back as tool-result messages. Stops when a turn ends without
    /// tool calls (`TurnEnd`) or when cancellation fires
    /// ([`CoreError::Cancelled`]). 6 identical consecutive tool calls abort
    /// with [`CoreError::LoopDetected`]. A tool-free ending whose text still
    /// announces a next step is nudged once before the stop is honored.
    /// Tool failures are *not* errors: they become `is_error` tool results
    /// so the model can recover.
    pub async fn run(
        &mut self,
        input: Message,
        ctx: ToolContext,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        if is_blank_input(&input) {
            return Ok(vec![]);
        }
        self.messages.push(input);
        self.run_inner(ctx, None).await
    }

    /// Streaming variant of [`run`]: every [`AgentEvent`] is handed to
    /// `on_event` the moment it is produced (text deltas arrive token-by-token)
    /// *and* collected into the returned Vec, which equals `run`'s output.
    pub async fn run_streaming(
        &mut self,
        input: Message,
        ctx: ToolContext,
        on_event: &mut dyn FnMut(&AgentEvent),
    ) -> Result<Vec<AgentEvent>, CoreError> {
        if is_blank_input(&input) {
            return Ok(vec![]);
        }
        self.messages.push(input);
        self.run_inner(ctx, Some(on_event)).await
    }

    async fn run_inner(
        &mut self,
        ctx: ToolContext,
        mut sink: Option<&mut dyn FnMut(&AgentEvent)>,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        // The session id rides the per-run context; compaction cites it in
        // elided-output stubs.
        self.session_id = ctx.session_id.clone();
        log::info!(target: "gray_agent", "agent run start ({} messages)", self.messages.len());
        let mut events = Vec::new();
        // Loop backstop: 6 identical consecutive tool calls → LoopDetected.
        let mut last_sig: Option<String> = None;
        let mut repeat: usize = 0;
        // A turn whose every round is a job-progress notice clears the streak
        // below, so it would otherwise bill forever on a no-timeout job.
        let mut poll_rounds: usize = 0;
        // Forward each event to the optional streaming sink, then collect it.
        macro_rules! emit {
            ($ev:expr) => {{
                let ev = $ev;
                if let Some(cb) = sink.as_deref_mut() {
                    cb(&ev);
                }
                events.push(ev);
            }};
        }
        emit!(AgentEvent::Start);
        let mut total_usage = Usage::default();
        // Billable turn totals: every provider request bills its full
        // input, so turn_end/cost accounting sums every round's report.
        // total_usage stays the context gauge (opencode parity: the latest
        // round's report only — each round's input already contains the
        // whole history, so summing outputs across rounds double-counts
        // and the gauge blows up superlinearly).
        let mut billed = Usage::default();
        let mut empty_retries: u8 = 0;
        let mut continuations: u8 = 0;
        // Whole-turn retries of a request the provider never answered (see
        // MAX_TURN_RETRIES). Reset per run; shared by every round in the run
        // so a pathological provider cannot loop forever across rounds.
        let mut turn_retries: u8 = 0;
        // Post-tool empty nudge fires once per run; silent-retry budget unchanged.
        let mut empty_nudge_sent = false;
        // An ending that announces a step the model never ran ("Let me check
        // …" then EndTurn, no tool call) gets one nudge per run; a repeat is
        // respected — the model may have reconsidered mid-sentence.
        let mut intent_nudge_sent = false;

        // Protocol v1 `prompt/context`: fetched once per turn, not per round,
        // so the system prefix stays byte-stable across a turn's requests
        // (provider prefix caching survives multi-round turns) and sidecar
        // hooks pay one call per turn instead of one per tool round.
        let mut hook_context = String::new();
        if let Some(hook) = self.checkpoint.clone() {
            hook(&self.messages, self.history_revision()).await;
        }
        for hook in &self.hooks {
            if let Some(text) = hook.prompt_context().await
                && !text.trim().is_empty()
            {
                if !hook_context.is_empty() {
                    hook_context.push_str("\n\n");
                }
                hook_context.push_str(&text);
            }
        }

        // Capture before pre-turn compaction so its request uses this same prefix.
        let mut system = self.system.clone();
        if !hook_context.is_empty() {
            if !system.is_empty() {
                system.push_str("\n\n");
            }
            system.push_str(&hook_context);
        }
        self.turn_system = Some(system);

        'turn: loop {
            // Cancellation is honored between turns, never mid-stream: a
            // half-finished assistant message would leave the transcript
            // inconsistent for the provider.
            if ctx.cancel.is_cancelled() {
                self.emit_turn_end(&billed).await;
                return Err(CoreError::Cancelled);
            }

            self.collect_background_notifications(&ctx);

            // Mid-turn steer: text the user typed while this turn was running
            // joins it here, at the boundary between two model requests, so it
            // can never land inside a stream or between a tool call and its
            // result. Steering only appends -- cancelling stays Ctrl-C/Esc, and
            // the turn keeps everything it has already done.
            if let Some(steer) = self.steer.clone()
                && let Some(text) = steer()
            {
                self.messages.push(Message::user(text));
            }

            // Step boundary: everything so far is consistent, hand it to the
            // host to persist before the next (slow, killable) request. Runs
            // after notification collection and steering so a kill between the
            // append and the checkpoint cannot drop the queued message.
            if let Some(hook) = self.checkpoint.clone() {
                hook(&self.messages, self.history_revision()).await;
            }

            // Pre-turn budget (pi `_compactBeforeNextAssistantResponse`):
            // history is append-only (mini-swe-agent / pi), so each request
            // extends the previous one and the provider prefix cache stays
            // hot. Compaction is the only rewrite, and it runs only when the
            // provider-anchored estimate reaches the window.
            if needs_pre_turn_compact(
                self.estimate_tokens(),
                self.context_window,
                self.compact_reserve,
            ) {
                // False = nothing to gain (all tail): fall through; the provider's
                // own overflow path remains the backstop. Success strictly shrinks
                // history, so re-check without looping forever. Errors finalize
                // the turn first: no silent exit without turn_end.
                while needs_pre_turn_compact(
                    self.estimate_tokens(),
                    self.context_window,
                    self.compact_reserve,
                ) {
                    let before = (self.estimate_tokens(), self.messages.len());
                    match self.try_compact_budgeted().await {
                        Ok(true) => {
                            emit!(AgentEvent::compacted(
                                before.0,
                                self.estimate_tokens(),
                                before.1,
                                self.messages.len()
                            ));
                            continue;
                        }
                        Ok(false) => break,
                        Err(e) => {
                            self.emit_turn_end(&billed).await;
                            return Err(e);
                        }
                    }
                }
            }
            // CCRM scrub (arXiv:2605.08563): contaminated partials stay in
            // `self.messages` (and so in the persisted transcript) but never
            // ride an outbound request — the retry starts from a clean
            // context with a one-line marker where the failure was.
            let mut request_messages = self.scrubbed_messages();
            if request_messages
                .last()
                .is_some_and(|message| message.role == Role::Assistant)
            {
                request_messages.push(Message::user(
                    "Continue the pending user request using the retained context and summary.",
                ));
            }
            let req = ChatRequest {
                system: (!self.system_text().is_empty()).then(|| self.system_text().to_string()),
                messages: request_messages,
                tools: self.tools.clone(),
                max_tokens: None,
            };
            // The exact request, kept for a cache refresh while this round's
            // tools run (pi cache warming). Only cloned when warming is on.
            let warm_req = self.cache_warm.is_some().then(|| req.clone());
            let request_sent = tokio::time::Instant::now();

            // Accumulate streamed deltas: text chunks in order, tool calls
            // keyed by their stream index (id/name arrive once, arguments
            // may be split across many deltas).
            let mut text_parts: Vec<String> = Vec::new();
            let mut thinking_text = String::new();
            // (item_id, encrypted_content) of the latest Responses
            // reasoning item — attached to the Thinking block at finalize so
            // the next turn can replay it verbatim (cache warmth).
            let mut pending_reasoning: Option<(String, String)> = None;
            let mut pending: Vec<PendingToolCall> = Vec::new();
            let (stop_reason, usage) = {
                let mut stream = self.provider.stream(req);
                loop {
                    let next_event = tokio::select! {
                        ev = stream.next() => ev,
                        _ = ctx.cancel.cancelled() => {
                            if !text_parts.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_text.clone(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );
                            }
                            self.emit_turn_end(&billed).await;
                            return Err(CoreError::Cancelled);
                        }
                    };
                    // Absolute event-count backstop: a hostile/broken server
                    // must not grow the retained turn without bound. Raised
                    // from 100k after benchmark runs died here: deltas are
                    // chunks, so ~100k events is only ~250-400k output tokens
                    // — a long legitimate turn. 500k still bounds the Vec
                    // (~40 MB) while leaving long runs headroom.
                    const MAX_EVENTS: usize = 500_000;
                    if events.len() >= MAX_EVENTS {
                        self.emit_turn_end(&billed).await;
                        return Err(CoreError::Provider("turn event limit exceeded".into()));
                    }
                    match next_event {
                        Some(Ok(StreamEvent::TextDelta { delta })) => {
                            emit!(AgentEvent::text_delta(delta.clone()));
                            text_parts.push(delta);
                        }
                        Some(Ok(StreamEvent::ThinkingDelta { delta })) => {
                            emit!(AgentEvent::thinking_delta(delta.clone()));
                            append_thinking_chunk(&mut thinking_text, &delta);
                        }
                        Some(Ok(StreamEvent::ReasoningItem {
                            item_id,
                            encrypted_content,
                        })) => {
                            pending_reasoning = Some((item_id, encrypted_content));
                        }
                        Some(Ok(StreamEvent::ToolCallDelta {
                            index,
                            id,
                            name,
                            arguments_delta,
                        })) => {
                            // cap wire-controlled indices — a hostile/broken server
                            // sending index = 2^40 would otherwise allocate gigabytes here.
                            const MAX_TOOL_CALL_INDEX: usize = 4096;
                            if index > MAX_TOOL_CALL_INDEX {
                                self.emit_turn_end(&billed).await;
                                return Err(CoreError::Provider(format!(
                                    "tool-call index {index} exceeds limit ({MAX_TOOL_CALL_INDEX})"
                                )));
                            }
                            while pending.len() <= index {
                                pending.push(PendingToolCall::default());
                            }
                            let slot = &mut pending[index];
                            // Stream-identity hardening: id and name are
                            // immutable once set (blank counts as unset). A
                            // conflicting re-set is a provider protocol error.
                            if let Some(new_id) = id
                                && !new_id.trim().is_empty()
                            {
                                if let Some(existing) = slot.id.as_ref() {
                                    if existing != &new_id {
                                        self.emit_turn_end(&billed).await;
                                        return Err(CoreError::Provider(format!(
                                            "provider changed tool-call id for index {index}: {existing:?} -> {new_id:?}"
                                        )));
                                    }
                                } else {
                                    slot.id = Some(new_id);
                                }
                            }
                            if let Some(new_name) = name
                                && !new_name.trim().is_empty()
                            {
                                if let Some(existing) = slot.name.as_ref() {
                                    if existing != &new_name {
                                        self.emit_turn_end(&billed).await;
                                        return Err(CoreError::Provider(format!(
                                            "provider changed tool-call name for index {index}: {existing:?} -> {new_name:?}"
                                        )));
                                    }
                                } else {
                                    slot.name = Some(new_name);
                                }
                            }
                            slot.arguments.push_str(&arguments_delta);
                            // Live emit: only once BOTH id and name are known
                            // non-blank, so the start ID can never mutate.
                            // Args arriving early stay buffered in `slot`.
                            if !slot.started
                                && slot.id.as_ref().is_some_and(|s| !s.trim().is_empty())
                                && slot.name.as_ref().is_some_and(|s| !s.trim().is_empty())
                            {
                                let live_id = slot.id.clone().unwrap();
                                let live_name = slot.name.clone().unwrap();
                                emit!(AgentEvent::tool_call_start(live_id, live_name));
                                slot.started = true;
                            }
                            // pi `updateArgs`: keep streaming partial args so the
                            // TUI can render the tool call live instead of
                            // popping it in only at ToolCallEnd/ToolResult.
                            // Forward every update to the live sink, but retain
                            // only the latest snapshot per call: retaining each
                            // cumulative snapshot would grow quadratically
                            // under per-byte deltas.
                            if slot.started && !arguments_delta.is_empty() {
                                // Started implies both id and name are present
                                // (start waits for both), so no fallback here.
                                let live_id = slot.id.clone().unwrap();
                                let live_name = slot.name.clone().unwrap();
                                let ev = AgentEvent::tool_call_progress(
                                    live_id.clone(),
                                    live_name,
                                    slot.arguments.clone(),
                                );
                                if let Some(cb) = sink.as_deref_mut() {
                                    cb(&ev);
                                }
                                let replace = matches!(
                                    events.last(),
                                    Some(AgentEvent::ToolCallProgress { id, .. })
                                        if *id == live_id
                                );
                                if replace {
                                    *events.last_mut().unwrap() = ev;
                                } else {
                                    events.push(ev);
                                }
                            }
                        }
                        Some(Ok(StreamEvent::MessageComplete { stop_reason, usage })) => {
                            break (
                                stop_reason.unwrap_or(StopReason::EndTurn),
                                usage.unwrap_or_default(),
                            );
                        }
                        // Codex steal: retry notices ride as Ok so the turn
                        // keeps going; forward live so UI shows Reconnecting.
                        Some(Ok(StreamEvent::StreamError { message, details })) => {
                            emit!(AgentEvent::stream_error(message.clone(), details.clone()));
                        }
                        Some(Err(e)) => {
                            // Whole-turn retry: the provider exhausted its own
                            // 5-attempt budget on a retryable failure (bursts of
                            // 503s do this in the wild) and nothing was streamed
                            // yet, so replaying the request is invisible to the
                            // user. A visible delta forbids the replay (the loop
                            // has committed it to history; the provider's
                            // partial-answer path owns that case) and so does a
                            // cancelled token.
                            if e.retryable()
                                && text_parts.is_empty()
                                && !ctx.cancel.is_cancelled()
                                && turn_retries < MAX_TURN_RETRIES
                            {
                                turn_retries += 1;
                                let delay = if turn_retries > 1 {
                                    turn_retry_delay() * 2
                                } else {
                                    turn_retry_delay()
                                };
                                log::warn!(
                                    target: "gray_agent",
                                    "turn failed retryably with nothing streamed; whole-turn retry {turn_retries}/{MAX_TURN_RETRIES} in {delay:?}: {e}"
                                );
                                if !delay.is_zero() {
                                    tokio::time::sleep(delay).await;
                                }
                                if ctx.cancel.is_cancelled() {
                                    self.emit_turn_end(&billed).await;
                                    return Err(CoreError::Cancelled);
                                }
                                continue 'turn;
                            }
                            // Mid-stream failure after deltas already reached the
                            // user's screen: salvage the partial assistant text
                            // into history so the transcript matches what was seen.
                            if !text_parts.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_text.clone(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );
                                // CCRM (arXiv:2605.08563): the salvaged
                                // partial is a failed attempt; flag it so the
                                // next request scrubs it (see request build).
                                // A successful compaction below clears the
                                // flag via the history rewrite.
                                self.contaminated.insert(self.messages.len() - 1);
                            }
                            // Context overflow: compact via the budgeted
                            // compaction pipeline, then retry the turn;
                            // otherwise surface the error.
                            if e.should_compress() {
                                let before = (self.estimate_tokens(), self.messages.len());
                                match self.try_compact_budgeted().await {
                                    Ok(true) => {
                                        emit!(AgentEvent::compacted(
                                            before.0,
                                            self.estimate_tokens(),
                                            before.1,
                                            self.messages.len()
                                        ));
                                        continue 'turn;
                                    }
                                    _ => {
                                        let err = CoreError::from(e);
                                        self.emit_turn_end(&billed).await;
                                        return Err(err);
                                    }
                                }
                            }
                            let err = CoreError::from(e);
                            self.emit_turn_end(&billed).await;
                            return Err(err);
                        }
                        None => {
                            // Provider closed without a completion event: never
                            // treat that as success. Text-only partial output
                            // is salvaged (marked interrupted); pending tool
                            // calls are NOT executed from a truncated stream.
                            if !text_parts.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_text.clone(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );
                            }
                            self.emit_turn_end(&billed).await;
                            return Err(CoreError::Provider(
                                "provider stream ended without completion".into(),
                            ));
                        }
                    }
                }
            };
            // Context gauge: latest report wins wholesale (opencode parity:
            // its sidebar/footer read the last assistant message's usage,
            // never a sum). A round with no usage signal keeps the previous
            // gauge. Billing stays cumulative below.
            if usage.input_tokens != 0
                || usage.output_tokens != 0
                || usage.cached_tokens != 0
                || usage.non_cached_input_tokens != 0
                || usage.cache_read_input_tokens != 0
                || usage.cache_write_input_tokens != 0
                || usage.reasoning_tokens != 0
                || usage.total_tokens != 0
            {
                total_usage = usage;
                total_usage.normalize();
            }
            billed.accumulate(&usage);
            usage.log_request("turn");
            // Stream-identity hardening: calls that never got a provider ID
            // get a conversation-unique fallback now and emit their single
            // start here, so every dispatched call already has its start and
            // dispatch never consults positional side tables.
            {
                let fallback_base = self.messages.len();
                for (wire_index, slot) in pending.iter_mut().enumerate() {
                    if !slot.name.as_ref().is_some_and(|s| !s.trim().is_empty()) {
                        continue;
                    }
                    if !slot.id.as_ref().is_some_and(|s| !s.trim().is_empty()) {
                        slot.id = Some(format!("gray_call_{fallback_base}_{wire_index}"));
                    }
                    if !slot.started {
                        let id = slot.id.clone().unwrap();
                        let name = slot.name.clone().unwrap();
                        emit!(AgentEvent::tool_call_start(id, name));
                        slot.started = true;
                    }
                }
            }
            emit!(AgentEvent::StepUsage { usage: total_usage });

            // Finalize the assistant message exactly as streamed.
            // Reasoning precedes text, mirroring the provider's emission order
            // (pi renders runs of thinking blocks ahead of prose).
            let mut content: Vec<ContentBlock> = Vec::new();
            let thinking = std::mem::take(&mut thinking_text);
            if !thinking.is_empty() {
                content.push(thinking_block(
                    thinking,
                    &pending_reasoning,
                    self.provider.model_id(),
                ));
            }
            let text = text_parts.concat();
            let text_is_empty = text.is_empty();
            // Evaluated before `text` moves into the content block; used only
            // on the no-tool end path below. A leaked tool-call envelope
            // (funnel fence / native markup the provider left as text) counts
            // as an announced step: the model tried to call, the call never
            // materialized.
            let announced_step = matches!(stop_reason, StopReason::EndTurn | StopReason::ToolUse)
                && (announced_unfinished_step(&text) || leaked_call_markup(&text));
            if !text.is_empty() {
                content.push(ContentBlock::Text { text });
            }
            for (index, call) in pending.iter().enumerate() {
                let name = match call.name.as_ref() {
                    Some(n) if !n.trim().is_empty() => n.clone(),
                    _ => {
                        log::warn!(target: "gray_agent", "dropping tool call index {index} with empty name (args: {})", call.arguments.chars().take(200).collect::<String>());
                        continue;
                    }
                };
                // IDs are final here: provider IDs from the stream or the
                // gray_call_* fallback assigned above — never a positional default.
                let Some(id) = call.id.clone().filter(|s| !s.trim().is_empty()) else {
                    log::warn!(target: "gray_agent", "dropping tool call index {index} with empty id after fallback");
                    continue;
                };
                content.push(ContentBlock::ToolUse {
                    id,
                    name,
                    args: call.parsed_args(),
                });
            }
            let assistant = Message {
                role: Role::Assistant,
                content,
            };
            self.messages.push(assistant.clone());
            // This round's report covers system + tools + history through the
            // assistant message just pushed (pi `estimateContextTokens`).
            self.record_context_usage(&usage);

            let tool_uses: Vec<(String, String, serde_json::Value)> = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, args } => {
                        Some((id.clone(), name.clone(), args.clone()))
                    }
                    _ => None,
                })
                .collect();

            // Truncated turn with a usable fragment: ask to continue where it
            // left off (capped); past the cap the stitched partial stands.
            if stop_reason == StopReason::MaxTokens
                && !text_is_empty
                && tool_uses.is_empty()
                && continuations < MAX_CONTINUATIONS
            {
                continuations += 1;
                self.messages.push(Message::user(
                    "previous response truncated — continue exactly where you left off",
                ));
                continue 'turn;
            }

            // Empty turn (no text, no tool calls): retry the provider call,
            // then nudge once after tool results, else end on `(empty)`.
            // A reasoning-only response cut by the output limit is NOT an
            // empty turn: the identical retry would hit the same limit (up to
            // 3x the max output cost, reasoning dropped). The MaxTokens branch
            // above owns the turn; ending here keeps the thinking (#99).
            if text_is_empty && tool_uses.is_empty() && stop_reason != StopReason::MaxTokens {
                self.messages.pop();
                if empty_retries < MAX_EMPTY_RETRIES {
                    empty_retries += 1;
                    continue 'turn;
                }
                let tail_had_results = self.messages.last().is_some_and(|m| {
                    m.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
                });
                if tail_had_results && !empty_nudge_sent {
                    empty_nudge_sent = true;
                    self.messages.push(Message::assistant("(empty)"));
                    self.messages.push(Message::user(
                        "executed tool calls but returned empty — process results and continue",
                    ));
                    continue 'turn;
                }
                self.messages.push(Message::assistant("(empty)"));
            }

            if tool_uses.is_empty() {
                // A text-only ending whose tail commits to a step the model
                // never ran looks like the run died mid-thought: the footer
                // prints and the user has to type "." to revive it. Nudge
                // once so the step happens or the model says it's done.
                if announced_step && !intent_nudge_sent {
                    intent_nudge_sent = true;
                    log::info!(target: "gray_agent", "end turn announced an untaken step; nudging once");
                    self.messages.push(Message::user(
                        "you ended your turn right after announcing a next step — take it now, or state that the task is complete; if that step waits on the user's answer or approval, say so in one line and stop",
                    ));
                    continue 'turn;
                }
                // A job that finished during inference gets a follow-up now;
                // unfinished jobs never hold this turn open.
                if self.collect_background_notifications(&ctx) {
                    continue 'turn;
                }
                let hit = if total_usage.input_tokens > 0 {
                    total_usage.cached_tokens as f64 / total_usage.input_tokens as f64 * 100.0
                } else {
                    0.0
                };
                log::info!(target: "gray_agent", "agent run end: session={} stop={stop_reason:?}, usage in={} out={} cached={} hit={:.0}%, {} messages", ctx.session_id.as_deref().unwrap_or("-"), total_usage.input_tokens, total_usage.output_tokens, total_usage.cached_tokens, hit, self.messages.len());
                emit!(AgentEvent::turn_end(stop_reason, billed));
                self.emit_turn_end(&billed).await;
                return Ok(events);
            }

            // Loop backstop: no nudge text, just stop if the identical call
            // keeps coming. Deliberate job polls reset the streak below.
            const SIGNATURE_ABORT_REPEATS: usize = 6;
            {
                let sig = tool_uses
                    .iter()
                    .map(|(_, n, a)| format!("{n}:{a}"))
                    .collect::<Vec<_>>()
                    .join("|");
                if last_sig.as_deref() == Some(&sig) {
                    repeat += 1;
                } else {
                    last_sig = Some(sig.clone());
                    repeat = 1;
                }
                if repeat >= SIGNATURE_ABORT_REPEATS {
                    answer_pending_tools(self, &tool_uses, 0, "aborted: tool loop detected");
                    self.emit_turn_end(&billed).await;
                    return Err(CoreError::LoopDetected(format!(
                        "same tool call {repeat}× in a row: {sig}"
                    )));
                }
            }

            // W3 dispatch validation: unknown tools and malformed args never
            // reach the executor; each still gets one error tool result so the
            // assistant/user alternation stays intact.
            // Parallel batch lane: a maximal run of batchable-known-object
            // calls executes concurrently via `join_ordered`; everything else
            // keeps the sequential path below, verbatim. Kill-switch off (or
            // any barrier) degrades to all-`Single` — today's loop exactly.
            let known: std::collections::HashSet<String> =
                self.tools.iter().map(|t| t.name.clone()).collect();
            // Everything pushed from here on is this round's tool results;
            // the poll check below reads them back by index.
            let round_start = self.messages.len();
            let segments = if crate::parallel::parallel_enabled() {
                crate::parallel::plan_segments(&tool_uses, &known)
            } else {
                tool_uses
                    .iter()
                    .enumerate()
                    .map(|(i, _)| crate::parallel::Segment::Single(i))
                    .collect()
            };
            let warm_spent = std::sync::Arc::new(std::sync::Mutex::new(Usage::default()));
            let warm_guard = match (&self.cache_warm, warm_req) {
                (Some(policy), Some(req)) => Some(crate::cache_warm::WarmGuard(tokio::spawn(
                    crate::cache_warm::keep_warm(
                        self.provider.clone(),
                        req,
                        policy.clone(),
                        total_usage.input_tokens,
                        total_usage.cache_read_input_tokens
                            + total_usage.cached_tokens
                            + total_usage.cache_write_input_tokens
                            > 0,
                        request_sent,
                        warm_spent.clone(),
                    ),
                ))),
                _ => None,
            };
            for segment in segments {
                // Parallel run over `tool_uses` indices. Pre-pass (loop
                // thread, in order): cancel check, validation, `tool_before`
                // verdicts, `pre_tool` hooks. Only `executor.execute` runs
                // concurrently — never `ApprovalGate::check`: batchable names
                // are statically `Allow` in every mode. `tool_call_end` for
                // every ready member fires before the run starts (`start`
                // already emitted live/at-finalize with the final ID).
                // Post-pass (loop thread, input order): `post_tool` hooks,
                // `tool_result` events, history writes.
                if let crate::parallel::Segment::Parallel(idxs) = segment {
                    let (Some(run_start), Some(run_end)) =
                        (idxs.first().copied(), idxs.last().map(|i| *i + 1))
                    else {
                        continue;
                    };
                    let mut ready: Vec<(usize, String, serde_json::Value)> =
                        Vec::with_capacity(idxs.len());
                    let mut inline_errors: std::collections::HashMap<usize, ToolOutput> =
                        std::collections::HashMap::new();
                    let mut cancelled_at: Option<usize> = None;
                    for &idx in &idxs {
                        let (_, name, args) = &tool_uses[idx];
                        if ctx.cancel.is_cancelled() {
                            cancelled_at = Some(idx);
                            break;
                        }
                        match self.preflight(name, args).await {
                            Ok(effective_args) => ready.push((idx, name.clone(), effective_args)),
                            Err(err) => {
                                inline_errors.insert(idx, err);
                            }
                        }
                    }
                    if cancelled_at.is_some() {
                        // Nothing in the run executed: the pre-pass only
                        // validates into `ready`/`inline_errors` and emits
                        // nothing, so no run index has a result yet — backfill
                        // the whole run exactly once, then everything after
                        // it, and bail. History must never hold an orphaned call.
                        crate::agent_tools::answer_pending_range(
                            self,
                            &tool_uses,
                            run_start,
                            run_end,
                            "cancelled by user",
                        );
                        answer_pending_tools(self, &tool_uses, run_end, "cancelled by user");
                        self.emit_turn_end(&billed).await;
                        return Err(CoreError::Cancelled);
                    }
                    // `tool_call_end` is the "args complete, executing" signal
                    // (the REPL flips the card to running and the status off
                    // "Preparing tool"): emit it for every member now, before
                    // the run starts — not after the slowest one finishes.
                    for (idx, _, _) in &ready {
                        let (id, _, args) = &tool_uses[*idx];
                        emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));
                    }
                    // Spawn: one future per ready item. Everything the future
                    // touches is owned (`'static`): no borrow of `self`
                    // escapes the loop thread.
                    let timeout = self.tool_timeout;
                    let mut futs = Vec::with_capacity(ready.len());
                    for (idx, name, effective_args) in ready {
                        let ex = self.executor.clone();
                        let c = ctx.clone();
                        futs.push((
                            idx,
                            Box::pin(async move {
                                match tokio::time::timeout(
                                    timeout,
                                    ex.execute(&c, &name, effective_args),
                                )
                                .await
                                {
                                    Ok(output) => output,
                                    Err(_) => ToolOutput::error(format!(
                                        "Tool '{name}' timed out after {}s",
                                        timeout.as_secs()
                                    )),
                                }
                            })
                                as futures::future::BoxFuture<'static, ToolOutput>,
                        ));
                    }
                    let joined = crate::parallel::join_ordered(futs, &ctx.cancel).await;
                    // Reconcile by real index (no sentinel exists: every
                    // entry carries its input index; panics arrive as error
                    // outputs). `None`/absent means cancelled
                    // pre-completion → synthetic backfill, message only.
                    let by_idx: std::collections::HashMap<usize, Option<ToolOutput>> =
                        joined.into_iter().collect();
                    let mut shortfall = false;
                    for &idx in &idxs {
                        let (id, name, args) = &tool_uses[idx];
                        if let Some(err) = inline_errors.remove(&idx) {
                            // `start` already emitted with the final ID (live
                            // or at finalize); dispatch only ends the call.
                            emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));
                            emit!(AgentEvent::tool_result(
                                id.clone(),
                                err.content.clone(),
                                true
                            ));
                            self.messages.push(Message {
                                role: Role::User,
                                content: vec![ContentBlock::ToolResult {
                                    id: id.clone(),
                                    content: err.content,
                                    is_error: true,
                                }],
                            });
                            continue;
                        }
                        match by_idx.get(&idx) {
                            Some(Some(output)) => {
                                for hook in &self.hooks {
                                    hook.post_tool(name, output).await;
                                }
                                emit!(AgentEvent::tool_result(
                                    id.clone(),
                                    output.content.clone(),
                                    output.is_error,
                                ));
                                self.messages.push(Message {
                                    role: Role::User,
                                    content: output.message_blocks(id),
                                });
                            }
                            _ => {
                                shortfall = true;
                                crate::agent_tools::push_synthetic(self, id, "cancelled by user");
                            }
                        }
                    }
                    if shortfall {
                        answer_pending_tools(self, &tool_uses, run_end, "cancelled by user");
                        self.emit_turn_end(&billed).await;
                        return Err(CoreError::Cancelled);
                    }
                    continue;
                }
                let crate::parallel::Segment::Single(idx) = segment else {
                    continue;
                };
                let (id, name, args) = &tool_uses[idx];
                if ctx.cancel.is_cancelled() {
                    answer_pending_tools(self, &tool_uses, idx, "cancelled by user");
                    self.emit_turn_end(&billed).await;
                    return Err(CoreError::Cancelled);
                }
                // `start` already emitted with the final ID (live or at
                // finalize); dispatch only ends the call — no positional side
                // tables (wire vs filtered index spaces must never mix).
                emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));

                let effective_args = match self.preflight(name, args).await {
                    Ok(effective_args) => effective_args,
                    Err(err) => {
                        emit!(AgentEvent::tool_result(
                            id.clone(),
                            err.content.clone(),
                            true
                        ));
                        self.messages.push(Message {
                            role: Role::User,
                            content: vec![ContentBlock::ToolResult {
                                id: id.clone(),
                                content: err.content,
                                is_error: true,
                            }],
                        });
                        continue;
                    }
                };
                // Pin the tool future rather than moving it into the
                // timeout: a cancel must be able to bound-wait on it.
                let mut exec_fut = Box::pin(self.executor.execute(&ctx, name, effective_args));
                let output = tokio::select! {
                    out = tokio::time::timeout(self.tool_timeout, &mut exec_fut) => match out {
                        Ok(output) => output,
                        Err(_) => ToolOutput::error(format!(
                            "Tool '{name}' timed out after {}s",
                            self.tool_timeout.as_secs()
                        )),
                    },
                    _ = ctx.cancel.cancelled() => {
                        // The tool shares `ctx`, so it is already signalled
                        // and owns the cleanup that matters (kill its process
                        // group, drain, report partial output). Wait out that
                        // report instead of dropping the future and answering
                        // with a bare synthetic string: a cancel must never be
                        // the reason output is lost.
                        let report = match tokio::time::timeout(
                            crate::parallel::CANCEL_REPORT_GRACE,
                            &mut exec_fut,
                        )
                        .await
                        {
                            Ok(report) => report,
                            Err(_) => ToolOutput::error("cancelled by user"),
                        };
                        for hook in &self.hooks {
                            hook.post_tool(name, &report).await;
                        }
                        emit!(AgentEvent::tool_result(
                            id.clone(),
                            report.content.clone(),
                            report.is_error,
                        ));
                        self.messages.push(Message {
                            role: Role::User,
                            content: vec![ContentBlock::ToolResult {
                                id: id.clone(),
                                content: report.content,
                                is_error: report.is_error,
                            }],
                        });
                        // This call now has a real result; only the calls
                        // after it still need the synthetic backfill.
                        answer_pending_tools(self, &tool_uses, idx + 1, "cancelled by user");
                        self.emit_turn_end(&billed).await;
                        return Err(CoreError::Cancelled);
                    },
                };
                for hook in &self.hooks {
                    hook.post_tool(name, &output).await;
                }

                emit!(AgentEvent::tool_result(
                    id.clone(),
                    output.content.clone(),
                    output.is_error,
                ));
                self.messages.push(Message {
                    role: Role::User,
                    content: output.message_blocks(id),
                });
            }

            // Tools done: stop warming and bill what the refreshes cost.
            drop(warm_guard);
            if let Ok(spent) = warm_spent.lock() {
                billed.accumulate(&spent);
            }

            // A round whose every result is a "job still running" notice is a
            // deliberate poll, not a stall: gray answers a repeated command
            // with the live status of the job it already started, so the
            // elapsed time moves and the model is waiting on real work. Clear
            // the signature streak for those rounds — a genuinely stuck call
            // returns ordinary output and still trips the guard.
            let poll_round = !self.messages[round_start..].is_empty()
                && self.messages[round_start..].iter().all(|m| {
                    !m.content.is_empty()
                        && m.content.iter().all(|b| match b {
                            ContentBlock::ToolResult { content, .. } => is_job_progress(content),
                            _ => false,
                        })
                });
            if poll_round {
                poll_rounds += 1;
                const MAX_POLL_ROUNDS: usize = 20;
                if poll_rounds >= MAX_POLL_ROUNDS {
                    self.emit_turn_end(&billed).await;
                    return Err(CoreError::LoopDetected(format!(
                        "job polling exceeded {poll_rounds} consecutive rounds"
                    )));
                }
                repeat = 0;
                last_sig = None;
            } else {
                poll_rounds = 0;
            }
        }
    }
}

/// True when a text-only ending carries raw tool-call/template markup the
/// provider never turned into calls: a fenced `gray_calls` block (the
/// text-funnel contract some providers front), native pipe-delimited call
/// tokens, or hallucinated `[User]`/`[Assistant]` transcript turns. The
/// model believed it called a tool; ending here looks like dying mid-task.
fn leaked_call_markup(text: &str) -> bool {
    if text.contains("```gray_calls")
        || text.contains("\n[User]\n")
        || text.contains("\n[Assistant]\n")
    {
        return true;
    }
    // `<|close|>` / `<|sep|>`-family specials, built without literal pipes
    // so this file's own diff never carries the raw markup.
    const TOKENS: &[&str] = &[
        "<\x7cclose\x7c>",
        "<\x7csep\x7c>",
        "<\x7ccall\x7c>",
        "<\x7ctools\x7c>",
        "<\x7cargument\x7c>",
        "<\x7cstart\x7c>",
        "<\x7cend\x7c>",
    ];
    TOKENS.iter().filter(|t| text.contains(**t)).count() >= 2
}

/// True when a round-ending text's tail commits to an action the model then
/// never performed: "Let me check the logs", "Two things to pin down: …",
/// "— searching for images …". Only the last line (truncated to 400 chars)
/// is read: intent stated earlier in a message and then resolved ("Let me
/// check. It was X.") is stale, and a trailing `?` means the turn was handed
/// to the user on purpose. User-directed, instructional and negated
/// readings ("let me know if…", "you can verify with …", "I won't …") are
/// scrubbed before the markers are checked.
fn announced_unfinished_step(text: &str) -> bool {
    let tail = text.trim_end();
    if tail.is_empty() || tail.ends_with('?') || awaits_user(tail) {
        return false;
    }
    let last_line = tail.lines().next_back().unwrap_or_default();
    let window: String = last_line
        .chars()
        .rev()
        .take(400)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let lower = window.to_lowercase().replace(['’', '‘'], "'");
    let scrubbed = lower
        .replace("let me know", "")
        .replace("i'll let you", "")
        .replace("i won't", "")
        .replace("i will not", "")
        .replace("i can't", "")
        .replace("i cannot", "")
        .replace("i couldn't", "")
        .replace("i could not", "")
        .replace("i'll skip", "")
        .replace("i'll leave", "")
        .replace("i'll stop", "")
        .replace("let's not", "")
        .replace("let's skip", "")
        // Instructions handed to the user are answers, not commitments the
        // model still owes ("run X to verify", "you can check …").
        .replace("you can ", "")
        .replace("you could ", "")
        .replace("you may ", "")
        .replace("you might ", "")
        .replace("you should ", "")
        .replace("feel free", "")
        .replace("here's how to ", "")
        .replace("here is how to ", "")
        .replace("here's how", "")
        .replace("here is how", "")
        .replace("how to ", "");
    const MARKERS: &[&str] = &[
        "let me",
        "i'll",
        "i will ",
        "i'm going to ",
        "i am going to ",
        "i need to ",
        "i have to ",
        "i'm about to ",
        "let's ",
        "time to ",
        "next step",
        // Still-mid-task declarations and pending-work enumerations —
        // the phrasing swe-2/other models actually die on.
        "i'm mid-",
        "i am mid-",
        "i'm still ",
        "i am still ",
        "still working",
        "still checking",
        "still digging",
        "still investigating",
        "still need",
        "still have to",
        "still left",
        "left to ",
        "left:",
        "remaining:",
        "remains to",
        "to do:",
        "todo:",
        "next:",
        "up next",
        "then i'll",
        "we need to",
        "we have to",
        "we should ",
        "we'll",
        "to pin down",
        "to figure out",
        "to work out",
        "to nail down",
        "to determine",
        "to investigate",
        "to dig into",
        "to look into",
        "to double-check",
        "to track down",
        "to confirm",
        "to verify",
        "to check",
        "to test",
        "to try",
        "to retry",
        "retrying",
        "trying again",
    ];
    MARKERS.iter().any(|m| scrubbed.contains(m))
}

/// True when the closing lines hand the turn back to the user: a question
/// for them, or a step gated on their answer ("say go and I'll …", "once
/// you confirm"). The announced step is then theirs to trigger, and a nudge
/// to "take it now" would act on work they never approved. Errs toward
/// ending the turn: a missed nudge costs one "." from the user, a wrong one
/// costs unapproved changes.
fn awaits_user(tail: &str) -> bool {
    let closing: Vec<&str> = tail
        .lines()
        .rev()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(8)
        .collect();
    let asks = closing.iter().any(|line| {
        line.trim_end_matches(|c: char| matches!(c, '*' | '_' | '`' | ')' | '"' | '\'' | '’'))
            .ends_with('?')
    });
    if asks {
        return true;
    }
    let lower = closing.join(" ").to_lowercase().replace(['’', '‘'], "'");
    const GATES: &[&str] = &[
        "say go",
        "your go",
        "the go-ahead",
        "your go-ahead",
        "your approval",
        "your answer",
        "your call",
        "your confirmation",
        "your decision",
        "your pick",
        "once you ",
        "when you approve",
        "when you confirm",
        "when you're ready",
        "if you approve",
        "if you confirm",
        "until you ",
        "wait for you",
        "waiting for you",
        "waiting on you",
        "i'll wait",
        "i will wait",
        "awaiting your",
        "hold off",
        "before i start",
        "before i proceed",
        "before i continue",
        "before i implement",
    ];
    GATES.iter().any(|g| lower.contains(g))
}

/// True when a tool result is gray's "the job is still going" progress
/// notice — the yield notice or a status/output poll of a live job. Both
/// carry a moving `elapsed`, so a repeated call answered with one of these
/// is a deliberate wait, never a stall.
fn is_job_progress(content: &str) -> bool {
    content.contains("· running · elapsed ") || content.contains("still running · job ")
}
