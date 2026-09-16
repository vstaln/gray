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
use crate::event::{StreamEvent, Usage, append_thinking_chunk};
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
    pub session_id: Option<String>,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("."),
            cancel: CancellationToken::new(),
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
    /// Parse a `tool/before` result. Strict: only documented verdicts
    /// authorize or rewrite; anything else denies (a claimed hook's
    /// confusion must not become permission).
    pub fn from_result(v: &serde_json::Value) -> Self {
        match v.get("decision").and_then(|d| d.as_str()) {
            Some("deny") => Self::Deny(
                v.get("reason")
                    .and_then(|r| r.as_str())
                    .filter(|r| !r.is_empty())
                    .unwrap_or("denied by plugin")
                    .to_string(),
            ),
            Some("modify") => match v.get("args") {
                Some(a) if a.is_object() => Self::Modify(a.clone()),
                _ => Self::Deny("plugin modify verdict missing object args".to_string()),
            },
            // A claimed hook's verdict must be explicit: anything but a
            // documented "allow" denies rather than silently authorizing.
            Some("allow") => Self::Allow,
            _ => Self::Deny("plugin returned an unrecognized policy verdict".to_string()),
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
/// The loop terminates when a turn ends without tool calls (`TurnEnd`) or when
/// cancellation fires. A lightweight stall
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
    pub(crate) executor: std::sync::Arc<dyn ToolExecutor>,
    pub(crate) system: String,
    pub(crate) tools: Vec<ToolDef>,
    pub(crate) messages: Vec<Message>,
    pub(crate) tool_timeout: Duration,
    pub(crate) hooks: Vec<Arc<dyn PluginHooks>>,
    pub(crate) context_window: Option<usize>,
}

impl Agent {
    /// Creates an agent over the given provider and tool executor.
    pub fn new(provider: Box<dyn Provider>, executor: std::sync::Arc<dyn ToolExecutor>) -> Self {
        Self {
            provider,
            executor,
            system: String::new(),
            tools: Vec::new(),
            messages: Vec::new(),
            tool_timeout: Duration::from_secs(120),
            hooks: Vec::new(),
            context_window: None,
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

    /// Bounds per-tool execution (default 120s); timeouts become error results.
    pub fn with_tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    /// Known model context window in tokens (`None` = unknown: only
    /// overflow-recovery compaction runs). Set via
    /// [`with_context_window`](Self::with_context_window) from
    /// `resolve_model_context_length` at build surfaces.
    pub fn with_context_window(mut self, window: Option<usize>) -> Self {
        self.context_window = window;
        self
    }

    /// Rough transcript size in tokens (bytes/4 — same approximation as
    /// `gray_tools::stats::est_tokens`). Delegates to the shared
    /// `agent_compact::est_tokens` owner so the estimators can never drift.
    pub(crate) fn estimate_tokens(&self) -> usize {
        crate::agent_compact::est_tokens(&self.messages)
    }

    /// Read-only view of the accumulated conversation so far.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Updates or replaces the accumulated conversation messages (e.g. after compaction).
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// System prompt sent with every request. `pub(crate)` because the
    /// compaction-v2 trigger call (sibling module `compact`) reuses it
    /// verbatim: private fields are visible only in the defining module, so
    /// the sibling cannot read `self.system` directly.
    pub(crate) fn system_text(&self) -> &str {
        &self.system
    }

    /// Tools advertised to the model. `pub(crate)` for the same reason as
    /// [`system_text`](Self::system_text): the compaction-v2 trigger call
    /// reuses them verbatim.
    pub(crate) fn tool_defs(&self) -> &[ToolDef] {
        &self.tools
    }

    /// Single-turn completion over `history` + `tools`: streams one assistant
    /// reply and returns its prose. The compaction-v2 in-band trigger call
    /// passes the live system + tools so the request prefix stays cache-hot.
    pub async fn complete_with_history(
        &self,
        system: Option<&str>,
        messages: Vec<Message>,
        tools: Vec<ToolDef>,
    ) -> Result<String, CoreError> {
        let req = ChatRequest {
            system: system.map(|s| s.to_string()),
            messages,
            tools,
        };
        drain_reply_text(self.provider.stream(req)).await
    }
}

/// Single-turn reply drain behind [`Agent::complete_with_history`]: collects
/// `Text`/`Thinking` prose only and drops tool calls — the compaction trigger
/// reply must never execute tools (codex v2 likewise collects only the
/// compaction output item).
async fn drain_reply_text(mut stream: ProviderStream) -> Result<String, CoreError> {
    let mut result = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { delta } => {
                result.push_str(&delta);
            }
            StreamEvent::ThinkingDelta { delta } => {
                append_thinking_chunk(&mut result, &delta);
            }
            StreamEvent::MessageComplete { .. } => break,
            _ => {}
        }
    }
    Ok(result)
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

#[path = "agent_tests.rs"]
#[cfg(test)]
mod agent_tests;
