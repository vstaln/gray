//! Agent turn loop: [`Agent::run`] / [`run_streaming`](Agent::run_streaming).
//!
//! Split from `agent.rs` (move-only): the multi-turn stream → finalize →
//! dispatch cycle plus its stall guards. Shared transcript helpers live in
//! `agent.rs`, tool-call plumbing in `agent_tools`, compaction in
//! `agent_compact`.

use futures::StreamExt as _;

use crate::agent::{
    Agent, ToolBefore, ToolContext, ToolOutput, salvage_partial_text, thinking_block,
};
use crate::agent_compact::needs_pre_turn_compact;
use crate::agent_tools::{PendingToolCall, answer_pending_tools};
use crate::error::CoreError;
use crate::event::{AgentEvent, StopReason, StreamEvent, Usage};
use crate::message::{ChatRequest, ContentBlock, Message, Role};
use crate::turn_queue::{Submission, SubmitMode, TurnState};

/// Empty-turn provider retries before nudging or ending on `(empty)`.
const MAX_EMPTY_RETRIES: u8 = 2;
/// Truncated-turn (`MaxTokens`) continuations before keeping the partial.
const MAX_CONTINUATIONS: u8 = 2;

impl Agent {
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
    /// Admission first: `input` goes through [`Agent::submit`] (empty input
    /// rejected before it touches history), then the admitted turn executes.
    ///
    /// Per turn: build a [`ChatRequest`], stream the response while
    /// forwarding `TextDelta`s in arrival order, finalize the assistant
    /// message, then execute any requested tools sequentially and feed their
    /// outputs back as tool-result messages. Stops when a turn ends without
    /// tool calls (`TurnEnd`) or when cancellation fires
    /// ([`CoreError::Cancelled`]). Two stall guards abort runaway loops:
    /// 3 identical consecutive tool calls, or 18 consecutive read-only
    /// (read/ls/find/grep) rounds with no file changes — nudged at 12.
    /// Tool failures are *not* errors: they become `is_error` tool results
    /// so the model can recover.
    pub async fn run(
        &mut self,
        input: Message,
        ctx: ToolContext,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        match self.submit(input, SubmitMode::StartOrSteer) {
            Submission::Started { turn_id } => {
                self.turn_state = TurnState::Busy { turn_id };
                let out = self.run_inner(ctx, None).await;
                self.turn_state = TurnState::Idle;
                out
            }
            // Defensive only: Busy needs reentrancy, impossible under `&mut`
            // today — no turn ran, the input is already queued as steer.
            Submission::Steered { .. } => Ok(vec![]),
            // Defensive only (see above): rejection mutated nothing.
            Submission::NotSubmitted(_) => Ok(vec![]),
        }
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
        match self.submit(input, SubmitMode::StartOrSteer) {
            Submission::Started { turn_id } => {
                self.turn_state = TurnState::Busy { turn_id };
                let out = self.run_inner(ctx, Some(on_event)).await;
                self.turn_state = TurnState::Idle;
                out
            }
            // Defensive only: Busy needs reentrancy, impossible under `&mut`
            // today — no turn ran, the input is already queued as steer.
            Submission::Steered { .. } => Ok(vec![]),
            // Defensive only (see above): rejection mutated nothing.
            Submission::NotSubmitted(_) => Ok(vec![]),
        }
    }

