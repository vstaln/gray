use futures::StreamExt as _;

use crate::agent::{
    Agent, InputSubmit, ProviderError, ToolBefore, ToolContext, ToolOutput, salvage_partial_text,
    thinking_block,
};
use crate::agent_compact::{needs_pre_turn_compact, needs_soft_elide};
use crate::agent_tools::{PendingToolCall, answer_pending_tools};
use crate::error::CoreError;
use crate::event::{
    AgentEvent, CompactReason, StopReason, StreamEvent, Usage, append_thinking_chunk,
};
use crate::message::{ChatRequest, ContentBlock, Message, Role, ThoughtSignature};

const MAX_EMPTY_RETRIES: u8 = 1;

const MAX_TURN_RETRIES: u8 = 8;

const MAX_STREAM_STALL_RETRIES: u8 = 2;

fn silent_stall(e: &ProviderError) -> bool {
    matches!(e.class(), ProviderError::Stream(m) if m.contains("stream stalled"))
}

const MAX_TURN_RETRY_WAIT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

#[cfg(not(test))]
fn turn_retry_delay() -> std::time::Duration {
    std::env::var("GRAY_TURN_RETRY_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(
            std::time::Duration::from_secs(3),
            std::time::Duration::from_millis,
        )
}
#[cfg(test)]
fn turn_retry_delay() -> std::time::Duration {
    std::time::Duration::ZERO
}

// Wall-clock ceiling for one stretch of consecutive retrying: provider-side
// reconnect attempts plus whole-turn retries share this budget. Quota/reset
// hints ("usage limit reached, resets in ~4m") keep their own long waits and
// are exempt — the clock is cleared while a hinted wait is in play.
#[cfg(not(test))]
fn retry_budget() -> std::time::Duration {
    std::env::var("GRAY_RETRY_BUDGET_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map_or(
            std::time::Duration::from_secs(60),
            std::time::Duration::from_millis,
        )
}
#[cfg(test)]
fn retry_budget() -> std::time::Duration {
    std::time::Duration::from_secs(3600)
}

fn retry_give_up(detail: &str, retries: u8, waited: Option<std::time::Duration>) -> CoreError {
    let span = waited
        .map(|d| format!(" over ~{}s", d.as_secs()))
        .unwrap_or_default();
    CoreError::Provider(format!(
        "still failing after {retries} retries{span}: {detail} — try again, check the endpoint, or /models to switch provider"
    ))
}

const QUOTA_WINDOW_HINTS: [&str; 6] = [
    "quota",
    "usage limit",
    "session limit",
    "resets in ~",
    "insufficient balance",
    "insufficient credits",
];

fn quota_flavored(e: &ProviderError) -> bool {
    let ProviderError::Auth(msg) = e.class() else {
        return false;
    };
    if e.detail().is_some_and(|d| d.status == Some(429)) {
        return true;
    }
    let lower = msg.to_lowercase();
    QUOTA_WINDOW_HINTS.iter().any(|h| lower.contains(h))
}

const ROUTING_FLAVOR_HINTS: [&str; 5] = [
    "not a chat model",
    "does not exist",
    "no such model",
    "model not found",
    "unsupported model",
];

fn routing_flavored(e: &ProviderError) -> bool {
    let ProviderError::BadRequest(msg) = e.class() else {
        return false;
    };
    let lower = msg.to_lowercase();
    ROUTING_FLAVOR_HINTS.iter().any(|h| lower.contains(h))
}

fn reset_hint_from_text(msg: &str) -> Option<std::time::Duration> {
    let lower = msg.to_lowercase();
    let rest = &lower[lower.find("resets in ~")? + "resets in ~".len()..];
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let n: u64 = rest.get(..digits)?.parse().ok()?;
    Some(match rest.as_bytes().get(digits) {
        Some(b'h') => std::time::Duration::from_secs(n * 3600),
        Some(b's') => std::time::Duration::from_secs(n),
        _ => std::time::Duration::from_secs(n * 60),
    })
}

fn provider_reset_hint(e: &ProviderError) -> Option<std::time::Duration> {
    if let Some(d) = e.detail() {
        if let Some(hint) = d.retry_after() {
            return Some(hint);
        }
        if let Some(hint) = d.vendor_message.as_deref().and_then(reset_hint_from_text) {
            return Some(hint);
        }
    }
    reset_hint_from_text(&e.to_string())
}

fn turn_retry_detail(e: &ProviderError, attempt: u8, already_shown: bool) -> String {
    // The provider's own reconnect notice already printed this error; don't
    // repeat the same body on the whole-turn retry line.
    let what = if already_shown {
        "same error".to_string()
    } else {
        e.to_string()
    };
    let mut detail = format!("{what} · retry {attempt}/{MAX_TURN_RETRIES}");
    if attempt >= 2 && silent_stall(e) {
        detail.push_str(
            " — provider is holding the request but sending nothing; try /model to another route",
        );
    }
    detail.chars().take(300).collect()
}

fn turn_retry_wait(e: &ProviderError, attempt: u8) -> Option<std::time::Duration> {
    if attempt >= MAX_TURN_RETRIES {
        return None;
    }
    if silent_stall(e) && attempt >= MAX_STREAM_STALL_RETRIES {
        return None;
    }
    let hint = provider_reset_hint(e);
    if hint.is_some_and(|h| h > MAX_TURN_RETRY_WAIT) {
        return None;
    }
    if !e.retryable() && !quota_flavored(e) && !routing_flavored(e) {
        return None;
    }
    let backoff = turn_retry_delay().saturating_mul(1u32 << attempt.min(10));
    Some(hint.map_or(backoff, |h| h.max(backoff)))
}

