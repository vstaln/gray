//! Shared execution contracts: the seams between gray-core and the
//! provider/tools leaves. The binary wires implementations together.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::BoxStream;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::error::CoreError;
use crate::event::{StreamEvent, Usage};
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
    #[error("context exhausted — start /new or compact: {0}")]
    ContextOverflow(String),
    #[error("connection failed: {0}")]
    Connection(String),
    #[error("request timed out: {0}")]
    Timeout(String),
    #[error("server error: {0}")]
    ServerError(String),
    #[error("stream broken: {0}")]
    Stream(String),
}

impl ProviderError {
    /// True when the transcript should be compacted before retrying.
    pub fn should_compress(&self) -> bool {
        matches!(self, Self::ContextOverflow(_))
    }
}

/// Output of a tool execution. Errors are data for the model, not crashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
        }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
        }
    }
}

/// Per-execution context handed to tools.
#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub cancel: CancellationToken,
    /// Bridge for user-question tools (`request_user_input`); `None` means
    /// no interactive user is reachable.
    pub questions: Option<crate::questions::QuestionBridge>,
    pub session_id: Option<String>,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("."),
            cancel: CancellationToken::new(),
            questions: None,
            session_id: None,
        }
    }
}

/// A streaming LLM provider (wire protocol behind this seam).
#[async_trait]
pub trait Provider: Send + Sync {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>>;

    /// Model id behind this provider ("" when unknown). Used to stamp
    /// captured reasoning items so replay stays same-model-only.
    fn model_id(&self) -> &str {
        ""
    }
}

/// A single agent-callable tool.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Static definition surfaced to the model (name, description, schema).
    fn def(&self) -> crate::message::ToolDef;

    /// One-line snippet rendered in the system prompt's "Available tools" list.
    /// `None` hides the tool from that list (mirrors pi's `toolSnippets[name]` filter).
    fn prompt_snippet(&self) -> Option<&'static str> {
        None
    }

    /// Guideline bullets contributed to the system prompt when this tool is active.
    fn prompt_guidelines(&self) -> Option<&'static [&'static str]> {
        None
    }

    /// Executes the tool. Failures are data ([`ToolOutput::error`]), never panics.
    /// NOTE: an earlier `is_concurrency_safe` hook was
    /// deleted — tools run sequentially and nothing read it. If a parallel
    /// executor lands, re-add it then (bash/edit are the unsafe ones).
    async fn execute(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolOutput;
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

/// Verdict of a `tool/before` plugin hook (protocol v1) for one tool call.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolBefore {
    /// Run the call with the args the model sent.
    Allow,
    /// Run the call with rewritten args.
    Modify(serde_json::Value),
    /// Skip the executor; the reason becomes an `is_error` tool result.
    Deny(String),
}

impl ToolBefore {
    /// Parse a `tool/before` result. Lenient: unknown shapes fail open
    /// (pre-v1 behavior) so a confused plugin can't wedge the agent loop.
    pub fn from_result(v: &serde_json::Value, args: &serde_json::Value) -> Self {
        match v.get("decision").and_then(|d| d.as_str()) {
            Some("deny") => Self::Deny(
                v.get("reason")
                    .and_then(|r| r.as_str())
                    .filter(|r| !r.is_empty())
                    .unwrap_or("denied by plugin")
                    .to_string(),
            ),
            Some("modify") => Self::Modify(v.get("args").cloned().unwrap_or_else(|| args.clone())),
            _ => Self::Allow,
        }
    }
}

/// A slash command (`/x`) claimed by a plugin for `/help` + REPL routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    /// Command name with leading slash, as on the wire (`commands:["/x"]`).
    pub name: String,
    pub description: String,
}

/// Outcome of a plugin `command/run`: text to say, or a prompt to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Say(String),
    Prompt(String),
}

/// Host-side view of a plugin's protocol-v1 hooks (`prompt/context`,
/// `tool/before`, `command/run`). All methods are no-ops by default, so an
/// agent without plugins behaves exactly as before. The sidecar transport
/// behind these (Task 1) maps each method to its wire request.
#[async_trait]
pub trait PluginHooks: Send + Sync {
    /// Text from `prompt/context`, appended to the turn's system prompt.
    async fn prompt_context(&self) -> Option<String> {
        None
    }
    /// Verdict from `tool/before`, consulted before the executor runs.
    async fn tool_before(&self, _name: &str, _args: &serde_json::Value) -> ToolBefore {
        ToolBefore::Allow
    }
    /// Slash commands this plugin claims (shown in `/help`).
    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }
    /// Runs `command/run`; `None` means "not handled".
    async fn run_command(&self, _name: &str, _argv: Vec<String>) -> Option<CommandOutcome> {
        None
    }
    async fn pre_tool(&self, _name: &str, _args: &serde_json::Value) {}
    async fn post_tool(&self, _name: &str, _output: &ToolOutput) {}
    async fn turn_end(&self, _usage: &Usage) {}
    /// Graceful teardown (`plugin/shutdown` on sidecars). Default no-op.
    async fn shutdown(&self) {}
}

impl From<ProviderError> for CoreError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::Connection(msg) => CoreError::Connection(msg),
            ProviderError::Timeout(msg) => CoreError::Timeout(msg),
            other => CoreError::Provider(other.to_string()),
        }
    }
}