    async fn run_inner(
        &mut self,
        ctx: ToolContext,
        mut sink: Option<&mut dyn FnMut(&AgentEvent)>,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        log::info!(target: "gray_agent", "agent run start ({} messages)", self.messages.len());
        let mut events = Vec::new();
        // Stall guard: 3 identical consecutive tool calls → LoopDetected.
        let mut last_sig: Option<String> = None;
        let mut repeat: usize = 0;
        // Exploration-stall guard: consecutive rounds using only read-only
        // lookup tools — the "keeps re-reading instead of acting" loop.
        let mut stall_rounds: usize = 0;
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
        let mut first_round = true;
        let mut empty_retries: u8 = 0;
        let mut continuations: u8 = 0;
        // Post-tool empty nudge fires once per run; silent-retry budget unchanged.
        let mut empty_nudge_sent = false;

        // Protocol v1 `prompt/context`: fetched once per turn, not per round,
        // so the system prefix stays byte-stable across a turn's requests
        // (provider prefix caching survives multi-round turns) and sidecar
        // hooks pay one call per turn instead of one per tool round.
        let mut hook_context = String::new();
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

        'turn: loop {
            // Cancellation is honored between turns, never mid-stream: a
            // half-finished assistant message would leave the transcript
            // inconsistent for the provider.
            if ctx.cancel.is_cancelled() {
                self.emit_turn_end(&billed).await;
                return Err(CoreError::Cancelled);
            }
            self.drain_steer(first_round);
            first_round = false;

            // Pre-turn budget: compact before the provider ever sees an overflow.
            if needs_pre_turn_compact(self.estimate_tokens(), self.context_window) {
                // False = nothing to gain (all tail): fall through; the provider's
                // own overflow path remains the backstop. Success strictly shrinks
                // history, so re-check without looping forever. Errors finalize
                // the turn first: no silent exit without turn_end.
                while needs_pre_turn_compact(self.estimate_tokens(), self.context_window) {
                    match self.try_compact_budgeted().await {
                        Ok(true) => continue,
                        Ok(false) => break,
                        Err(e) => {
                            self.emit_turn_end(&billed).await;
                            return Err(e);
                        }
                    }
                }
            }
            // Per-turn hook context (fetched once above) concatenates onto
            // this turn's system prompt. Empty when no hooks replied.
            let mut system = self.system.clone();
            if !hook_context.is_empty() {
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&hook_context);
            }

            let req = ChatRequest {
                system: (!system.is_empty()).then_some(system),
                messages: self.messages.clone(),
                tools: self.tools.clone(),
            };