pub(crate) const CONTAMINATED_SCRUB_MARKER: &str =
    "(previous attempt was cut off by a stream error; partial output omitted — start fresh)";

const MASK_STALE_ROUNDS: usize = 10;

impl Agent {
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

    pub fn mask_stale_tool_output(&mut self) {
        let boundary = self.stale_boundary();
        if boundary > self.masked_prefix {
            self.prefix_guard.expect_change("stale output mask");
        }
        self.masked_prefix = self.masked_prefix.max(boundary);
    }
}

fn is_blank_input(msg: &Message) -> bool {
    msg.content.iter().all(|b| match b {
        ContentBlock::Text { text } => text.trim().is_empty(),
        _ => false,
    })
}

impl Agent {
    fn collect_background_notifications(&mut self, ctx: &ToolContext) -> bool {
        let notices = self.executor.drain_notifications(ctx);
        let any = !notices.is_empty();
        for notice in notices {
            self.messages.push(Message::user_injected(format!(
                "[Background task notification]\n{notice}"
            )));
        }
        any
    }

    fn surface_running_jobs(&mut self, ctx: &ToolContext) {
        if let Some(note) = self.executor.running_jobs_note(ctx) {
            self.messages.push(Message::user_injected(format!(
                "[Background jobs still running]\n{note}\nEnd your turn to be woken when one finishes; `tail` a log to peek."
            )));
        }
    }

    async fn emit_turn_end(&self, usage: &Usage) {
        if let Some(warm) = &self.warm_run {
            warm.settle();
        }
        for hook in &self.hooks {
            hook.turn_end(usage).await;
        }
    }

