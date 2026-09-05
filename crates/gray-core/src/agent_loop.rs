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
use crate::agent_tools::{PendingToolCall, answer_pending_tools};
use crate::error::CoreError;
use crate::event::{AgentEvent, StopReason, StreamEvent, Usage};
use crate::message::{ChatRequest, ContentBlock, Message, Role};

/// Empty-turn provider retries before nudging or ending on `(empty)`.
const MAX_EMPTY_RETRIES: u8 = 2;
/// Truncated-turn (`MaxTokens`) continuations before keeping the partial.
const MAX_CONTINUATIONS: u8 = 2;

impl Agent {
    /// Runs the agent loop starting from `input`, returning every event
    /// emitted along the way.
    ///
    /// Per turn: build a [`ChatRequest`], stream the response while
    /// forwarding `TextDelta`s in arrival order, finalize the assistant
    /// message, then execute any requested tools sequentially and feed their
    /// outputs back as tool-result messages. Stops when a turn ends without
    /// tool calls (`TurnEnd`) or when cancellation fires
    /// ([`CoreError::Cancelled`]). Two stall guards abort runaway loops:
    /// 3 identical consecutive tool calls, or 20 consecutive read-only
    /// (read/ls/find/grep) rounds with no file changes — nudged at 12.
    /// Tool failures are *not* errors: they become `is_error` tool results
    /// so the model can recover.
    pub async fn run(
        &mut self,
        input: Message,
        ctx: ToolContext,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        self.run_inner(input, ctx, None).await
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
        self.run_inner(input, ctx, Some(on_event)).await
    }

