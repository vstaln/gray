//! Shared execution contracts: the seams between gray-core and the
//! provider/tools leaves. The binary wires implementations together.

use std::path::PathBuf;

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::CoreError;
use crate::event::StreamEvent;
use crate::message::ChatRequest;

/// Errors surfaced by a provider implementation.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("rate limited: {0}")]
    RateLimited(String),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("stream broken: {0}")]
    Stream(String),
}

/// Output of a tool execution. Errors are data for the model, not crashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: false }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self { content: content.into(), is_error: true }
    }
}

/// Per-execution context handed to tools.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancel: CancellationToken,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self { cwd: PathBuf::from("."), cancel: CancellationToken::new() }
    }
}

/// A streaming LLM provider (wire protocol behind this seam).
#[async_trait]
pub trait Provider: Send + Sync {
    fn stream(
        &self,
        req: ChatRequest,
    ) -> BoxStream<'static, Result<StreamEvent, ProviderError>>;
}

/// Executes named tools. The registry lives behind this seam so core
/// never knows what tools exist.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput>;
}

/// Convenience alias used by Agent wiring.
pub type ProviderStream = BoxStream<'static, Result<StreamEvent, ProviderError>>;

impl From<ProviderError> for CoreError {
    fn from(e: ProviderError) -> Self {
        CoreError::Provider(e.to_string())
    }
}

use futures::StreamExt as _;

use crate::event::{AgentEvent, StopReason, Usage};
use crate::message::{ContentBlock, Message, Role, ToolDef};

/// The agent loop: drives a conversation against a [`Provider`], executing
/// tool calls through a [`ToolExecutor`] until the model stops requesting
/// tools or cancellation fires.
///
/// There is no fixed turn limit — the loop is model-driven. It terminates
/// when a turn ends without tool calls (`TurnEnd`) or when cancellation
/// fires. A lightweight stall guard (3 identical consecutive tool calls)
/// aborts runaway loops without an arbitrary round cap; provider context
/// errors and user cancellation remain the other natural bounds.
///
/// `run` is in *collecting* form: it buffers all [`AgentEvent`]s and returns
/// them once the run finishes, rather than invoking a callback or yielding
/// through a channel. This keeps the core loop synchronous-in-shape (single
/// value out, single error path), trivially unit-testable, and free of
/// back-pressure concerns; a streaming façade can be layered on top later by
/// draining these events (or by swapping the return type for a receiver).
pub struct Agent {
    provider: Box<dyn Provider>,
    executor: Box<dyn ToolExecutor>,
    system: String,
    tools: Vec<ToolDef>,
    messages: Vec<Message>,
}

impl Agent {
    /// Creates an agent over the given provider and tool executor.
    pub fn new(provider: Box<dyn Provider>, executor: Box<dyn ToolExecutor>) -> Self {
        Self {
            provider,
            executor,
            system: String::new(),
            tools: Vec::new(),
            messages: Vec::new(),
        }
    }