    async fn preflight(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<serde_json::Value, ToolOutput> {
        if name == crate::tool_deferral::LOAD_TOOLS && self.tool_deferral.is_some() {
            return Err(crate::tool_deferral::run_loader(&self.tools, args));
        }
        if !self.tools.iter().any(|t| t.name == name) {
            let names: Vec<String> = self.tools.iter().map(|t| t.name.clone()).collect();
            return Err(ToolOutput::error(crate::tool_out::unknown_tool(
                name, &names,
            )));
        }
        if !args.is_object() {
            let got = match args {
                serde_json::Value::Null => {
                    "nothing (empty arguments — output may have been truncated)".to_string()
                }
                serde_json::Value::String(s) => {
                    let head: String = s.chars().take(200).collect();
                    format!("unparseable text: {head:?}")
                }
                other => other.to_string(),
            };
            return Err(ToolOutput::error(format!(
                "Invalid arguments for tool '{name}': expected a JSON object, got {got}. Please provide a valid JSON object."
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

    pub async fn run(
        &mut self,
        input: Message,
        ctx: ToolContext,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        if is_blank_input(&input) {
            return Ok(vec![]);
        }
        let Some(input) = self.apply_input_hooks(input).await else {
            return Ok(vec![]);
        };
        self.messages.push(input);
        self.run_inner(ctx, None).await
    }

    pub async fn run_streaming(
        &mut self,
        input: Message,
        ctx: ToolContext,
        on_event: &mut dyn FnMut(&AgentEvent),
    ) -> Result<Vec<AgentEvent>, CoreError> {
        if is_blank_input(&input) {
            return Ok(vec![]);
        }
        let Some(input) = self.apply_input_hooks(input).await else {
            return Ok(vec![]);
        };
        self.messages.push(input);
        self.run_inner(ctx, Some(on_event)).await
    }

    fn bill_tool_usage(billed: &mut Usage, name: &str, output: &ToolOutput, session: Option<&str>) {
        if let Some(u) = output.usage.as_deref().filter(|u| u.any()) {
            billed.accumulate(u);
            u.log_request(&format!("tool:{name}"), session);
        }
    }

    async fn apply_tool_after(&self, name: &str, output: &ToolOutput) -> ToolOutput {
        let mut output = output.clone();
        for hook in &self.hooks {
            if let Some(replacement) = hook.tool_after(name, &output).await {
                output = replacement;
            }
        }
        output
    }

    async fn apply_input_hooks(&self, input: Message) -> Option<Message> {
        if input.role != Role::User
            || !input
                .content
                .iter()
                .all(|b| matches!(b, ContentBlock::Text { .. }))
        {
            return Some(input);
        }
        let mut text = input.text_content();
        for hook in &self.hooks {
            match hook.input_submit(&text).await {
                InputSubmit::Pass => {}
                InputSubmit::Handled => return None,
                InputSubmit::Rewrite(t) => text = t,
            }
        }

        if text.trim().is_empty() {
            return None;
        }
        Some(Message::user(text))
    }

    async fn run_inner(
        &mut self,
        ctx: ToolContext,
        mut sink: Option<&mut dyn FnMut(&AgentEvent)>,
    ) -> Result<Vec<AgentEvent>, CoreError> {
        self.session_id = ctx.session_id.clone();

        self.refresh_tools();
        log::info!(target: "gray_agent", "agent run start ({} messages)", self.messages.len());
        let mut events = Vec::new();

        let mut last_sig: Option<String> = None;
        let mut repeat: usize = 0;

        let mut poll_rounds: usize = 0;

        let mut jobs_note_due = true;

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

        let mut billed = Usage::default();
        let mut empty_retries: u8 = 0;

        let mut turn_retries: u8 = 0;

        // When consecutive retrying began; cleared as soon as the stream makes
        // real progress again. Bounds total retry time (retry_budget()).
        let mut retry_clock: Option<tokio::time::Instant> = None;
        let mut last_retry_err: Option<String> = None;

        let mut overflow_recovery_attempted = false;

        let bench_nudges = self.bench_nudges;
        let bench_thrash_marks = self.bench_thrash_marks.clone();
        let mut bench_request_num: u32 = 0;
        let mut bench_thrash_idx: usize = 0;
        let mut bench_finish_gate_sent = false;
        let bench_start = std::time::Instant::now();

        let mut hook_context = String::new();
        if let Some(hook) = self.checkpoint.clone() {
            hook(&self.messages, self.history_revision()).await;
        }
        for hook in &self.hooks {
            if self.lean_prompt && !hook.is_global() {
                continue;
            }
            if let Some(text) = hook.prompt_context().await
                && !text.trim().is_empty()
            {
                if !hook_context.is_empty() {
                    hook_context.push_str("\n\n");
                }
                hook_context.push_str(&text);
            }
        }

        if !hook_context.is_empty() {
            let note = Message::user_injected(format!("[Context update]\n{hook_context}"));
            if !self.messages.contains(&note) {
                self.messages.push(note);
            }
        }

        if !self.lean_prompt {
            for hook in &self.hooks {
                if let Some(text) = hook.agent_before_start().await {
                    self.messages.push(Message::user_injected(text));
                }
            }
        }

        self.turn_system = Some(self.system.clone());

        'turn: loop {
            if ctx.cancel.is_cancelled() {
                self.emit_turn_end(&billed).await;
                return Err(CoreError::Cancelled);
            }

            self.collect_background_notifications(&ctx);
            if std::mem::take(&mut jobs_note_due) {
                self.surface_running_jobs(&ctx);
            }

            if let Some(steer) = self.steer.clone()
                && let Some(text) = steer()
            {
                emit!(AgentEvent::Steered { text: text.clone() });
                self.messages.push(Message::user(text));
            }

            if let Some(hook) = self.checkpoint.clone() {
                hook(&self.messages, self.history_revision()).await;
            }

            // A caching provider re-bills the whole prefix after a rewrite
            // (an 86k write to stub ~10k tokens); mask_stale_tool_output on a
            // cold cache and pre-turn compaction cover it instead.
            if self.prefix_rewrite_ok
                && !self.warm_cache_reported
                && needs_soft_elide(self.estimate_tokens(), self.context_window)
            {
                let before = self.estimate_tokens();
                let elided = self.soft_elide();
                if elided > 0 {
                    log::info!(
                        target: "gray_agent",
                        "soft elision: {elided} tool results stubbed, {before} -> {} tokens",
                        self.estimate_tokens()
                    );
                }
            }

            let media_elided = self.elide_media_over_budget();
            if media_elided > 0 {
                log::info!(
                    target: "gray_agent",
                    "media payload budget: {media_elided} older images stubbed"
                );
            }

            if needs_pre_turn_compact(
                self.estimate_tokens(),
                self.context_window,
                self.compact_reserve,
                self.compact_ceiling,
            ) {
                while needs_pre_turn_compact(
                    self.estimate_tokens(),
                    self.context_window,
                    self.compact_reserve,
                    self.compact_ceiling,
                ) {
                    let before = (self.estimate_tokens(), self.messages.len());
                    match self.try_compact_budgeted().await {
                        Ok(true) => {
                            let reason = self.ceiling_reason(before.0);
                            emit!(AgentEvent::compacted(
                                before.0,
                                self.estimate_tokens(),
                                before.1,
                                self.messages.len(),
                                reason
                            ));
                            jobs_note_due = true;
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

            if std::mem::take(&mut jobs_note_due) {
                self.surface_running_jobs(&ctx);
            }

            let mut request_messages = self.scrubbed_messages();
            if request_messages
                .last()
                .is_some_and(|message| message.role == Role::Assistant)
            {
                request_messages.push(Message::user_injected(
                    "Continue the pending user request using the retained context and summary.",
                ));
            }

            for hook in &self.hooks {
                if let Some(msgs) = hook.context_build(&request_messages).await {
                    request_messages = msgs;
                }
            }

            if bench_nudges {
                bench_request_num += 1;
                let mut note = format!(
                    "[harness] request {bench_request_num} · {}m elapsed · {}k tokens used",
                    bench_start.elapsed().as_secs() / 60,
                    billed.total_tokens / 1000,
                );
                if bench_thrash_idx < bench_thrash_marks.len()
                    && bench_request_num >= bench_thrash_marks[bench_thrash_idx]
                {
                    bench_thrash_idx += 1;
                    note.push_str(
                        " — many requests used; if a self-check keeps failing, change approach or keep the best-working state and finish",
                    );
                }
                request_messages.push(Message::user_injected(note));
            }

            let req = ChatRequest {
                system: (!self.system_text().is_empty()).then(|| self.system_text().to_string()),
                messages: request_messages,
                tools: self.request_tools(),
                max_tokens: None,
            };
            let prev_len = self.prefix_guard.last().map_or(0, |fp| fp.message_count());
            if let Some((change, announced)) =
                self.prefix_guard.observe(self.provider.model_id(), &req)
                && let Some(cause) = crate::prefix_guard::describe(change, announced, prev_len)
            {
                log::debug!(target: "gray_agent", "prefix changed: {change} ({cause})");
                emit!(AgentEvent::PrefixChanged { cause });
            }
            if let Some(sink) = &self.prefix_fp_sink
                && let Some(fp) = self.prefix_guard.last()
            {
                sink(fp);
            }

            let request_sent = tokio::time::Instant::now();

            if let Ok(mut hint) = self.warm_hint.lock() {
                hint.prompt_tokens = self.estimate_tokens();
                hint.cache_reported = self.warm_cache_reported;
            }
            let warm_off = self
                .warm_hint
                .lock()
                .map(|hint| hint.warm_off)
                .unwrap_or(false);
            if let Some(prev) = self.warm_run.take()
                && let Ok(spent) = prev.spent.lock()
            {
                billed.accumulate(&spent);
            }
            if self.cache_warm.is_some() && !warm_off {
                let warm_spent = std::sync::Arc::new(std::sync::Mutex::new(Usage::default()));
                let phase = crate::cache_warm::warm_phase();
                self.warm_run = Some(crate::cache_warm::ActiveWarm {
                    guard: crate::cache_warm::WarmGuard(tokio::spawn(
                        crate::cache_warm::keep_warm(
                            self.provider.clone(),
                            req.clone(),
                            self.cache_warm.clone().unwrap(),
                            self.warm_hint.clone(),
                            request_sent,
                            warm_spent.clone(),
                            phase.clone(),
                        ),
                    )),
                    phase,
                    spent: warm_spent,
                });
            }

            let mut text_parts: Vec<String> = Vec::new();
            let mut thinking_text = String::new();

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
                        _ = async {
                            match retry_clock {
                                Some(start) => {
                                    tokio::time::sleep_until(start + retry_budget()).await
                                }
                                None => std::future::pending::<()>().await,
                            }
                        } => {
                            if !text_parts.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_text.clone(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );
                                self.contaminated.insert(self.messages.len() - 1);
                            }
                            self.emit_turn_end(&billed).await;
                            return Err(retry_give_up(
                                last_retry_err.as_deref().unwrap_or("provider unreachable"),
                                turn_retries,
                                Some(retry_budget()),
                            ));
                        }
                    };

                    // Real progress resets the retry clock; only consecutive
                    // retrying counts against the budget.
                    if matches!(
                        next_event,
                        Some(Ok(StreamEvent::TextDelta { .. }
                            | StreamEvent::ThinkingDelta { .. }
                            | StreamEvent::ToolCallDelta { .. }
                            | StreamEvent::ReasoningItem { .. }
                            | StreamEvent::MessageComplete { .. }))
                    ) {
                        retry_clock = None;
                    }

                    const MAX_EVENTS: usize = 500_000;
                    if events.len() >= MAX_EVENTS {
                        self.emit_turn_end(&billed).await;
                        return Err(CoreError::Provider("turn event limit exceeded".into()));
                    }
                    match next_event {
                        None => {
                            let e = ProviderError::Stream(
                                "provider stream ended without completion".into(),
                            );
                            if !ctx.cancel.is_cancelled()
                                && let Some(delay) = turn_retry_wait(&e, turn_retries)
                            {
                                let reset_wait = provider_reset_hint(&e).is_some();
                                if reset_wait {
                                    // Reset-hint waits (quota windows) keep
                                    // their own long budget; the consecutive-
                                    // retry clock stays disarmed.
                                    retry_clock = None;
                                }
                                let waited = if reset_wait {
                                    std::time::Duration::ZERO
                                } else {
                                    retry_clock
                                        .get_or_insert_with(tokio::time::Instant::now)
                                        .elapsed()
                                };
                                let remaining = retry_budget().saturating_sub(waited);
                                if !reset_wait && remaining.is_zero() {
                                    if !text_parts.is_empty() {
                                        salvage_partial_text(
                                            &mut self.messages,
                                            thinking_text.clone(),
                                            text_parts.concat(),
                                            &pending_reasoning,
                                            self.provider.model_id(),
                                        );
                                        self.contaminated.insert(self.messages.len() - 1);
                                    }
                                    self.emit_turn_end(&billed).await;
                                    return Err(retry_give_up(
                                        &e.to_string(),
                                        turn_retries,
                                        Some(waited),
                                    ));
                                }
                                turn_retries += 1;
                                last_retry_err = Some(e.to_string());
                                let wait = if reset_wait {
                                    delay
                                } else {
                                    delay.min(remaining)
                                };
                                log::warn!(
                                    target: "gray_agent",
                                    "provider stream dropped; whole-turn retry {turn_retries}/{MAX_TURN_RETRIES} in {wait:?}: {e}"
                                );
                                let already_shown = matches!(
                                    events.last(),
                                    Some(AgentEvent::StreamError { details, .. })
                                        if *details == e.to_string().chars().take(200).collect::<String>()
                                );
                                emit!(AgentEvent::stream_error(
                                    format!("Retrying turn... {turn_retries}/{MAX_TURN_RETRIES}"),
                                    turn_retry_detail(&e, turn_retries, already_shown)
                                ));
                                if !wait.is_zero() {
                                    tokio::select! {
                                        _ = tokio::time::sleep(wait) => {}
                                        _ = ctx.cancel.cancelled() => {}
                                    }
                                }
                                if ctx.cancel.is_cancelled() {
                                    self.emit_turn_end(&billed).await;
                                    return Err(CoreError::Cancelled);
                                }
                                continue 'turn;
                            }
                            if !text_parts.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_text.clone(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );

                                self.contaminated.insert(self.messages.len() - 1);
                            }
                            self.emit_turn_end(&billed).await;
                            return Err(if turn_retries >= MAX_TURN_RETRIES {
                                retry_give_up(&e.to_string(), turn_retries, None)
                            } else {
                                CoreError::from(e)
                            });
                        }
                        Some(Ok(StreamEvent::TextDelta { delta })) => {
                            if !delta.is_empty() {
                                emit!(AgentEvent::text_delta(delta.clone()));
                                text_parts.push(delta);
                            }
                        }
                        Some(Ok(StreamEvent::ThinkingDelta { delta })) => {
                            if !delta.is_empty() {
                                emit!(AgentEvent::thinking_delta(delta.clone()));
                                append_thinking_chunk(&mut thinking_text, &delta);
                            }
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
                            thought_signature,
                        })) => {
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

                            if let Some(signature) = thought_signature {
                                if slot
                                    .thought_signature
                                    .as_ref()
                                    .is_some_and(|s| s != &signature)
                                {
                                    self.emit_turn_end(&billed).await;
                                    return Err(CoreError::Provider(format!(
                                        "provider changed thought signature for tool-call index {index}"
                                    )));
                                }
                                slot.thought_signature = Some(signature);
                            }

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

                            if !slot.started
                                && slot.id.as_ref().is_some_and(|s| !s.trim().is_empty())
                                && slot.name.as_ref().is_some_and(|s| !s.trim().is_empty())
                            {
                                let live_id = slot.id.clone().unwrap();
                                let live_name = slot.name.clone().unwrap();
                                emit!(AgentEvent::tool_call_start(live_id, live_name));
                                slot.started = true;
                            }

                            if slot.started && !arguments_delta.is_empty() {
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

                        Some(Ok(StreamEvent::StreamError { message, details })) => {
                            retry_clock.get_or_insert_with(tokio::time::Instant::now);
                            last_retry_err = Some(if details.is_empty() {
                                message.clone()
                            } else {
                                details.clone()
                            });
                            emit!(AgentEvent::stream_error(message.clone(), details.clone()));
                        }
                        Some(Err(e)) => {
                            if !ctx.cancel.is_cancelled()
                                && let Some(delay) = turn_retry_wait(&e, turn_retries)
                            {
                                let reset_wait = provider_reset_hint(&e).is_some();
                                if reset_wait {
                                    // Reset-hint waits (quota windows) keep
                                    // their own long budget; the consecutive-
                                    // retry clock stays disarmed.
                                    retry_clock = None;
                                }
                                let waited = if reset_wait {
                                    std::time::Duration::ZERO
                                } else {
                                    retry_clock
                                        .get_or_insert_with(tokio::time::Instant::now)
                                        .elapsed()
                                };
                                let remaining = retry_budget().saturating_sub(waited);
                                if !reset_wait && remaining.is_zero() {
                                    if !text_parts.is_empty() {
                                        salvage_partial_text(
                                            &mut self.messages,
                                            thinking_text.clone(),
                                            text_parts.concat(),
                                            &pending_reasoning,
                                            self.provider.model_id(),
                                        );
                                        self.contaminated.insert(self.messages.len() - 1);
                                    }
                                    self.emit_turn_end(&billed).await;
                                    return Err(retry_give_up(
                                        &e.to_string(),
                                        turn_retries,
                                        Some(waited),
                                    ));
                                }
                                turn_retries += 1;
                                last_retry_err = Some(e.to_string());
                                let wait = if reset_wait {
                                    delay
                                } else {
                                    delay.min(remaining)
                                };
                                log::warn!(
                                    target: "gray_agent",
                                    "turn failed retryably; whole-turn retry {turn_retries}/{MAX_TURN_RETRIES} in {wait:?}: {e}"
                                );
                                let already_shown = matches!(
                                    events.last(),
                                    Some(AgentEvent::StreamError { details, .. })
                                        if *details == e.to_string().chars().take(200).collect::<String>()
                                );
                                emit!(AgentEvent::stream_error(
                                    format!("Retrying turn... {turn_retries}/{MAX_TURN_RETRIES}"),
                                    turn_retry_detail(&e, turn_retries, already_shown)
                                ));
                                if !wait.is_zero() {
                                    tokio::select! {
                                        _ = tokio::time::sleep(wait) => {}
                                        _ = ctx.cancel.cancelled() => {}
                                    }
                                }
                                if ctx.cancel.is_cancelled() {
                                    self.emit_turn_end(&billed).await;
                                    return Err(CoreError::Cancelled);
                                }
                                continue 'turn;
                            }

                            if !text_parts.is_empty() {
                                salvage_partial_text(
                                    &mut self.messages,
                                    thinking_text.clone(),
                                    text_parts.concat(),
                                    &pending_reasoning,
                                    self.provider.model_id(),
                                );

                                self.contaminated.insert(self.messages.len() - 1);
                            }

                            if e.should_compress() {
                                let before = (self.estimate_tokens(), self.messages.len());
                                match self.try_compact_budgeted().await {
                                    Ok(true) => {
                                        emit!(AgentEvent::compacted(
                                            before.0,
                                            self.estimate_tokens(),
                                            before.1,
                                            self.messages.len(),
                                            CompactReason::Overflow
                                        ));
                                        jobs_note_due = true;
                                        continue 'turn;
                                    }
                                    _ => {
                                        let err = if turn_retries >= MAX_TURN_RETRIES {
                                            retry_give_up(&e.to_string(), turn_retries, None)
                                        } else {
                                            CoreError::from(e)
                                        };
                                        self.emit_turn_end(&billed).await;
                                        return Err(err);
                                    }
                                }
                            }
                            let err = if turn_retries >= MAX_TURN_RETRIES {
                                retry_give_up(&e.to_string(), turn_retries, None)
                            } else {
                                CoreError::from(e)
                            };
                            self.emit_turn_end(&billed).await;
                            return Err(err);
                        }
                    }
                }
            };

            turn_retries = 0;
            retry_clock = None;
            last_retry_err = None;
            if usage.any() {
                total_usage = usage;
                total_usage.normalize();
            }
            billed.accumulate(&usage);
            usage.log_request("turn", self.session_id.as_deref());
            let prices = self.cache_warm.as_ref().and_then(|p| (p.prices)());
            self.cache_stats.observe(
                &usage,
                self.provider.model_id(),
                self.session_id.as_deref(),
                prices,
            );

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

            let mut content: Vec<ContentBlock> = Vec::new();
            let thinking = std::mem::take(&mut thinking_text);

            if !thinking.is_empty() || pending_reasoning.is_some() {
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

                let Some(id) = call.id.clone().filter(|s| !s.trim().is_empty()) else {
                    log::warn!(target: "gray_agent", "dropping tool call index {index} with empty id after fallback");
                    continue;
                };
                content.push(ContentBlock::ToolUse {
                    id,
                    name,
                    args: call.parsed_args(),
                    thought_signature: call.thought_signature.clone().map(|signature| {
                        ThoughtSignature {
                            signature,
                            model: self.provider.model_id().to_string(),
                        }
                    }),
                });
            }
            let assistant = Message {
                role: Role::Assistant,
                injected: false,
                content,
            };
            self.messages.push(assistant.clone());

            self.record_context_usage(&usage);
            self.warm_cache_reported = self.warm_cache_reported
                || usage.cache_read_input_tokens
                    + usage.cached_tokens
                    + usage.cache_write_input_tokens
                    > 0;
            if let (true, Ok(mut hint)) = (usage.input_tokens > 0, self.warm_hint.lock()) {
                hint.prompt_tokens = usage.input_tokens;
                hint.cache_reported = self.warm_cache_reported;
            }

            let overflow_retry =
                stop_reason == StopReason::MaxTokens && !overflow_recovery_attempted;
            if self
                .context_window
                .is_some_and(|w| crate::overflow::is_silent_overflow(&usage, stop_reason, w))
                && (overflow_retry || stop_reason == StopReason::EndTurn)
            {
                if overflow_retry
                    && self
                        .messages
                        .last()
                        .is_some_and(|m| m.role == Role::Assistant)
                {
                    self.messages.pop();
                }
                let before = (self.estimate_tokens(), self.messages.len());
                if let Ok(true) = self.try_compact_budgeted().await {
                    emit!(AgentEvent::compacted(
                        before.0,
                        self.estimate_tokens(),
                        before.1,
                        self.messages.len(),
                        CompactReason::Overflow
                    ));
                    jobs_note_due = true;
                    if overflow_retry {
                        overflow_recovery_attempted = true;
                        continue 'turn;
                    }
                }
            }

            let tool_uses: Vec<(String, String, serde_json::Value)> = assistant
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse { id, name, args, .. } => {
                        Some((id.clone(), name.clone(), args.clone()))
                    }
                    _ => None,
                })
                .collect();

            if stop_reason == StopReason::MaxTokens && !tool_uses.is_empty() {
                for (id, name, args) in &tool_uses {
                    emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));
                    let content = format!(
                        "Tool '{name}' was not executed: the response hit the output token limit, so its arguments may be truncated. Re-issue the tool call with complete arguments."
                    );
                    emit!(AgentEvent::tool_result(id.clone(), content.clone(), true));
                    self.messages.push(Message {
                        role: Role::User,
                        injected: false,
                        content: vec![ContentBlock::ToolResult {
                            id: id.clone(),
                            content,
                            is_error: true,
                        }],
                    });
                }
                emit!(AgentEvent::MessageBoundary);
                continue 'turn;
            }

            if text_is_empty && tool_uses.is_empty() && stop_reason != StopReason::MaxTokens {
                self.messages.pop();
                if empty_retries < MAX_EMPTY_RETRIES {
                    empty_retries += 1;
                    continue 'turn;
                }
                self.messages.push(Message::assistant("(empty)"));
            } else if !text_is_empty || !tool_uses.is_empty() {
                empty_retries = 0;
            }

            if tool_uses.is_empty() {
                if self.collect_background_notifications(&ctx) {
                    emit!(AgentEvent::MessageBoundary);
                    continue 'turn;
                }

                if bench_nudges
                    && !bench_finish_gate_sent
                    && self
                        .messages
                        .iter()
                        .flat_map(|m| m.content.iter())
                        .any(|b| matches!(b, ContentBlock::ToolResult { .. }))
                {
                    bench_finish_gate_sent = true;
                    log::info!(target: "gray_agent", "bench finish gate: requesting pre-exit verification");
                    self.messages.push(Message::user_injected(
                        "Before finishing, verify the deliverable: re-read the task spec; for every file path it names, confirm the file exists and contains a real implementation, not a stub; if the task ships tests or examples, run them against the deliverable. If anything is missing or failing, keep working. Otherwise finish.",
                    ));
                    emit!(AgentEvent::MessageBoundary);
                    continue 'turn;
                }
                let hit = if total_usage.input_tokens > 0 {
                    total_usage.cached_tokens as f64 / total_usage.input_tokens as f64 * 100.0
                } else {
                    0.0
                };

                if let Some(warm) = &self.warm_run
                    && let Ok(spent) = warm.spent.lock()
                {
                    billed.accumulate(&spent);
                }
                log::info!(target: "gray_agent", "agent run end: session={} stop={stop_reason:?}, usage in={} out={} cached={} hit={:.0}%, {} messages", ctx.session_id.as_deref().unwrap_or("-"), total_usage.input_tokens, total_usage.output_tokens, total_usage.cached_tokens, hit, self.messages.len());
                emit!(AgentEvent::turn_end(stop_reason, billed));
                self.emit_turn_end(&billed).await;
                return Ok(events);
            }

            const SIGNATURE_ABORT_REPEATS: usize = 12;
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

            let known: std::collections::HashSet<String> =
                self.tools.iter().map(|t| t.name.clone()).collect();

            let round_start = self.messages.len();
            let segments = crate::parallel::plan_segments(&tool_uses, &known);
            for segment in segments {
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

                    let demoted = {
                        let calls: Vec<(&str, &serde_json::Value)> =
                            ready.iter().map(|(_, n, a)| (n.as_str(), a)).collect();
                        crate::parallel::rewrite_conflicts(&calls)
                    };
                    if !demoted.is_empty() {
                        let mut keep = Vec::with_capacity(ready.len());
                        for (pos, item) in ready.drain(..).enumerate() {
                            if demoted.contains(&pos) {
                                inline_errors.insert(
                                    item.0,
                                    ToolOutput::error(format!(
                                        "Tool '{}' was rewritten by a hook into a form that cannot run in parallel; not executed.",
                                        item.1
                                    )),
                                );
                            } else {
                                keep.push(item);
                            }
                        }
                        ready = keep;
                    }

                    for (idx, _, _) in &ready {
                        let (id, _, args) = &tool_uses[*idx];
                        emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));
                    }

                    let timeout = self.tool_timeout;
                    let mut futs = Vec::with_capacity(ready.len());
                    for (idx, name, effective_args) in ready {
                        let ex = self.executor.clone();
                        let c = ctx.clone();
                        let sink = self.tool_output_sink(&tool_uses[idx].0);
                        futs.push((
                            idx,
                            Box::pin(async move {
                                let exec = ex.execute(&c, &name, effective_args);
                                let out = match sink {
                                    Some(sink) => {
                                        tokio::time::timeout(
                                            timeout,
                                            crate::agent::scope_tool_output(sink, exec),
                                        )
                                        .await
                                    }
                                    None => tokio::time::timeout(timeout, exec).await,
                                };
                                match out {
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

                    let by_idx: std::collections::HashMap<usize, Option<ToolOutput>> =
                        joined.into_iter().collect();
                    let mut shortfall = false;
                    for &idx in &idxs {
                        let (id, name, args) = &tool_uses[idx];
                        if let Some(err) = inline_errors.remove(&idx) {
                            emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));
                            emit!(AgentEvent::tool_result(
                                id.clone(),
                                err.content.clone(),
                                err.is_error
                            ));
                            self.messages.push(Message {
                                role: Role::User,
                                injected: false,
                                content: vec![ContentBlock::ToolResult {
                                    id: id.clone(),
                                    content: err.content,
                                    is_error: err.is_error,
                                }],
                            });
                            continue;
                        }
                        match by_idx.get(&idx) {
                            Some(Some(output)) => {
                                let output = self.apply_tool_after(name, output).await;
                                Self::bill_tool_usage(
                                    &mut billed,
                                    name,
                                    &output,
                                    ctx.session_id.as_deref(),
                                );
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
                                    injected: false,
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

                emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));

                let effective_args = match self.preflight(name, args).await {
                    Ok(effective_args) => effective_args,
                    Err(err) => {
                        emit!(AgentEvent::tool_result(
                            id.clone(),
                            err.content.clone(),
                            err.is_error
                        ));
                        self.messages.push(Message {
                            role: Role::User,
                            injected: false,
                            content: vec![ContentBlock::ToolResult {
                                id: id.clone(),
                                content: err.content,
                                is_error: err.is_error,
                            }],
                        });
                        continue;
                    }
                };

                let mut exec_fut: futures::future::BoxFuture<'_, ToolOutput> =
                    match self.tool_output_sink(id) {
                        Some(sink) => Box::pin(crate::agent::scope_tool_output(
                            sink,
                            self.executor.execute(&ctx, name, effective_args),
                        )),
                        None => Box::pin(self.executor.execute(&ctx, name, effective_args)),
                    };
                let output = tokio::select! {
                    out = tokio::time::timeout(self.tool_timeout, &mut exec_fut) => match out {
                        Ok(output) => output,
                        Err(_) => ToolOutput::error(format!(
                            "Tool '{name}' timed out after {}s",
                            self.tool_timeout.as_secs()
                        )),
                    },
                    _ = ctx.cancel.cancelled() => {

                        let report = match tokio::time::timeout(
                            crate::parallel::CANCEL_REPORT_GRACE,
                            &mut exec_fut,
                        )
                        .await
                        {
                            Ok(report) => report,
                            Err(_) => ToolOutput::error("cancelled by user"),
                        };
                        let report = self.apply_tool_after(name, &report).await;
                        Self::bill_tool_usage(&mut billed, name, &report, ctx.session_id.as_deref());
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
                            injected: false,
                            content: vec![ContentBlock::ToolResult {
                                id: id.clone(),
                                content: report.content,
                                is_error: report.is_error,
                            }],
                        });

                        answer_pending_tools(self, &tool_uses, idx + 1, "cancelled by user");
                        self.emit_turn_end(&billed).await;
                        return Err(CoreError::Cancelled);
                    },
                };
                let output = self.apply_tool_after(name, &output).await;
                Self::bill_tool_usage(&mut billed, name, &output, ctx.session_id.as_deref());
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
                    injected: false,
                    content: output.message_blocks(id),
                });
            }

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

            emit!(AgentEvent::MessageBoundary);
        }
    }
}

