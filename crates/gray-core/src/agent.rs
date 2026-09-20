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
    /// Vision attachments (opencode `read` parity: "Image read successfully"
    /// + file part). Empty for every other tool.
    ///
    /// `#[serde(default)]` keeps old transcripts parsing.
    #[serde(default)]
    pub images: Vec<AttachedImage>,
}

/// One downscaled image riding with a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedImage {
    pub media_type: String,
    pub data: String,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: Vec::new(),
        }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            images: Vec::new(),
        }
    }
    /// Image read success: text note + one vision block (opencode parity).
    pub fn image(content: impl Into<String>, media_type: String, data: String) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: vec![AttachedImage { media_type, data }],
        }
    }
    /// Message blocks for this result: the text result first, then one
    /// vision block per attached image. Image blocks in user-role messages
    /// already serialize as vision parts on every provider path.
    pub fn message_blocks(&self, id: &str) -> Vec<ContentBlock> {
        let mut blocks = vec![ContentBlock::ToolResult {
            id: id.to_string(),
            content: self.content.clone(),
            is_error: self.is_error,
        }];
        blocks.extend(self.images.iter().map(|img| ContentBlock::Image {
            media_type: img.media_type.clone(),
            data: img.data.clone(),
        }));
        blocks
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

    /// Drain completed background notices without waiting. Called only at safe
    /// transcript boundaries, never in the middle of tool-result placement.
    fn drain_notifications(&self, _ctx: &ToolContext) -> Vec<String> {
        Vec::new()
    }

    /// Executes the tool. Failures are data ([`ToolOutput::error`]), never panics.
    async fn execute(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolOutput;
}

/// Executes named tools. The registry lives behind this seam so core
/// never knows what tools exist.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn drain_notifications(&self, _ctx: &ToolContext) -> Vec<String> {
        Vec::new()
    }

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
        // 1:1 with the provider taxonomy so the failure class survives the
        // boundary (harnesses branch on `CoreError::code`, not message text).
        match e {
            ProviderError::RateLimited(msg) => CoreError::RateLimited(msg),
            ProviderError::Auth(msg) => CoreError::Auth(msg),
            ProviderError::BadRequest(msg) => CoreError::BadRequest(msg),
            ProviderError::ContextOverflow(msg) => CoreError::ContextOverflow(msg),
            ProviderError::Connection(msg) => CoreError::Connection(msg),
            ProviderError::Timeout(msg) => CoreError::Timeout(msg),
            ProviderError::ServerError(msg) => CoreError::ServerError(msg),
            ProviderError::Stream(msg) => CoreError::Stream(msg),
        }
    }
}

use futures::StreamExt as _;

use crate::message::{ContentBlock, Message, Role, ToolDef};