    async fn run_inner(
        &mut self,
        input: Message,
        ctx: ToolContext,
        mut sink: Option<&mut dyn FnMut(&AgentEvent)>,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        log::info!(target: "gray_agent", "agent run start ({} messages)", self.messages.len() + 1);
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
        self.messages.push(input);
        let mut total_usage = Usage::default();
        let mut round: u32 = 0;
        let mut empty_retries: u8 = 0;
        let mut continuations: u8 = 0;
        let mut compact_attempted = false;
        // Post-tool empty nudge fires once per run; silent-retry budget unchanged.
        let mut empty_nudge_sent = false;

        'turn: loop {
            // Cancellation is honored between turns, never mid-stream: a
            // half-finished assistant message would leave the transcript
            // inconsistent for the provider.
            if ctx.cancel.is_cancelled() {
                return Err(CoreError::Cancelled);
            }
            if let Some(m) = self.max_rounds
                && round >= m
            {
                return Err(CoreError::LoopDetected(format!(
                    "max rounds ({m}) exceeded"
                )));
            }
            round += 1;
            self.drain_steer(round == 1);

            // Protocol v1 `prompt/context`: every hook's reply concatenates
            // onto this turn's system prompt, in hook order. No hooks (or no
            // replies) leaves `self.system` untouched.
            let mut system = self.system.clone();
            for hook in &self.hooks {
                if let Some(text) = hook.prompt_context().await
                    && !text.trim().is_empty()
                {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
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
            let mut pending_emitted_start: Vec<bool> = Vec::new();
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
                            return Err(CoreError::Cancelled);
                        }
                    };
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
                                return Err(CoreError::Provider(format!(
                                    "tool-call index {index} exceeds limit ({MAX_TOOL_CALL_INDEX})"
                                )));
                            }
                            while pending.len() <= index {
                                pending.push(PendingToolCall::default());
                                pending_emitted_start.push(false);
                            }
                            let slot = &mut pending[index];
                            let was_unnamed = slot.name.is_none();
                            if slot.id.is_none() {
                                slot.id = id;
                            }
                            if slot.name.is_none() {
                                slot.name = name.clone();
                            }
                            slot.arguments.push_str(&arguments_delta);
                            // Live emit: as soon as we know the tool name, tell the UI
                            // so it can show "Preparing tool: bash…" instead of "Thinking... 53s".
                            if was_unnamed && slot.name.is_some() && !pending_emitted_start[index] {
                                let live_id =
                                    slot.id.clone().unwrap_or_else(|| format!("call_{index}"));
                                let live_name = slot.name.clone().unwrap_or_default();
                                emit!(AgentEvent::tool_call_start(live_id, live_name));
                                pending_emitted_start[index] = true;
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
                            // Context overflow: compact once via complete_prompt,
                            // then retry the turn; otherwise surface the error.
                            if e.should_compress() && !compact_attempted {
                                compact_attempted = true;
                                match self.try_compact_once().await {
                                    Ok(true) => continue 'turn,
                                    _ => return Err(CoreError::from(e)),
                                }
                            }
                            return Err(CoreError::from(e));
                        }
                        None => {
                            // Provider closed without a completion event;
                            // treat as a normal end of turn.
                            break (StopReason::EndTurn, Usage::default());
                        }
                    }
                }
            };
            if usage.input_tokens != 0
                || usage.cached_tokens != 0
                || usage.non_cached_input_tokens != 0
                || usage.cache_read_input_tokens != 0
                || usage.cache_write_input_tokens != 0
                || usage.total_tokens != 0
            {
                total_usage.input_tokens = usage.input_tokens;
                total_usage.cached_tokens = usage.cached_tokens;
                total_usage.non_cached_input_tokens = usage.non_cached_input_tokens;
                total_usage.cache_read_input_tokens = usage.cache_read_input_tokens;
                total_usage.cache_write_input_tokens = usage.cache_write_input_tokens;
            }
            total_usage.output_tokens += usage.output_tokens;
            total_usage.reasoning_tokens += usage.reasoning_tokens;
            total_usage.total_tokens = 0;
            total_usage.normalize();
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
                let name = call.name.clone().unwrap_or_default();
                if name.trim().is_empty() {
                    log::warn!(target: "gray_agent", "dropping tool call index {index} with empty name (args: {})", call.arguments.chars().take(200).collect::<String>());
                    continue;
                }
                content.push(ContentBlock::ToolUse {
                    id: call.id.clone().unwrap_or_else(|| format!("call_{index}")),
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
                emit!(AgentEvent::turn_end(stop_reason, total_usage));
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
            const STALL_ABORT_ROUNDS: usize = 20;
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
                return Err(CoreError::LoopDetected(format!(
                    "exploration stall: {stall_rounds} read-only tool rounds with no file changes"
                )));
            }

            // W3 dispatch validation: unknown tools and malformed args never
            // reach the executor; each still gets one error tool result so the
            // assistant/user alternation stays intact.
            let available: Vec<String> = self.tools.iter().map(|t| t.name.clone()).collect();
            for (idx, (id, name, args)) in tool_uses.iter().enumerate() {
                if ctx.cancel.is_cancelled() {
                    answer_pending_tools(self, &tool_uses, idx, "cancelled by user");
                    return Err(CoreError::Cancelled);
                }
                if !pending_emitted_start.get(idx).copied().unwrap_or(false) {
                    emit!(AgentEvent::tool_call_start(id.clone(), name.clone()));
                }
                emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));

                if !self.tools.iter().any(|t| t.name == *name) {
                    let list = if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    };
                    let err = ToolOutput::error(format!(
                        "Tool '{name}' does not exist. Available: {list}"
                    ));
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
                if !args.is_object() {
                    let err = ToolOutput::error(format!(
                        "Invalid arguments for tool '{name}': expected a JSON object. Please provide a valid JSON object."
                    ));
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

                // Protocol v1 `tool/before`: each hook sees the call in
                // order; a modify rewrites args for later hooks and the
                // executor, the first deny wins and skips the executor.
                let mut effective_args = args.clone();
                let mut denial: Option<String> = None;
                for hook in &self.hooks {
                    match hook.tool_before(name, &effective_args).await {
                        ToolBefore::Allow => {}
                        ToolBefore::Modify(rewritten) => effective_args = rewritten,
                        ToolBefore::Deny(reason) => {
                            denial = Some(reason);
                            break;
                        }
                    }
                }
                if let Some(reason) = denial {
                    let err = ToolOutput::error(reason);
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
                        return Err(CoreError::Cancelled);
                    },
                };

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