fn is_job_progress(content: &str) -> bool {
    content.starts_with("still running · job ")
}

#[cfg(test)]
mod turn_retry_tests {
    use super::*;

    fn detailed(e: ProviderError, detail: crate::error::ProviderDetail) -> ProviderError {
        ProviderError::Detailed {
            class: Box::new(e),
            detail: Box::new(detail),
        }
    }

    #[test]
    fn turn_retry_detail_dedupes_already_shown_error() {
        let e = ProviderError::ServerError("status 500: boom".into());
        let fresh = turn_retry_detail(&e, 1, false);
        assert!(fresh.contains("status 500"), "{fresh}");
        let dup = turn_retry_detail(&e, 1, true);
        assert!(dup.contains("same error"), "{dup}");
        assert!(!dup.contains("status 500"), "{dup}");
        assert!(dup.contains("retry 1/"), "{dup}");
    }

    #[test]
    fn retry_give_up_says_what_failed_and_what_to_try() {
        let e = ProviderError::ServerError("status 500: boom".into());
        let err = retry_give_up(&e.to_string(), 4, Some(std::time::Duration::from_secs(62)));
        let msg = err.to_string();
        assert!(msg.contains("status 500"), "{msg}");
        assert!(msg.contains("4 retries"), "{msg}");
        assert!(msg.contains("~62s"), "{msg}");
        assert!(msg.contains("/models"), "{msg}");
        let err = retry_give_up(&e.to_string(), 8, None);
        assert!(err.to_string().contains("8 retries"), "{err}");
    }