pub use super::agent_compact::{estimate_message_tokens, summary_message};

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
    /// Effective prefix captured once per turn, including plugin context.
    pub(crate) turn_system: Option<String>,
    pub(crate) tools: Vec<ToolDef>,
    pub(crate) messages: Vec<Message>,
    pub(crate) tool_timeout: Duration,
    pub(crate) hooks: Vec<Arc<dyn PluginHooks>>,
    pub(crate) context_window: Option<usize>,
    /// Latest provider-reported context size and the history length it
    /// covered (pi `getLastAssistantUsage`). Cleared on every history
    /// rewrite: a pre-rewrite report describes a context that no longer exists.
    context_usage: Option<(usize, usize)>,
    history_revision: u64,
    history_rewrite_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Agent {
    /// Creates an agent over the given provider and tool executor.
    pub fn new(provider: Box<dyn Provider>, executor: std::sync::Arc<dyn ToolExecutor>) -> Self {
        Self {
            provider,
            executor,
            system: String::new(),
            turn_system: None,
            tools: Vec::new(),
            messages: Vec::new(),
            tool_timeout: Duration::from_secs(120),
            hooks: Vec::new(),
            context_window: None,
            context_usage: None,
            history_revision: 0,
            history_rewrite_hook: None,
        }
    }

    /// Decorate all provider requests, including compaction, with host policy.
    pub fn map_provider(
        mut self,
        wrap: impl FnOnce(Box<dyn Provider>) -> Box<dyn Provider>,
    ) -> Self {
        self.provider = wrap(self.provider);
        self
    }

    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }

    pub fn with_history_rewrite_hook(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.history_rewrite_hook = Some(hook);
        self
    }

    pub(crate) fn history_rewritten(&mut self) {
        self.context_usage = None;
        self.history_revision = self.history_revision.wrapping_add(1);
        if let Some(hook) = &self.history_rewrite_hook {
            hook();
        }
    }

    /// Sets the system prompt sent with every request.
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self.turn_system = None;
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
        self.turn_system = None;
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
        self.history_rewritten();
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

    /// Context size in tokens, pi `estimateContextTokens`: the latest
    /// provider-reported total (which already includes the system prompt and
    /// tool definitions) plus a bytes/4 estimate of the messages appended
    /// since. Without a report (fresh, resumed or just-rewritten history) it
    /// falls back to bytes/4 over the whole transcript, via the shared
    /// `agent_compact::est_tokens` owner so the estimators can never drift.
    pub(crate) fn estimate_tokens(&self) -> usize {
        match self.context_usage {
            Some((tokens, covered)) if covered <= self.messages.len() => {
                tokens + crate::agent_compact::est_tokens(&self.messages[covered..])
            }
            _ => crate::agent_compact::est_tokens(&self.messages),
        }
    }

    /// Anchors [`estimate_tokens`](Self::estimate_tokens) on one provider
    /// report covering the current history. Rounds without usage keep the
    /// previous anchor; the trailing estimate covers what came after it.
    pub(crate) fn record_context_usage(&mut self, usage: &crate::event::Usage) {
        let tokens = usage.total();
        if tokens > 0 {
            self.context_usage = Some((tokens, self.messages.len()));
        }
    }

    /// Read-only view of the accumulated conversation so far.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Updates or replaces the accumulated conversation messages (e.g. after compaction).
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.history_rewritten();
    }

    /// Repairs a turn the caller had to drop mid-flight: its bounded cancel grace
    /// expired (a plugin hook, compaction or tool that ignores cancellation), so
    /// none of the loop's own cancel arms ran. This restores the two invariants
    /// they guarantee, and only appends what is missing — a run that finished on
    /// its own never needs it:
    /// 1. every `tool_use` carries a `tool_result`. Strict providers 400 on an
    ///    orphaned call, which bricks the saved session for good.
    /// 2. text the user already saw on screen is not silently dropped from the
    ///    transcript a resumed session replays.
    pub fn repair_dropped_cancel(&mut self, thinking: &str, text: &str) {
        let mut answered = std::collections::HashSet::new();
        let mut orphaned: Vec<String> = Vec::new();
        for message in &self.messages {
            for block in &message.content {
                match block {
                    ContentBlock::ToolUse { id, .. } => orphaned.push(id.clone()),
                    ContentBlock::ToolResult { id, .. } => {
                        answered.insert(id.clone());
                    }
                    _ => {}
                }
            }
        }
        for id in orphaned.into_iter().filter(|id| !answered.contains(id)) {
            crate::agent_tools::push_synthetic(self, &id, "cancelled by user");
        }
        // Same text the loop already finalized (identical delta stream) must not
        // be appended twice.
        let already_recorded = self.messages[current_turn_start(&self.messages)..]
            .iter()
            .any(|m| {
                m.role == Role::Assistant
                    && m.content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::Text { text: seen } if seen == text))
            });
        if !text.is_empty() && !already_recorded {
            let mut content = Vec::new();
            if !thinking.is_empty() {
                content.push(thinking_block(
                    thinking.to_string(),
                    &None,
                    self.provider.model_id(),
                ));
            }
            content.push(ContentBlock::Text {
                text: text.to_string(),
            });
            self.messages.push(Message {
                role: Role::Assistant,
                content,
            });
        }
    }

    /// Effective system prompt captured for the current/last turn, or the base
    /// prompt before any turn. `pub(crate)` because the
    /// compaction-v2 trigger call (sibling module `compact`) reuses it
    /// verbatim: private fields are visible only in the defining module, so
    /// the sibling cannot read `self.system` directly.
    pub(crate) fn system_text(&self) -> &str {
        self.turn_system.as_deref().unwrap_or(&self.system)
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

/// Index just past the last genuine user message: the boundary of the turn
/// currently being built. Synthetic `tool_result` messages are user-role but
/// never start a turn, so they cannot hide earlier output.
fn current_turn_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|m| {
            m.role == Role::User
                && m.content
                    .iter()
                    .any(|b| !matches!(b, ContentBlock::ToolResult { .. }))
        })
        .map_or(0, |i| i + 1)
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

#[path = "agent_repair_tests.rs"]
#[cfg(test)]
mod repair_tests;