use futures::StreamExt as _;

use crate::message::{ContentBlock, Message, Role, ToolDef};

pub use super::agent_compact::summary_pair;

/// The agent loop: drives a conversation against a [`Provider`], executing
/// tool calls through a [`ToolExecutor`] until the model stops requesting
/// tools or cancellation fires.
///
/// `max_rounds` (default 50, [`with_max_rounds`](Self::with_max_rounds))
/// bounds total loop iterations. The loop terminates when a turn ends without
/// tool calls (`TurnEnd`) or when cancellation fires. A lightweight stall
/// guard (3 identical consecutive tool calls) aborts runaway loops; provider
/// context errors and user cancellation remain the other natural bounds.
///
/// `run` is in *collecting* form: it buffers all [`AgentEvent`]s and returns
/// them once the run finishes, rather than invoking a callback or yielding
/// through a channel. This keeps the core loop synchronous-in-shape (single
/// value out, single error path), trivially unit-testable, and free of
/// back-pressure concerns; a streaming façade can be layered on top later by
/// draining these events (or by swapping the return type for a receiver).
pub struct Agent {
    pub(crate) provider: Box<dyn Provider>,
    pub(crate) executor: Box<dyn ToolExecutor>,
    pub(crate) system: String,
    pub(crate) tools: Vec<ToolDef>,
    pub(crate) messages: Vec<Message>,
    pub(crate) max_rounds: Option<u32>,
    pub(crate) tool_timeout: Duration,
    pub(crate) pending_steer: Vec<String>,
    pub(crate) hooks: Vec<Arc<dyn PluginHooks>>,
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
            max_rounds: Some(50),
            tool_timeout: Duration::from_secs(120),
            pending_steer: Vec::new(),
            hooks: Vec::new(),
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

    /// Attaches plugin hooks (protocol v1). Empty by default: no hooks
    /// means the loop behaves exactly as before.
    pub fn with_hooks(mut self, hooks: Vec<Arc<dyn PluginHooks>>) -> Self {
        self.hooks = hooks;
        self
    }

    /// Plugin hooks attached via [`with_hooks`](Self::with_hooks) (REPL
    /// slash-command routing reads these).
    pub fn hooks(&self) -> &[Arc<dyn PluginHooks>] {
        &self.hooks
    }