            // Accumulate streamed deltas: text chunks in order, tool calls
            // keyed by their stream index (id/name arrive once, arguments
            // may be split across many deltas).
            let mut text_parts: Vec<String> = Vec::new();
            let mut thinking_parts: Vec<String> = Vec::new();
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
                            if !text_parts.is_empty() && pending.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_parts.concat(),
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
                    // must not grow the retained turn without bound.
                    const MAX_EVENTS: usize = 100_000;
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
                            thinking_parts.push(delta);
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
                            // Mid-stream failure after deltas already reached the
                            // user's screen: salvage the partial assistant text
                            // into history so the transcript matches what was seen.
                            if !text_parts.is_empty() && pending.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_parts.concat(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );
                            }
                            // Context overflow: compact via budgeted complete_prompt,
                            // then retry the turn; otherwise surface the error.
                            if e.should_compress() {
                                match self.try_compact_budgeted().await {
                                    Ok(true) => continue 'turn,
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
                            if !text_parts.is_empty() && pending.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_parts.concat(),
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
            let thinking = thinking_parts.concat();
            if !thinking.is_empty() {
                content.push(thinking_block(
                    thinking,
                    &pending_reasoning,
                    self.provider.model_id(),
                ));
            }
            let text = text_parts.concat();
            let text_is_empty = text.is_empty();
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
            if text_is_empty && tool_uses.is_empty() {
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
                let hit = if total_usage.input_tokens > 0 {
                    total_usage.cached_tokens as f64 / total_usage.input_tokens as f64 * 100.0
                } else {
                    0.0
                };
                log::info!(target: "gray_agent", "agent run end: stop={stop_reason:?}, usage in={} out={} cached={} hit={:.0}%, {} messages", total_usage.input_tokens, total_usage.output_tokens, total_usage.cached_tokens, hit, self.messages.len());
                emit!(AgentEvent::turn_end(stop_reason, billed));
                self.emit_turn_end(&billed).await;
                return Ok(events);
            }

            // Stall guard: abort if the same tool+args repeats 3 times consecutively.
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
                if repeat >= 3 {
                    answer_pending_tools(self, &tool_uses, 0, "aborted: tool loop detected");
                    self.emit_turn_end(&billed).await;
                    return Err(CoreError::LoopDetected(format!(
                        "same tool call 3× in a row: {sig}"
                    )));
                }
            }

            // Exploration-stall guard: a round made up entirely of read-only
            // lookup tools extends the streak; any other tool (bash, write,
            // edit, questions, …) proves progress and resets it. Nudge once,
            // then abort — a varied-args read loop never trips the signature
            // guard above but burns tokens forever otherwise.
            const STALL_NUDGE_ROUNDS: usize = 12;
            const STALL_POST_NUDGE_ROUNDS: usize = 6;
            const STALL_ABORT_ROUNDS: usize = STALL_NUDGE_ROUNDS + STALL_POST_NUDGE_ROUNDS;
            const EXPLORATION_TOOLS: [&str; 4] = ["read", "ls", "find", "grep"];
            if tool_uses
                .iter()
                .all(|(_, n, _)| EXPLORATION_TOOLS.contains(&n.as_str()))
            {
                stall_rounds += 1;
            } else {
                stall_rounds = 0;
            }
            if stall_rounds >= STALL_ABORT_ROUNDS {
                answer_pending_tools(self, &tool_uses, 0, "aborted: exploration stall");
                self.emit_turn_end(&billed).await;
                let explored: Vec<String> = tool_uses.iter().map(|(_, n, _)| n.clone()).collect();
                return Err(CoreError::LoopDetected(format!(
                    "Stopped: {stall_rounds} consecutive exploration rounds with no file changes — last round used [{}]",
                    explored.join(", ")
                )));
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
            let segments = if crate::parallel::parallel_enabled() {
                crate::parallel::plan_segments(&tool_uses, &known)
            } else {
                tool_uses
                    .iter()
                    .enumerate()
                    .map(|(i, _)| crate::parallel::Segment::Single(i))
                    .collect()
            };
            for segment in segments {
                // Parallel run over `tool_uses` indices. Pre-pass (loop
                // thread, in order): cancel check, validation, `tool_before`
                // verdicts, `pre_tool` hooks. Only `executor.execute` runs
                // concurrently — never `ApprovalGate::check`: batchable names
                // are statically `Allow` in every mode. Post-pass (loop
                // thread, input order): `tool_call_end` emission (`start`
                // already emitted live/at-finalize with the final ID),
                // `post_tool` hooks, `tool_result` events, history writes.
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
                    let joined = crate::parallel::join_ordered(
                        futs,
                        crate::parallel::MAX_WORKERS,
                        &ctx.cancel,
                    )
                    .await;
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
                                emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));
                                emit!(AgentEvent::tool_result(
                                    id.clone(),
                                    output.content.clone(),
                                    output.is_error,
                                ));
                                self.messages.push(Message {
                                    role: Role::User,
                                    content: vec![ContentBlock::ToolResult {
                                        id: id.clone(),
                                        content: output.content.clone(),
                                        is_error: output.is_error,
                                    }],
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
                let output = tokio::select! {
                    out = tokio::time::timeout(
                        self.tool_timeout,
                        self.executor.execute(&ctx, name, effective_args),
                    ) => match out {
                        Ok(output) => output,
                        Err(_) => ToolOutput::error(format!(
                            "Tool '{name}' timed out after {}s",
                            self.tool_timeout.as_secs()
                        )),
                    },
                    _ = ctx.cancel.cancelled() => {
                        answer_pending_tools(self, &tool_uses, idx, "cancelled by user");
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
                    content: vec![ContentBlock::ToolResult {
                        id: id.clone(),
                        content: output.content,
                        is_error: output.is_error,
                    }],
                });
            }

            if stall_rounds == STALL_NUDGE_ROUNDS {
                log::warn!(target: "gray_agent", "exploration stall: injecting nudge after {stall_rounds} read-only rounds");
                self.messages.push(Message::user(format!(
                    "[gray stall guard: {stall_rounds} consecutive exploration tool rounds with no file changes. \
                     Stop re-reading — either make the edit now, or report your findings and stop exploring.]"
                )));
            }
        }
    }
}