    #[test]
    fn rate_limited_retries_with_no_hint() {
        assert!(turn_retry_wait(&ProviderError::RateLimited("429".into()), 0).is_some());
    }

    #[test]
    fn quota_flavored_auth_retries() {
        let e = ProviderError::Auth("status 429: usage limit reached".into());
        assert!(turn_retry_wait(&e, 0).is_some());
        let e = detailed(
            ProviderError::Auth("status 429".into()),
            crate::error::ProviderDetail {
                status: Some(429),
                ..Default::default()
            },
        );
        assert!(turn_retry_wait(&e, 0).is_some());
    }

    #[test]
    fn genuine_auth_failure_stays_terminal() {
        let e = ProviderError::Auth("status 401: invalid api key".into());
        assert!(turn_retry_wait(&e, 0).is_none());
    }

    #[test]
    fn terminal_classes_stay_terminal() {
        for e in [
            ProviderError::BadRequest("bad".into()),
            ProviderError::ContextOverflow("full".into()),
        ] {
            assert!(turn_retry_wait(&e, 0).is_none());
        }
    }

    #[test]
    fn retry_after_detail_sets_the_wait() {
        let e = detailed(
            ProviderError::RateLimited("429".into()),
            crate::error::ProviderDetail {
                retry_after_ms: Some(90_000),
                ..Default::default()
            },
        );
        assert_eq!(
            turn_retry_wait(&e, 0),
            Some(std::time::Duration::from_secs(90))
        );
    }