    /// Sets the initial conversation messages (useful for resumed sessions).
    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self
    }

    /// Caps agent-loop rounds (default 50); `None` leaves the loop unbounded.
    pub fn with_max_rounds(mut self, max_rounds: Option<u32>) -> Self {
        self.max_rounds = max_rounds;
        self
    }

    /// Bounds per-tool execution (default 120s); timeouts become error results.
    pub fn with_tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    /// Queues a steering note for the running turn. Drained before the next
    /// request: appended to the newest tool result when one exists, else held
    /// for the next turn boundary as a real user message. `redirect` is just
    /// cancelling the [`ToolContext`] token, then calling this.
    pub fn steer(&mut self, s: String) {
        self.pending_steer.push(s);
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
    pub async fn complete_prompt(
        &self,
        prompt: &str,
        system: Option<&str>,
    ) -> Result<String, CoreError> {
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

    /// Drains queued [`steer`](Self::steer) text into history before the next
    /// request: appended to the newest tool result when one exists, else (at a
    /// turn boundary only) injected as a real user message; mid-turn with no
    /// tool results it stays queued for the next boundary.
    pub(crate) fn drain_steer(&mut self, turn_boundary: bool) {
        if self.pending_steer.is_empty() {
            return;
        }
        if let Some(content) = newest_tool_result_mut(&mut self.messages) {
            for text in std::mem::take(&mut self.pending_steer) {
                content.push_str(&format!("\n\n[steer] {text}"));
            }
        } else if turn_boundary {
            let joined = std::mem::take(&mut self.pending_steer).join("\n");
            self.messages.push(Message::user(joined));
        }
    }
}

/// Newest tool-result content in history, for [`Agent::steer`] injection.
fn newest_tool_result_mut(messages: &mut [Message]) -> Option<&mut String> {
    messages.iter_mut().rev().find_map(|m| {
        m.content.iter_mut().rev().find_map(|b| match b {
            ContentBlock::ToolResult { content, .. } => Some(content),
            _ => None,
        })
    })
}

/// Builds the finalized Thinking block, attaching captured Responses
/// reasoning replay data when present. `model` stamps the generating model
/// so replay stays same-model-only (a foreign model cannot decrypt the blob).
pub(crate) fn thinking_block(
    text: String,
    pending_reasoning: &Option<(String, String)>,
    model: &str,
) -> ContentBlock {
    let (item_id, encrypted_content, model) = match pending_reasoning {
        Some((i, e)) if !model.is_empty() => {
            (Some(i.clone()), Some(e.clone()), Some(model.to_string()))
        }
        _ => (None, None, None),
    };
    ContentBlock::Thinking {
        text,
        encrypted_content,
        item_id,
        model,
    }
}

/// Push streamed-so-far thinking + text so the transcript matches what the
/// user already saw on screen. Shared by the cancel and mid-stream-error arms
/// (the end-of-turn finalize differs: it also appends tool calls).
pub(crate) fn salvage_partial_text(
    messages: &mut Vec<Message>,
    thinking: String,
    text: String,
    pending_reasoning: &Option<(String, String)>,
    model: &str,
) {
    let mut content = Vec::new();
    if !thinking.is_empty() {
        content.push(thinking_block(thinking, pending_reasoning, model));
    }
    content.push(ContentBlock::Text { text });
    messages.push(Message {
        role: Role::Assistant,
        content,
    });
}

#[cfg(test)]
mod agent_tests {
    use super::*;
    use crate::event::{AgentEvent, StopReason, Usage};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Provider whose responses are scripted up front, one event list per
    /// expected request.
    struct FakeProvider {
        scripted: Mutex<VecDeque<Vec<StreamEvent>>>,
        failures: Mutex<VecDeque<ProviderError>>,
        seen_systems: std::sync::Arc<Mutex<Vec<Option<String>>>>,
    }

    impl FakeProvider {
        fn new(scripts: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                scripted: Mutex::new(VecDeque::from(scripts)),
                failures: Mutex::new(VecDeque::new()),
                seen_systems: std::sync::Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_failures(mut self, errs: Vec<ProviderError>) -> Self {
            self.failures = Mutex::new(VecDeque::from(errs));
            self
        }

        /// System prompts of every request served so far, in order.
        fn seen_systems(&self) -> std::sync::Arc<Mutex<Vec<Option<String>>>> {
            self.seen_systems.clone()
        }
    }

    #[async_trait]
    impl Provider for FakeProvider {
        fn stream(&self, req: ChatRequest) -> ProviderStream {
            self.seen_systems
                .lock()
                .expect("seen lock poisoned")
                .push(req.system.clone());
            if let Some(err) = self
                .failures
                .lock()
                .expect("failures lock poisoned")
                .pop_front()
            {
                return Box::pin(futures::stream::iter(vec![Err(err)]));
            }
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
        call_args: std::sync::Arc<Mutex<Vec<(String, serde_json::Value)>>>,
        by_name: Vec<(String, ToolOutput)>,
        default_output: ToolOutput,
        on_execute: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
        delay: Option<std::time::Duration>,
    }

    impl FakeExecutor {
        fn new(default_output: ToolOutput) -> Self {
            Self {
                calls: std::sync::Arc::new(Mutex::new(Vec::new())),
                call_args: std::sync::Arc::new(Mutex::new(Vec::new())),
                by_name: Vec::new(),
                default_output,
                on_execute: None,
                delay: None,
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
            args: serde_json::Value,
        ) -> BoxFuture<'static, ToolOutput> {
            self.calls
                .lock()
                .expect("calls lock poisoned")
                .push(name.to_string());
            self.call_args
                .lock()
                .expect("args lock poisoned")
                .push((name.to_string(), args.clone()));
            let output = self
                .by_name
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, o)| o.clone())
                .unwrap_or_else(|| self.default_output.clone());
            let hook = self.on_execute.clone();
            let delay = self.delay;
            Box::pin(async move {
                if let Some(hook) = &hook {
                    hook();
                }
                if let Some(d) = delay {
                    tokio::time::sleep(d).await;
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
            StreamEvent::tool_call_delta(
                0,
                Some(id.to_string()),
                Some(TOOL_NAME.to_string()),
                r#"{"q":"#,
            ),
            StreamEvent::tool_call_delta(0, None, None, r#""x"}"#),
            StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(10, 5))),
        ]
    }

    fn read_script(id: &str, path: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::text_delta("peeking..."),
            StreamEvent::tool_call_delta(
                0,
                Some(id.to_string()),
                Some("read".to_string()),
                &format!(r#"{{"path":"{path}"}}"#),
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ]
    }

    fn write_script(id: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::text_delta("writing..."),
            StreamEvent::tool_call_delta(
                0,
                Some(id.to_string()),
                Some("write".to_string()),
                r#"{"path":"/tmp/x","content":"y"}"#,
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), None),
        ]
    }

    fn end_script() -> Vec<StreamEvent> {
        vec![
            StreamEvent::text_delta("done"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
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

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        // Thinking deltas surface in the event stream, before the text.
        assert!(events.contains(&AgentEvent::thinking_delta("hmm ")));
        assert!(events.contains(&AgentEvent::thinking_delta("let me think")));
        let text_pos = events
            .iter()
            .position(|e| *e == AgentEvent::text_delta("answer"))
            .unwrap();
        let think_pos = events
            .iter()
            .position(|e| *e == AgentEvent::thinking_delta("hmm "))
            .unwrap();
        assert!(think_pos < text_pos, "reasoning should precede prose");

        // ...and land in the transcript as a thinking block ahead of the text.
        let assistant = &agent.messages()[1];
        assert_eq!(
            assistant.content,
            vec![
                ContentBlock::Thinking {
                    text: "hmm let me think".to_string(),
                    encrypted_content: None,
                    item_id: None,
                    model: None
                },
                ContentBlock::Text {
                    text: "answer".to_string()
                },
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
                AgentEvent::StepUsage {
                    usage: Usage::new(10, 5)
                },
                AgentEvent::tool_call_end("call_1", serde_json::json!({"q": "x"})),
                AgentEvent::tool_result("call_1", "result payload", false),
                AgentEvent::text_delta("all done"),
                AgentEvent::StepUsage {
                    usage: Usage::new(20, 15)
                },
                AgentEvent::turn_end(StopReason::EndTurn, Usage::new(20, 15)),
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
        let executor = FakeExecutor::new(ToolOutput::ok("unused"))
            .with_output(TOOL_NAME, ToolOutput::error("disk on fire"));
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);

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
                AgentEvent::StepUsage {
                    usage: Usage::new(10, 5)
                },
                AgentEvent::tool_call_end("call_err", serde_json::json!({"q": "x"})),
                AgentEvent::tool_result("call_err", "disk on fire", true),
                AgentEvent::text_delta("recovered"),
                AgentEvent::StepUsage {
                    usage: Usage::new(10, 5)
                },
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
        let provider = FakeProvider::new(vec![
            tool_script("c1"),
            tool_script("c2"),
            tool_script("c3"),
        ]);
        let executor = FakeExecutor::new(ToolOutput::ok("ok"));
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);

        let err = agent
            .run(Message::user("loop forever"), ToolContext::default())
            .await
            .expect_err("should detect loop");

        assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
        // 3rd identical turn aborting before tool result + synthetic tool_result: 1 user + 2 full rounds + 3rd assistant + 1 synthetic = 7
        assert_eq!(agent.messages().len(), 1 + 2 * 2 + 2);
    }

    #[tokio::test]
    async fn exploration_stall_aborts_varied_read_rounds() {
        // 25 rounds of `read`, each with a DIFFERENT path: the consecutive-identical
        // signature guard never trips, so the exploration-stall guard must.
        let scripts: Vec<Vec<StreamEvent>> = (0..25)
            .map(|i| read_script(&format!("r{i}"), &format!("/tmp/f{i}.rs")))
            .collect();
        let provider = FakeProvider::new(scripts);
        let executor = FakeExecutor::new(ToolOutput::ok("file body"));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor));

        let err = agent
            .run(Message::user("explore"), ToolContext::default())
            .await
            .expect_err("exploration-only loop should abort");

        assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn exploration_stall_injects_nudge_then_recovers() {
        // Nudge after 12 exploration-only rounds must land in history without
        // killing the run — the model can still end the turn cleanly.
        let mut scripts: Vec<Vec<StreamEvent>> = (0..12)
            .map(|i| read_script(&format!("r{i}"), &format!("/tmp/f{i}.rs")))
            .collect();
        scripts.push(end_script());
        let provider = FakeProvider::new(scripts);
        let executor = FakeExecutor::new(ToolOutput::ok("file body"));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor));

        let events = agent
            .run(Message::user("explore"), ToolContext::default())
            .await
            .expect("nudge should not kill the run");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
        assert!(
            agent
                .messages()
                .iter()
                .any(|m| m.text_content().contains("stall guard")),
            "expected a stall-guard nudge message in history"
        );
    }

    #[tokio::test]
    async fn mutating_tool_resets_exploration_streak() {
        // 11 reads → 1 write → 11 reads = 23 rounds: without the reset the
        // streak would pass 20 and abort; with it the run completes.
        let mut scripts: Vec<Vec<StreamEvent>> = Vec::new();
        for i in 0..11 {
            scripts.push(read_script(&format!("a{i}"), &format!("/tmp/a{i}.rs")));
        }
        scripts.push(write_script("w1"));
        for i in 0..11 {
            scripts.push(read_script(&format!("b{i}"), &format!("/tmp/b{i}.rs")));
        }
        scripts.push(end_script());
        let provider = FakeProvider::new(scripts);
        let executor = FakeExecutor::new(ToolOutput::ok("ok"));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor));

        let events = agent
            .run(Message::user("work"), ToolContext::default())
            .await
            .expect("a write must reset the stall streak");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
    }

    #[tokio::test]
    async fn stream_error_notices_forward_live_without_ending_turn() {
        // Codex steal: provider `Reconnecting...` notices ride as Ok events
        // so the turn continues; agent must forward them verbatim.
        let provider = FakeProvider::new(vec![vec![
            StreamEvent::stream_error("Reconnecting... 1/3", "status 503: boom"),
            StreamEvent::text_delta("still here"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok(""))),
        );

        let events = agent
            .run(Message::user("hi"), ToolContext::default())
            .await
            .expect("retry notice must not fail the run");

        assert!(
            events.contains(&AgentEvent::stream_error(
                "Reconnecting... 1/3",
                "status 503: boom"
            )),
            "expected forwarded StreamError, got {events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
    }

    #[tokio::test]
    async fn text_deltas_forwarded_in_stream_order() {
        let provider = FakeProvider::new(vec![vec![
            StreamEvent::text_delta("alpha "),
            StreamEvent::text_delta("beta "),
            StreamEvent::text_delta("gamma"),
            StreamEvent::message_complete(Some(StopReason::EndTurn), None),
        ]]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok(""))),
        );

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
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);

        let err = agent
            .run(
                Message::user("go"),
                ToolContext {
                    cwd: ".".into(),
                    cancel: cancel_token,
                    questions: None,
                    session_id: None,
                },
            )
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

    #[tokio::test]
    async fn malformed_tool_args_degrade_to_string_payload() {
        let provider = FakeProvider::new(vec![
            vec![
                StreamEvent::tool_call_delta(
                    0,
                    Some("c1".into()),
                    Some(TOOL_NAME.into()),
                    "not-json{{",
                ),
                StreamEvent::message_complete(Some(StopReason::ToolUse), None),
            ],
            vec![
                StreamEvent::text_delta("ok"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("ok"))),
        )
        .with_tools(vec![tool_def()]);
        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallEnd { args: serde_json::Value::String(s), .. } if s == "not-json{{")));
    }

    #[tokio::test]
    async fn unknown_tool_name_skips_executor_with_error_result() {
        let provider = FakeProvider::new(vec![
            vec![
                StreamEvent::tool_call_delta(
                    0,
                    Some("c-unknown".into()),
                    Some("nope".into()),
                    r#"{"q":"x"}"#,
                ),
                StreamEvent::message_complete(Some(StopReason::ToolUse), None),
            ],
            end_script(),
        ]);
        let executor = FakeExecutor::new(ToolOutput::ok("should-not-reach"));
        let call_log = executor.calls.clone();
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);
        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();
        assert!(
            call_log.lock().expect("calls lock poisoned").is_empty(),
            "executor must not run for unknown tool"
        );
        let (output, is_error) = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolResult {
                    id,
                    output,
                    is_error,
                } if id == "c-unknown" => Some((output.clone(), *is_error)),
                _ => None,
            })
            .expect("expected tool result for unknown tool");
        assert!(is_error, "unknown tool must be is_error, got {output}");
        assert!(
            output.contains("does not exist")
                && output.contains("nope")
                && output.contains(TOOL_NAME),
            "got {output}"
        );
        assert_eq!(agent.messages()[1].role, Role::Assistant);
        assert!(matches!(
            &agent.messages()[2].content[0],
            ContentBlock::ToolResult { is_error: true, .. }
        ));
    }

    #[tokio::test]
    async fn null_args_skips_executor_with_error_result() {
        let provider = FakeProvider::new(vec![
            vec![
                StreamEvent::tool_call_delta(0, Some("c-null".into()), Some(TOOL_NAME.into()), ""),
                StreamEvent::message_complete(Some(StopReason::ToolUse), None),
            ],
            end_script(),
        ]);
        let executor = FakeExecutor::new(ToolOutput::ok("should-not-reach"));
        let call_log = executor.calls.clone();
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);
        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();
        assert!(
            call_log.lock().expect("calls lock poisoned").is_empty(),
            "executor must not run for null args"
        );
        let (output, is_error) = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolResult {
                    id,
                    output,
                    is_error,
                } if id == "c-null" => Some((output.clone(), *is_error)),
                _ => None,
            })
            .expect("expected tool result for null args");
        assert!(is_error, "null args must be is_error, got {output}");
        assert!(output.contains("valid JSON object"), "got {output}");
    }

    #[tokio::test]
    async fn malformed_string_args_skips_executor_with_error_result() {
        let provider = FakeProvider::new(vec![
            vec![
                StreamEvent::tool_call_delta(
                    0,
                    Some("c-bad".into()),
                    Some(TOOL_NAME.into()),
                    "not-json{{",
                ),
                StreamEvent::message_complete(Some(StopReason::ToolUse), None),
            ],
            end_script(),
        ]);
        let executor = FakeExecutor::new(ToolOutput::ok("should-not-reach"));
        let call_log = executor.calls.clone();
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);
        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();
        assert!(
            call_log.lock().expect("calls lock poisoned").is_empty(),
            "executor must not run for malformed args"
        );
        let (output, is_error) = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolResult {
                    id,
                    output,
                    is_error,
                } if id == "c-bad" => Some((output.clone(), *is_error)),
                _ => None,
            })
            .expect("expected tool result for malformed args");
        assert!(is_error, "malformed args must be is_error, got {output}");
        assert!(output.contains("valid JSON object"), "got {output}");
    }

    // --- workstream A-core ---

    fn empty_script() -> Vec<StreamEvent> {
        vec![StreamEvent::message_complete(
            Some(StopReason::EndTurn),
            None,
        )]
    }

    fn maxtokens_script(text: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::text_delta(text.to_string()),
            StreamEvent::message_complete(Some(StopReason::MaxTokens), None),
        ]
    }

    #[test]
    fn should_compress_true_only_for_context_overflow() {
        assert!(ProviderError::ContextOverflow("ctx".into()).should_compress());
        assert!(!ProviderError::RateLimited("x".into()).should_compress());
        assert!(!ProviderError::BadRequest("x".into()).should_compress());
        assert!(!ProviderError::ServerError("x".into()).should_compress());
    }

    #[tokio::test]
    async fn context_overflow_compacts_once_then_continues() {
        let provider = FakeProvider::new(vec![
            vec![
                StreamEvent::text_delta("summarized"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
            vec![
                StreamEvent::text_delta("continued"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
        ])
        .with_failures(vec![ProviderError::ContextOverflow(
            "context exhausted — start /new or compact".into(),
        )]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        );

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("compact+retry should succeed");

        assert!(
            events
                .iter()
                .any(|e| *e == AgentEvent::text_delta("continued"))
        );
        assert!(
            agent.messages()[0]
                .text_content()
                .contains("<conversation_summary>"),
            "history must start with the summary pair"
        );
    }

    #[tokio::test]
    async fn context_overflow_surfaces_actionable_error_when_compact_fails() {
        let provider = FakeProvider::new(vec![]).with_failures(vec![
            ProviderError::ContextOverflow("boom".into()),
            ProviderError::ContextOverflow("boom".into()),
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        );

        let err = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect_err("must surface when compact also overflows");

        assert!(
            err.to_string().contains("context exhausted"),
            "actionable message, got {err}"
        );
    }

    #[tokio::test]
    async fn empty_turn_retries_twice_then_ends_with_sentinel() {
        let provider = FakeProvider::new(vec![
            empty_script(),
            empty_script(),
            empty_script(),
            empty_script(),
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        );

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("empty turn should end gracefully");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
        assert_eq!(agent.messages().last().unwrap().text_content(), "(empty)");
    }

    #[tokio::test]
    async fn empty_after_tool_results_nudges_once_then_continues() {
        let provider = FakeProvider::new(vec![
            tool_script("c1"),
            empty_script(),
            empty_script(),
            empty_script(),
            end_script(),
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("tool says hi"))),
        )
        .with_tools(vec![tool_def()]);

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("nudge should recover the turn");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
        assert!(
            agent
                .messages()
                .iter()
                .any(|m| m.text_content().contains("process results")),
            "expected the empty-after-tools nudge in history"
        );
    }

    fn tool_script_with_args(id: &str, args: &str) -> Vec<StreamEvent> {
        vec![
            StreamEvent::text_delta("checking..."),
            StreamEvent::tool_call_delta(
                0,
                Some(id.to_string()),
                Some(TOOL_NAME.to_string()),
                args,
            ),
            StreamEvent::message_complete(Some(StopReason::ToolUse), Some(Usage::new(10, 5))),
        ]
    }

    #[tokio::test]
    async fn alternating_tool_empty_stays_bounded_and_nudges_once() {
        // tool -> empty x3 (nudge) -> tool(varied) -> empty x3 -> end.
        // Without a once-per-run nudge gate the second empty burst would nudge
        // again and the alternation could continue unbounded.
        let provider = FakeProvider::new(vec![
            tool_script_with_args("c1", r#"{"q":"a"}"#),
            empty_script(),
            empty_script(),
            empty_script(),
            tool_script_with_args("c2", r#"{"q":"b"}"#),
            empty_script(),
            empty_script(),
            empty_script(),
            end_script(),
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("ok"))),
        )
        .with_tools(vec![tool_def()]);

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("alternation must terminate");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
        let nudges = agent
            .messages()
            .iter()
            .filter(|m| m.text_content().contains("process results"))
            .count();
        assert_eq!(nudges, 1, "post-tool empty nudge must fire once per run");
    }

    #[tokio::test]
    async fn maxtokens_truncation_continues_and_stitches_partial() {
        let provider = FakeProvider::new(vec![
            maxtokens_script("part1 "),
            vec![
                StreamEvent::text_delta("part2"),
                StreamEvent::message_complete(Some(StopReason::EndTurn), None),
            ],
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        );

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("continuation should succeed");

        let joined = agent
            .messages()
            .iter()
            .map(|m| m.text_content())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("part1 ") && joined.contains("part2"),
            "stitched: {joined}"
        );
        assert!(
            joined.contains("continue exactly where you left off"),
            "continuation prompt: {joined}"
        );
    }

    #[tokio::test]
    async fn maxtokens_continuations_are_capped_then_partial_kept() {
        let provider = FakeProvider::new(vec![
            maxtokens_script("a"),
            maxtokens_script("b"),
            maxtokens_script("c"),
            maxtokens_script("d"),
        ]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        );

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("capped continuation keeps partial");

        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
        let nudges = agent
            .messages()
            .iter()
            .filter(|m| {
                m.text_content()
                    .contains("continue exactly where you left off")
            })
            .count();
        assert_eq!(nudges, 2, "continuations must be capped");
    }

    #[tokio::test]
    async fn max_rounds_bound_aborts_runaway_loop() {
        let provider = FakeProvider::new(vec![tool_script("c1"), tool_script("c2")]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("ok"))),
        )
        .with_tools(vec![tool_def()])
        .with_max_rounds(Some(1));

        let err = agent
            .run(Message::user("loop"), ToolContext::default())
            .await
            .expect_err("max rounds must abort");

        assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
    }

    #[test]
    fn tool_timeout_builder_keeps_120s_default() {
        let agent = Agent::new(
            Box::new(FakeProvider::new(vec![])),
            Box::new(FakeExecutor::new(ToolOutput::ok(""))),
        );
        assert_eq!(agent.tool_timeout, std::time::Duration::from_secs(120));
        let agent = agent.with_tool_timeout(std::time::Duration::from_millis(50));
        assert_eq!(agent.tool_timeout, std::time::Duration::from_millis(50));
    }

    #[tokio::test]
    async fn tool_timeout_becomes_error_result_and_continues() {
        let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
        let mut executor = FakeExecutor::new(ToolOutput::ok("too slow"));
        executor.delay = Some(std::time::Duration::from_secs(5));
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_tools(vec![tool_def()])
            .with_tool_timeout(std::time::Duration::from_millis(50));

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .expect("timeout must not fail the run");

        let (output, is_error) = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolResult {
                    id,
                    output,
                    is_error,
                } if id == "c1" => Some((output.clone(), *is_error)),
                _ => None,
            })
            .expect("expected tool result after timeout");
        assert!(is_error, "timeout must be is_error, got {output}");
        assert!(output.contains("timed out"), "got {output}");
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AgentEvent::TurnEnd { .. }))
        );
    }

    #[tokio::test]
    async fn steer_appends_to_newest_tool_result() {
        let provider = FakeProvider::new(vec![tool_script("c1"), end_script(), end_script()]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("data"))),
        )
        .with_tools(vec![tool_def()]);
        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        agent.steer("be faster".to_string());
        agent
            .run(Message::user("next"), ToolContext::default())
            .await
            .unwrap();

        let newest = agent
            .messages()
            .iter()
            .rev()
            .find_map(|m| {
                m.content.iter().find_map(|b| match b {
                    ContentBlock::ToolResult { content, .. } => Some(content.clone()),
                    _ => None,
                })
            })
            .expect("tool result must exist");
        assert!(newest.contains("[steer] be faster"), "got {newest}");
    }

    #[tokio::test]
    async fn steer_without_tool_results_becomes_user_message() {
        let provider = FakeProvider::new(vec![end_script()]);
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        );

        agent.steer("focus on tests".to_string());
        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        assert_eq!(agent.messages()[1], Message::user("focus on tests"));
    }

    #[test]
    fn summary_pair_envelope_is_byte_stable() {
        let [u, a] = super::summary_pair("  hello world  ");
        assert_eq!(
            u.text_content(),
            "<conversation_summary>\nhello world\n</conversation_summary>\n\nPlease continue assisting based on the summary above."
        );
        assert_eq!(
            a.text_content(),
            "Understood. I have reviewed the conversation summary and context, and I am ready to continue."
        );
        // Byte-equality: trimming + envelope must never drift.
        let [u2, a2] = super::summary_pair("hello world");
        assert_eq!(u.text_content().as_bytes(), u2.text_content().as_bytes());
        assert_eq!(a.text_content().as_bytes(), a2.text_content().as_bytes());
    }

    /// Stub `prompt/context` hook: returns fixed text like a sidecar's
    /// `{"result":{"text":"…"}}` reply.
    struct CtxHook {
        text: Option<String>,
    }

    #[async_trait]
    impl PluginHooks for CtxHook {
        async fn prompt_context(&self) -> Option<String> {
            self.text.clone()
        }
    }

    #[tokio::test]
    async fn prompt_context_replies_concatenate_onto_system() {
        let provider = FakeProvider::new(vec![end_script()]);
        let seen = provider.seen_systems();
        let mut agent = Agent::new(
            Box::new(provider),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        )
        .with_system("BASE-SYSTEM")
        .with_hooks(vec![
            Arc::new(CtxHook {
                text: Some("PLUGIN-CTX-AAA".to_string()),
            }),
            Arc::new(CtxHook { text: None }),
            Arc::new(CtxHook {
                text: Some("PLUGIN-CTX-BBB".to_string()),
            }),
        ]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        let seen = seen.lock().expect("seen lock poisoned");
        assert_eq!(seen.len(), 1, "one turn → one request, got {seen:?}");
        let system = seen[0].as_deref().unwrap_or("");
        assert!(
            system.contains("BASE-SYSTEM"),
            "base prompt preserved, got: {system}"
        );
        let (a, b) = (system.find("PLUGIN-CTX-AAA"), system.find("PLUGIN-CTX-BBB"));
        assert!(
            a.is_some() && b.is_some(),
            "both hook replies present, got: {system}"
        );
        assert!(a.unwrap() < b.unwrap(), "hook order kept, got: {system}");
    }

    /// Stub `tool/before` hook: denies or rewrites every call, like a
    /// sidecar's `{"decision":"deny"|"modify",…}` reply.
    struct VetoHook {
        deny: Option<String>,
        rewrite: Option<serde_json::Value>,
    }

    #[async_trait]
    impl PluginHooks for VetoHook {
        async fn tool_before(&self, _name: &str, _args: &serde_json::Value) -> ToolBefore {
            if let Some(reason) = &self.deny {
                return ToolBefore::Deny(reason.clone());
            }
            if let Some(args) = &self.rewrite {
                return ToolBefore::Modify(args.clone());
            }
            ToolBefore::Allow
        }
    }

    #[tokio::test]
    async fn tool_before_deny_skips_executor_with_error_result() {
        let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
        let executor = FakeExecutor::new(ToolOutput::ok("must-not-run"));
        let call_log = executor.calls.clone();
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_tools(vec![tool_def()])
            .with_hooks(vec![Arc::new(VetoHook {
                deny: Some("DENIED-XYZ".to_string()),
                rewrite: None,
            })]);

        let events = agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        assert!(
            call_log.lock().expect("calls lock poisoned").is_empty(),
            "denied tool must never execute"
        );
        let (output, is_error) = events
            .iter()
            .find_map(|e| match e {
                AgentEvent::ToolResult {
                    output, is_error, ..
                } => Some((output.clone(), *is_error)),
                _ => None,
            })
            .expect("deny must still emit a tool result");
        assert!(is_error, "deny result is an error, got {output:?}");
        assert!(
            output.contains("DENIED-XYZ"),
            "reason surfaced, got {output:?}"
        );
        // History carries the same error result so alternation stays intact.
        let stored = agent
            .messages()
            .iter()
            .flat_map(|m| m.content.iter())
            .find_map(|b| match b {
                ContentBlock::ToolResult {
                    content, is_error, ..
                } => Some((content.clone(), *is_error)),
                _ => None,
            })
            .expect("deny must leave a history tool result");
        assert!(
            stored.1 && stored.0.contains("DENIED-XYZ"),
            "got {stored:?}"
        );
    }

    #[tokio::test]
    async fn tool_before_modify_rewrites_executor_args() {
        let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
        let executor = FakeExecutor::new(ToolOutput::ok("ok"));
        let arg_log = executor.call_args.clone();
        let mut agent = Agent::new(Box::new(provider), Box::new(executor))
            .with_tools(vec![tool_def()])
            .with_hooks(vec![Arc::new(VetoHook {
                deny: None,
                rewrite: Some(serde_json::json!({"q": "rewritten"})),
            })]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        let logged = arg_log.lock().expect("args lock poisoned");
        assert_eq!(logged.len(), 1, "executor runs once, got {logged:?}");
        assert_eq!(
            logged[0].1,
            serde_json::json!({"q": "rewritten"}),
            "got {logged:?}"
        );
    }

    #[tokio::test]
    async fn tool_before_absent_leaves_args_untouched() {
        let provider = FakeProvider::new(vec![tool_script("c1"), end_script()]);
        let executor = FakeExecutor::new(ToolOutput::ok("ok"));
        let arg_log = executor.call_args.clone();
        let mut agent =
            Agent::new(Box::new(provider), Box::new(executor)).with_tools(vec![tool_def()]);

        agent
            .run(Message::user("go"), ToolContext::default())
            .await
            .unwrap();

        let logged = arg_log.lock().expect("args lock poisoned");
        assert_eq!(logged.len(), 1, "executor runs once, got {logged:?}");
        assert_eq!(logged[0].0, TOOL_NAME);
        assert_eq!(logged[0].1, serde_json::json!({"q": "x"}), "got {logged:?}");
    }

    /// Counting hook for lifecycle emission: records pre/post order and
    /// counts turn_end calls.
    struct LifecycleHook {
        turn_ends: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        calls: std::sync::Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl PluginHooks for LifecycleHook {
        async fn pre_tool(&self, name: &str, _args: &serde_json::Value) {
            self.calls.lock().expect("calls lock poisoned").push(format!("pre:{name}"));
        }
        async fn post_tool(&self, name: &str, _output: &ToolOutput) {
            self.calls.lock().expect("calls lock poisoned").push(format!("post:{name}"));
        }
        async fn turn_end(&self, _usage: &Usage) {
            self.turn_ends.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn turn_end_hook_called_once_on_end_and_on_error() {
        // Success path: exactly once.
        let ends = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let mut agent = Agent::new(
            Box::new(FakeProvider::new(vec![end_script()])),
            Box::new(FakeExecutor::new(ToolOutput::ok("unused"))),
        )
        .with_hooks(vec![Arc::new(LifecycleHook { turn_ends: ends.clone(), calls })]);
        agent.run(Message::user("go"), ToolContext::default()).await.unwrap();
        assert_eq!(ends.load(std::sync::atomic::Ordering::SeqCst), 1, "turn_end once on success");

        // Error path (identical-tool loop → LoopDetected): still exactly once.
        let ends_err = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls_err = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let mut agent = Agent::new(
            Box::new(FakeProvider::new(vec![tool_script("c1"), tool_script("c2"), tool_script("c3")])),
            Box::new(FakeExecutor::new(ToolOutput::ok("ok"))),
        )
        .with_tools(vec![tool_def()])
        .with_hooks(vec![Arc::new(LifecycleHook { turn_ends: ends_err.clone(), calls: calls_err })]);
        let err = agent.run(Message::user("loop"), ToolContext::default()).await.expect_err("loop must fail");
        assert!(matches!(err, CoreError::LoopDetected(_)), "got {err:?}");
        assert_eq!(ends_err.load(std::sync::atomic::Ordering::SeqCst), 1, "turn_end once on error");
    }

    #[tokio::test]
    async fn pre_post_hooks_emit_around_tool_execution() {
        let ends = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let calls = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
        let mut agent = Agent::new(
            Box::new(FakeProvider::new(vec![tool_script("c1"), end_script()])),
            Box::new(FakeExecutor::new(ToolOutput::ok("ok"))),
        )
        .with_tools(vec![tool_def()])
        .with_hooks(vec![Arc::new(LifecycleHook { turn_ends: ends.clone(), calls: calls.clone() })]);

        agent.run(Message::user("go"), ToolContext::default()).await.unwrap();

        let logged = calls.lock().expect("calls lock poisoned").clone();
        assert_eq!(logged, vec![format!("pre:{TOOL_NAME}"), format!("post:{TOOL_NAME}")], "order pre→post, got {logged:?}");
        // Sidecar-provided tools run through the same executor call site as
        // builtin tools, so this emission covers both by construction.
    }
}