    /// Sets the system prompt sent with every request.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self
    }

    /// Sets the tools advertised to the model.
    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    /// Deprecated: turn limit removed — loop is model-driven with stall
    /// detection. Kept for API compatibility; no effect.
    #[deprecated(note = "max_turns removed; loop is now unbounded with stall detection")]
    pub fn with_max_turns(self, _max_turns: usize) -> Self {
        self
    }

    /// Sets the initial conversation messages (useful for resumed sessions).
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Read-only view of the accumulated conversation so far.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Updates or replaces the accumulated conversation messages (e.g. after compaction).
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// Clears all accumulated conversation messages (e.g. on `/new`).
    pub fn clear_messages(&mut self) {
        self.messages.clear();
    }

    /// Reference to the underlying LLM provider.
    pub fn provider(&self) -> &dyn Provider {
        &*self.provider
    }

    /// Single-turn text completion with optional system prompt (used for compaction & summarization).
    pub async fn complete_prompt(&self, prompt: &str, system: Option<&str>) -> Result<String, CoreError> {
        let req = ChatRequest {
            system: system.map(|s| s.to_string()),
            messages: vec![Message::user(prompt)],
            tools: Vec::new(),
        };
        let mut stream = self.provider.stream(req);
        let mut result = String::new();
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::TextDelta { delta } => result.push_str(&delta),
                StreamEvent::MessageComplete { .. } => break,
                _ => {}
            }
        }
        Ok(result)
    }

    /// Runs the agent loop starting from `input`, returning every event
    /// emitted along the way.
    ///
    /// Per turn: build a [`ChatRequest`], stream the response while
    /// forwarding `TextDelta`s in arrival order, finalize the assistant
    /// message, then execute any requested tools sequentially and feed their
    /// outputs back as tool-result messages. Stops when a turn ends without
    /// tool calls (`TurnEnd`) or when cancellation fires
    /// ([`CoreError::Cancelled`]). A stall guard aborts 3 identical
    /// consecutive tool calls as [`CoreError::LoopDetected`]. Tool failures
    /// are *not* errors: they become `is_error` tool results so the model can
    /// recover.
    pub async fn run(&mut self, input: Message, ctx: ToolContext) -> Result<Vec<AgentEvent>, CoreError> {
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

        loop {
            // Cancellation is honored between turns, never mid-stream: a
            // half-finished assistant message would leave the transcript
            // inconsistent for the provider.
            if ctx.cancel.is_cancelled() {
                return Err(CoreError::Cancelled);
            }

            let req = ChatRequest {
                system: (!self.system.is_empty()).then(|| self.system.clone()),
                messages: self.messages.clone(),
                tools: self.tools.clone(),
            };

            // Accumulate streamed deltas: text chunks in order, tool calls
            // keyed by their stream index (id/name arrive once, arguments
            // may be split across many deltas).
            let mut text_parts: Vec<String> = Vec::new();
            let mut thinking_parts: Vec<String> = Vec::new();
            let mut pending: Vec<PendingToolCall> = Vec::new();
            let mut pending_emitted_start: Vec<bool> = Vec::new();
            // ponytail: emit ToolCallStart live as soon as name is known, so UI doesn't wait 50s for MessageComplete
            let (stop_reason, usage) = {
                let mut stream = self.provider.stream(req);
                loop {
                    let next_event = tokio::select! {
                        ev = stream.next() => ev,
                        _ = ctx.cancel.cancelled() => {
                            if !text_parts.is_empty() && pending.is_empty() {
                                let mut content = Vec::new();
                                let thinking = thinking_parts.concat();
                                if !thinking.is_empty() {
                                    content.push(ContentBlock::Thinking { text: thinking });
                                }
                                content.push(ContentBlock::Text { text: text_parts.concat() });
                                self.messages.push(Message {
                                    role: Role::Assistant,
                                    content,
                                });
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
                        Some(Ok(StreamEvent::ToolCallDelta { index, id, name, arguments_delta })) => {
                            // cap wire-controlled indices — a hostile/broken server
                            // sending index = 2^40 would otherwise allocate gigabytes here.
                            const MAX_TOOL_CALL_INDEX: usize = 4096;
                            if index > MAX_TOOL_CALL_INDEX {
                                return Err(CoreError::Provider(
                                    format!("tool-call index {index} exceeds limit ({MAX_TOOL_CALL_INDEX})"),
                                ));
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
                                let live_id = slot.id.clone().unwrap_or_else(|| format!("call_{index}"));
                                let live_name = slot.name.clone().unwrap_or_default();
                                emit!(AgentEvent::tool_call_start(live_id, live_name));
                                pending_emitted_start[index] = true;
                            }
                        }
                        Some(Ok(StreamEvent::MessageComplete { stop_reason, usage })) => {
                            break (stop_reason.unwrap_or(StopReason::EndTurn), usage.unwrap_or_default());
                        }
                        Some(Err(e)) => {
                            // Mid-stream failure after deltas already reached the
                            // user's screen: salvage the partial assistant text
                            // into history so the transcript matches what was seen.
                            if !text_parts.is_empty() && pending.is_empty() {
                                let mut content = Vec::new();
                                let thinking = thinking_parts.concat();
                                if !thinking.is_empty() {
                                    content.push(ContentBlock::Thinking { text: thinking });
                                }
                                content.push(ContentBlock::Text { text: text_parts.concat() });
                                self.messages.push(Message {
                                    role: Role::Assistant,
                                    content,
                                });
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
            total_usage.input_tokens += usage.input_tokens;
            total_usage.output_tokens += usage.output_tokens;
            total_usage.cached_tokens += usage.cached_tokens;
            total_usage.reasoning_tokens += usage.reasoning_tokens;
            total_usage.non_cached_input_tokens += usage.non_cached_input_tokens;
            total_usage.cache_read_input_tokens += usage.cache_read_input_tokens;
            total_usage.cache_write_input_tokens += usage.cache_write_input_tokens;
            total_usage.total_tokens += usage.total_tokens;
            // Keep legacy alias in sync
            if usage.cache_read_input_tokens != 0 {
                total_usage.cached_tokens = total_usage.cache_read_input_tokens;
            }

            // Finalize the assistant message exactly as streamed.
            // Reasoning precedes text, mirroring the provider's emission order
            // (pi renders runs of thinking blocks ahead of prose).
            let mut content: Vec<ContentBlock> = Vec::new();
            let thinking = thinking_parts.concat();
            if !thinking.is_empty() {
                content.push(ContentBlock::Thinking { text: thinking });
            }
            let text = text_parts.concat();
            if !text.is_empty() {
                content.push(ContentBlock::Text { text });
            }
            for (index, call) in pending.iter().enumerate() {
                content.push(ContentBlock::ToolUse {
                    id: call.id.clone().unwrap_or_else(|| format!("call_{index}")),
                    name: call.name.clone().unwrap_or_default(),
                    args: call.parsed_args(),
                });
            }
            let assistant = Message { role: Role::Assistant, content };
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
                    return Err(CoreError::LoopDetected(format!(
                        "same tool call 3× in a row: {sig}"
                    )));
                }
            }

            for (idx, (id, name, args)) in tool_uses.into_iter().enumerate() {
                if ctx.cancel.is_cancelled() {
                    return Err(CoreError::Cancelled);
                }
                if !pending_emitted_start.get(idx).copied().unwrap_or(false) {
                    emit!(AgentEvent::tool_call_start(id.clone(), name.clone()));
                }
                emit!(AgentEvent::tool_call_end(id.clone(), args.clone()));

                let output = tokio::select! {
                    out = self.executor.execute(&ctx, &name, args) => out,
                    _ = ctx.cancel.cancelled() => return Err(CoreError::Cancelled),
                };

                emit!(AgentEvent::tool_result(
                    id.clone(),
                    output.content.clone(),
                    output.is_error,
                ));
                self.messages.push(Message {
                    role: Role::User,
                    content: vec![ContentBlock::ToolResult {
                        id,
                        content: output.content,
                        is_error: output.is_error,
                    }],
                });
            }
        }
    }
}

/// A partially-streamed tool call awaiting its `MessageComplete`.
#[derive(Default)]
struct PendingToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl PendingToolCall {
    /// Parses accumulated argument JSON; unparseable fragments degrade to a
    /// string payload rather than aborting the run.
    fn parsed_args(&self) -> serde_json::Value {
        if self.arguments.is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&self.arguments)
                .unwrap_or(serde_json::Value::String(self.arguments.clone()))
        }
    }
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Provider whose responses are scripted up front, one event list per
    /// expected request.
    struct FakeProvider {
        scripted: Mutex<VecDeque<Vec<StreamEvent>>>,
    }

    impl FakeProvider {
        fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
            Self { scripted: Mutex::new(VecDeque::from(scripts)) }
        }
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn stream(&self, _req: ChatRequest) -> ProviderStream {
            let script = self
                .scripted
                .lock()
                .expect("scripted lock poisoned")
                .pop_front()
                .unwrap_or_default();
            Box::pin(futures::stream::iter(script.into_iter().map(Ok)))
        }
    }

    /// Executor that records every call and answers from a canned table,
    /// falling back to a default output for unknown tools.
    struct FakeExecutor {
        calls: std::sync::Arc<Mutex<Vec<String>>>,
        by_name: Vec<(String, ToolOutput)>,
        default_output: ToolOutput,
        on_execute: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    }

    impl FakeExecutor {
        fn new(default_output: ToolOutput) -> Self {
            Self {
                calls: std::sync::Arc::new(Mutex::new(Vec::new())),
                by_name: Vec::new(),
                default_output,
                on_execute: None,
            }
        }

        fn with_output(mut self, name: &str, output: ToolOutput) -> Self {
            self.by_name.push((name.to_string(), output));
            self
        }
    }

    #[async_trait]
    impl ToolExecutor for FakeExecutor {
        fn execute(
            &self,
            _ctx: &ToolContext,
            name: &str,
            _args: serde_json::Value,
        ) -> BoxFuture<'static, ToolOutput> {
            self.calls
                .lock()
                .expect("calls lock poisoned")
                .push(name.to_string());
            let output = self
                .by_name
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, o)| o.clone())
                .unwrap_or_else(|| self.default_output.clone());
            let hook = self.on_execute.clone();
            Box::pin(async move {
                if let Some(hook) = &hook {
                    hook();
                }
                output
            })
        }
    }

    const TOOL_NAME: &str = "lookup";

    fn tool_def() -> ToolDef {
        ToolDef::new(TOOL_NAME, "A fake lookup tool", serde_json::json!({}))
    }

    fn tool_script(id: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::text_delta("checking..."),
            StreamEvent::tool_call_delta(0, Some(id.to_string()), Some(TOOL_NAME.to_string()), r#"{"q":"#),
            StreamEvent::tool_call_delta(0, None, None, r#""x"}"#),
            StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(10, 5))),
        ]
    }

    #[tokio::test]
    async fn reasoning_deltas_stream_and_persist_into_history() {
        let provider = FakeProvider::new(vec![vec![
            StreamEvent::thinking_delta("hmm "),
            StreamEvent::thinking_delta("let me think"),
            StreamEvent::text_delta("answer"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]]);
        let executor = FakeExecutor::new(ToolOutput::ok("unused"));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor));

        let events = agent.run(Message::user("go"), ToolContext::default()).await.unwrap();

        // Thinking deltas surface in the event stream, before the text.
        assert!(events.contains(&AgentEvent::thinking_delta("hmm ")));
        assert!(events.contains(&AgentEvent::thinking_delta("let me think")));
        let text_pos = events.iter().position(|e| *e == AgentEvent::text_delta("answer")).unwrap();
        let think_pos = events.iter().position(|e| *e == AgentEvent::thinking_delta("hmm ")).unwrap();
        assert!(think_pos < text_pos, "reasoning should precede prose");

        // ...and land in the transcript as a thinking block ahead of the text.
        let assistant = &agent.messages()[1];
        assert_eq!(
            assistant.content,
            vec![
                ContentBlock::Thinking { text: "hmm let me think".to_string() },
                ContentBlock::Text { text: "answer".to_string() },
            ]
        );
    }

    #[tokio::test]
    async fn happy_path_single_tool_round_trip() {
        let provider = FakeProvider::new(vec![
            tool_script("call_1"),
            vec![
                StreamEvent::text_delta("all done"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), Some(Usage::new(20, 10))),
            ],
        ]);
        let executor = FakeExecutor::new(ToolOutput::ok("result payload"));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_system("be terse")
            .with_tools(vec![tool_def()]);

        let events = agent
            .run(Message::user("find it"), ToolContext::default())
            .await
            .expect("run should succeed");

        assert_eq!(
            events,
            vec![
                AgentEvent::Start,
                AgentEvent::text_delta("checking..."),
                AgentEvent::tool_call_start("call_1", TOOL_NAME),
                AgentEvent::tool_call_end("call_1", serde_json::json!({"q": "x"})),
                AgentEvent::tool_result("call_1", "result payload", false),
                AgentEvent::text_delta("all done"),
                AgentEvent::turn_end(StopReason::EndTurn, Usage::new(30, 15)),
            ]
        );

        let msgs = agent.messages();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0], Message::user("find it"));
        assert_eq!(
            msgs[1].content,
            vec![
                ContentBlock::text("checking..."),
                ContentBlock::tool_use("call_1", TOOL_NAME, serde_json::json!({"q": "x"})),
            ]
        );
        assert_eq!(
            msgs[2].content,
            vec![ContentBlock::tool_result("call_1", "result payload", false)]
        );
        assert_eq!(msgs[3], Message::assistant("all done"));
    }

    #[tokio::test]
    async fn tool_error_is_fed_back_and_model_recovers() {
        let provider = FakeProvider::new(vec![
            tool_script("call_err"),
            vec![
                StreamEvent::text_delta("recovered"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
        ]);
        let executor =
            FakeExecutor::new(ToolOutput::ok("unused")).with_output(
                TOOL_NAME,
                ToolOutput::error("disk on fire"),
            );
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_tools(vec![tool_def()]);

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("run should succeed despite tool error");

        // Error surfaced as data in the event stream...
        assert_eq!(
            events,
            vec![
                AgentEvent::Start,
                AgentEvent::text_delta("checking..."),
                AgentEvent::tool_call_start("call_err", TOOL_NAME),
                AgentEvent::tool_call_end("call_err", serde_json::json!({"q": "x"})),
                AgentEvent::tool_result("call_err", "disk on fire", true),
                AgentEvent::text_delta("recovered"),
                AgentEvent::turn_end(StopReason::EndTurn, Usage::new(10, 5)),
            ]
        );
        // ...and in the transcript handed back to the model.
        let feedback = &agent.messages()[2];
        assert_eq!(
            feedback.content,
            vec![ContentBlock::tool_result("call_err", "disk on fire", true)]
        );
        assert_eq!(agent.messages()[3], Message::assistant("recovered"));
    }

    #[tokio::test]
    async fn loop_guard_stops_identical_consecutive_tool_calls() {
        // Same tool+args 3× in a row → LoopDetected (replaces arbitrary max_turns).
        let provider = FakeProvider::new(vec![tool_script("c1"), tool_script("c2"), tool_script("c3")]);
        let executor = FakeExecutor::new(ToolOutput::ok("ok"));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);

        let err = agent
            .run(Message::user("loop forever"), ToolContext::default())
            .await
            .expect_err("should detect loop");

        assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
        // 3rd identical turn aborting before tool result: 1 user + 2 full rounds + 3rd assistant = 6
        assert_eq!(agent.messages().len(), 1 + 2 * 2 + 1);
    }

    #[tokio::test]
    async fn max_turns_guard_stops_runloop_with_error() {
        // Deprecated API is now a no-op — loop is unbounded with stall detection.
        // Keep test for compat: with_max_turns does not enforce a turn limit.
        let provider = FakeProvider::new(vec![
            tool_script("c1"),
            vec![
                StreamEvent::text_delta("done"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
        ]);
        let executor = FakeExecutor::new(ToolOutput::ok("ok"));
        #[allow(deprecated)]
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_tools(vec![tool_def()])
            .with_max_turns(2);

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("with_max_turns is now no-op, should succeed");
        assert!(events.iter().any(|e| matches!(e, AgentEvent::TurnEnd { .. })));
    }

    #[tokio::test]
    async fn text_deltas_forwarded_in_stream_order() {
        let provider = FakeProvider::new(vec![vec![
            StreamEvent::text_delta("alpha "),
            StreamEvent::text_delta("beta "),
            StreamEvent::text_delta("gamma"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]]);
        let mut agent =
            Agent::new(Box::new(provider), Box::new(FakeExecutor::new(ToolOutput::ok(""))));

        let events = agent
            .run(Message::user("hi"), ToolContext::default())
            .await
            .expect("run should succeed");

        let deltas: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { delta } => Some(delta.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["alpha ", "beta ", "gamma"]);
        // Reassembled text lands in the assistant message verbatim.
        assert_eq!(agent.messages()[1], Message::assistant("alpha beta gamma"));
    }

    #[tokio::test]
    async fn cancel_between_turns_aborts_gracefully() {
        let provider = FakeProvider::new(vec![tool_script("c1"), tool_script("never_reached")]);
        // The tool cancels the shared token mid-execution: the current turn
        // finishes cleanly, the *next* round observes cancellation.
        let cancel_token = CancellationToken::new();
        let mut executor = FakeExecutor::new(ToolOutput::ok("ok"));
        let token = cancel_token.clone();
        executor.on_execute = Some(std::sync::Arc::new(move || token.cancel()));
        let call_log = executor.calls.clone();
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_tools(vec![tool_def()]);

        let err = agent
            .run(Message::user("go"), ToolContext { cwd: ".".into(), cancel: cancel_token })
            .await
            .expect_err("cancelled run should surface Cancelled");

        assert!(matches!(err, CoreError::Cancelled), "got {err:?}");
        assert_eq!(
            call_log.lock().expect("calls lock poisoned").clone(),
            vec![TOOL_NAME.to_string()]
        );
    }

    #[tokio::test]
    async fn with_messages_preserves_prior_conversation_history() {
        let provider = FakeProvider::new(vec![vec![
            StreamEvent::text_delta("hello again"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]]);
        let prior = vec![
            Message::user("first question"),
            Message::assistant("first answer"),
        ];
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("ok"))),
        )
        .with_messages(prior);

        let events = agent
            .run(Message::user("second question"), ToolContext::default())
            .await
            .expect("run should succeed");

        assert!(events.contains(&AgentEvent::text_delta("hello again")));
        assert_eq!(agent.messages().len(), 4);
        assert_eq!(agent.messages()[0], Message::user("first question"));
        assert_eq!(agent.messages()[1], Message::assistant("first answer"));
        assert_eq!(agent.messages()[2], Message::user("second question"));
        assert_eq!(agent.messages()[3], Message::assistant("hello again"));
    }
}