    #[test]
    fn text_reset_hint_sets_the_wait() {
        let e = ProviderError::Auth("usage limit reached, resets in ~4m".into());
        assert_eq!(
            turn_retry_wait(&e, 0),
            Some(std::time::Duration::from_secs(240))
        );
    }

    #[test]
    fn long_reset_hint_is_terminal() {
        let e = ProviderError::Auth("usage limit reached, resets in ~2h".into());
        assert!(turn_retry_wait(&e, 0).is_none());
        let e = detailed(
            ProviderError::RateLimited("429".into()),
            crate::error::ProviderDetail {
                retry_after_ms: Some(2 * 3600 * 1000),
                ..Default::default()
            },
        );
        assert!(turn_retry_wait(&e, 0).is_none());
    }

    #[test]
    fn routing_flavored_bad_request_retries() {
        let e = ProviderError::BadRequest("status 400: model 'x' is not a chat model".into());
        assert!(turn_retry_wait(&e, 0).is_some());
        assert!(turn_retry_wait(&ProviderError::BadRequest("bad input".into()), 0).is_none());
    }

    #[test]
    fn silent_stall_retry_detail_carries_attempt_and_hint() {
        let e = ProviderError::Stream("stream stalled: no output for 300s".into());
        let first = turn_retry_detail(&e, 1, false);
        assert!(first.contains("retry 1/8"));
        assert!(!first.contains("/model"));
        let second = turn_retry_detail(&e, 2, false);
        assert!(second.contains("retry 2/8"));
        assert!(second.contains("/model"));
        let other = ProviderError::RateLimited("429".into());
        assert!(!turn_retry_detail(&other, 3, false).contains("/model"));
    }

    #[test]
    fn silent_stalls_give_up_before_other_retryable_errors() {
        let stall = ProviderError::Stream("stream stalled: no output for 300s".into());
        assert!(turn_retry_wait(&stall, 0).is_some());
        assert!(turn_retry_wait(&stall, MAX_STREAM_STALL_RETRIES - 1).is_some());
        assert!(turn_retry_wait(&stall, MAX_STREAM_STALL_RETRIES).is_none());
        let other = ProviderError::RateLimited("429".into());
        assert!(turn_retry_wait(&other, MAX_STREAM_STALL_RETRIES + 3).is_some());
    }

    #[test]
    fn retry_cap_is_honored() {
        let e = ProviderError::RateLimited("429".into());
        assert!(turn_retry_wait(&e, MAX_TURN_RETRIES - 1).is_some());
        assert!(turn_retry_wait(&e, MAX_TURN_RETRIES).is_none());
    }
}
