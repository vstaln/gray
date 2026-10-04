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

    /// True when re-running the same request can plausibly succeed: the
    /// provider or network failed, not the request. Mirrors
    /// [`CoreError::retryable`](crate::error::CoreError::retryable) so the
    /// agent loop can decide on the raw provider class before conversion.
    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited(_)
                | Self::ServerError(_)
                | Self::Stream(_)
                | Self::Connection(_)
                | Self::Timeout(_)
        )
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
    /// Non-image media riding with a tool result (bash `cat` of a clip,
    /// PDF or audio file). Each carries what a model that can't take it
    /// sees instead; the provider picks via [`crate::message::resolve_media`].
    #[serde(default, alias = "videos")]
    pub media: Vec<AttachedMedia>,
}

/// One downscaled image riding with a tool result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedImage {
    pub media_type: String,
    pub data: String,
}

/// One base64 video/audio/PDF riding with a tool result. Not re-encoded, so
/// the producer keeps it under [`crate::message::MAX_NATIVE_MEDIA_BYTES`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedMedia {
    pub media_type: String,
    pub data: String,
    /// What a model without this input sees instead (contact sheet, PDF
    /// text, a note). The tool cannot know the model.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fallback: Vec<ContentBlock>,
}

impl ToolOutput {
    pub fn ok(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: Vec::new(),
            media: Vec::new(),
        }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            images: Vec::new(),
            media: Vec::new(),
        }
    }
    /// Image read success: text note + one vision block (opencode parity).
    pub fn image(content: impl Into<String>, media_type: String, data: String) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: vec![AttachedImage { media_type, data }],
            media: Vec::new(),
        }
    }
    /// Message blocks for this result: the text result first, then one
    /// vision block per attached image, then one media block per attachment.
    /// Image blocks in user-role messages already serialize as vision parts
    /// on every provider path; media resolves per model there.
    pub fn message_blocks(&self, id: &str) -> Vec<ContentBlock> {
        let mut blocks = vec![ContentBlock::ToolResult {
            id: id.to_string(),
            content: self.content.clone(),
            is_error: self.is_error,
        }];
        for img in &self.images {
            blocks.push(ContentBlock::image(&img.media_type, &img.data));
        }
        for m in &self.media {
            blocks.push(ContentBlock::media(
                &m.media_type,
                &m.data,
                m.fallback.clone(),
            ));
        }
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

/// Host callback polled before each model request of a turn. Returns text the
/// user typed while the turn was running; the agent appends it as a user
/// message, so it joins the turn rather than replacing it.
pub type SteerHook = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Host callback awaited at every step boundary of a turn, with the whole
/// history (consistent there: each tool call has its result) and the current
/// [`Agent::history_revision`]. Lets the host persist a turn as it goes, so a
/// process that dies mid-turn loses at most the step in flight.
pub type CheckpointHook = std::sync::Arc<
    dyn Fn(&[Message], u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

/// A single agent-callable tool.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Static definition surfaced to the model (name, description, schema).
    fn def(&self) -> crate::message::ToolDef;

    /// Downcast hook for executor-level introspection (the completion-wake
    /// path finds the bash tool's job registry without a registry-wide
    /// redesign). Every tool returns `self`.
    fn as_any(&self) -> &dyn std::any::Any;

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

    /// Bounded wait until at least one background notification is ready (or
    /// `None` when the timeout elapsed with nothing to deliver). The loop
    /// calls this at turn end with unfinished background jobs instead of
    /// ending the run: headless invocations would otherwise exit and kill
    /// every job. Default `None` — executors without background work answer
    /// instantly, so callers cannot hang on it.
    fn wait_for_notification(
        &self,
        _ctx: &ToolContext,
        _timeout: std::time::Duration,
    ) -> BoxFuture<'static, Option<()>> {
        Box::pin(std::future::ready(None))
    }

    /// Whether any background job is still running for this session — the
    /// gate that decides whether turn-end waits ([`wait_for_notification`]).
    fn has_pending_background(&self, _ctx: &ToolContext) -> bool {
        false
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
    /// Shared so a cache-warming task can replay a request while tools run.
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) executor: std::sync::Arc<dyn ToolExecutor>,
    pub(crate) system: String,
    /// Effective prefix captured once per turn, including plugin context.
    pub(crate) turn_system: Option<String>,
    pub(crate) tools: Vec<ToolDef>,
    /// Display-only headlines per tool name, consulted by transcript
    /// renderers (`tool_fmt` reads the entry through injected args).
    /// Never reaches the model: provider mappers project
    /// name/description/parameters only. Empty by default.
    pub(crate) tool_labels: std::collections::HashMap<String, String>,
    /// Display-only arg preview paths per tool name, consulted by the same
    /// renderers (`tool_fmt` reads the entry through injected args).
    /// Same visibility contract as `tool_labels`. Empty by default.
    pub(crate) tool_previews: std::collections::HashMap<String, String>,
    pub(crate) messages: Vec<Message>,
    pub(crate) tool_timeout: Duration,
    pub(crate) hooks: Vec<Arc<dyn PluginHooks>>,
    pub(crate) context_window: Option<usize>,
    /// Pre-turn compaction reserve and retained-tail budget overrides
    /// (`GRAY_CONTEXT_RESERVE` / `GRAY_CONTEXT_KEEP`); `None` = core defaults.
    pub(crate) compact_reserve: Option<usize>,
    pub(crate) compact_keep: Option<usize>,
    /// Latest provider-reported context size and the history length it
    /// covered (pi `getLastAssistantUsage`). Cleared on every history
    /// rewrite: a pre-rewrite report describes a context that no longer exists.
    context_usage: Option<(usize, usize)>,
    history_revision: u64,
    history_rewrite_hook: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Polled before every model request of a turn: `Some(text)` steers the
    /// turn in flight by appending a user message the model reads on its next
    /// request. `None` (no hook, or nothing typed) leaves the turn alone.
    pub(crate) steer: Option<SteerHook>,
    /// See [`CheckpointHook`]. Set per turn by the host.
    pub(crate) checkpoint: Option<CheckpointHook>,
    /// Session this transcript persists to, when known — woven into
    /// compaction citation stubs (arXiv:2607.25066). Captured from each
    /// run's [`ToolContext`].
    pub(crate) session_id: Option<String>,
    /// Indices of salvaged partial-assistant messages left by a mid-stream
    /// error. They stay in `messages` (the persisted transcript must match
    /// what the user saw on screen) but are replaced with a one-line marker
    /// in outbound requests: a failed attempt left in context measurably
    /// contaminates the retry (arXiv:2605.08563 CCRM — cascade ratio
    /// ε1/ε0 ≈ 7.1 on SWE-bench Verified; clean restart dominates).
    /// Cleared by every history rewrite — compaction subsumes the failure.
    pub(crate) contaminated: std::collections::BTreeSet<usize>,
    /// Stale-output mask watermark: every at-threshold `ToolResult` in
    /// `messages[..masked_prefix]` rides outbound requests as a citation
    /// stub. Advanced only when the cache is already cold (see
    /// `agent_loop::mask_stale_tool_output`) and reset by every history
    /// rewrite, which invalidates the index.
    pub(crate) masked_prefix: usize,
    /// Prompt-cache warming during long tool runs; `None` = off.
    pub(crate) cache_warm: Option<crate::cache_warm::CacheWarmPolicy>,
}

/// Lends a shared provider to a `Box` decorator ([`Agent::map_provider`]).
struct SharedProvider(Arc<dyn Provider>);

impl Provider for SharedProvider {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        self.0.stream(req)
    }

    fn model_id(&self) -> &str {
        self.0.model_id()
    }
}

impl Agent {
    /// Creates an agent over the given provider and tool executor.
    pub fn new(provider: Box<dyn Provider>, executor: std::sync::Arc<dyn ToolExecutor>) -> Self {
        Self {
            provider: Arc::from(provider),
            executor,
            system: String::new(),
            turn_system: None,
            tools: Vec::new(),
            tool_labels: std::collections::HashMap::new(),
            tool_previews: std::collections::HashMap::new(),
            messages: Vec::new(),
            tool_timeout: Duration::from_secs(120),
            hooks: Vec::new(),
            context_window: None,
            compact_reserve: None,
            compact_keep: None,
            context_usage: None,
            history_revision: 0,
            history_rewrite_hook: None,
            steer: None,
            checkpoint: None,
            session_id: None,
            contaminated: std::collections::BTreeSet::new(),
            masked_prefix: 0,
            cache_warm: None,
        }
    }

    /// Keep the provider's prompt cache warm while long tools run (pi cache
    /// warming, streaming mode). `None` turns it off.
    pub fn with_cache_warm(mut self, policy: Option<crate::cache_warm::CacheWarmPolicy>) -> Self {
        self.cache_warm = policy;
        self
    }

    /// Decorate all provider requests, including compaction, with host policy.
    pub fn map_provider(
        mut self,
        wrap: impl FnOnce(Box<dyn Provider>) -> Box<dyn Provider>,
    ) -> Self {
        self.provider = Arc::from(wrap(Box::new(SharedProvider(self.provider.clone()))));
        self
    }

    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }

    /// Install the mid-turn steer hook (see [`Agent::steer`]). Set per turn by
    /// the host, which is where the queue lives. The hook runs on the turn's
    /// own task and must not block: the model's next request waits on it.
    pub fn set_steer(&mut self, hook: SteerHook) {
        self.steer = Some(hook);
    }

    /// Install the step-boundary checkpoint hook (see [`CheckpointHook`]).
    pub fn set_checkpoint(&mut self, hook: Option<CheckpointHook>) {
        self.checkpoint = hook;
    }

    pub fn with_history_rewrite_hook(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.history_rewrite_hook = Some(hook);
        self
    }

    pub(crate) fn history_rewritten(&mut self) {
        self.context_usage = None;
        // A rewrite subsumes whatever the contaminated partials were part
        // of; the indices would point at the wrong messages anyway.
        self.contaminated.clear();
        // Indices into the old history are meaningless after a rewrite.
        self.masked_prefix = 0;
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

    /// Display-only headlines per tool name (plugin `label` support).
    /// Renderers inject the entry as `args.label`; the executor and both
    /// providers keep using the wire name, and events stay unchanged.
    pub fn with_tool_labels(
        mut self,
        labels: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.tool_labels = labels
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    /// Display headline for a wire name, if one was registered.
    pub fn tool_label(&self, name: &str) -> Option<&str> {
        self.tool_labels.get(name).map(|s| s.as_str())
    }

    /// Display-only arg preview paths per tool name (plugin `preview`
    /// support). Same merge/render contract as [`Self::with_tool_labels`].
    pub fn with_tool_previews(
        mut self,
        previews: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
    ) -> Self {
        self.tool_previews = previews
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        self
    }

    /// Display arg preview path for a wire name, if one was registered.
    pub fn tool_preview(&self, name: &str) -> Option<&str> {
        self.tool_previews.get(name).map(|s| s.as_str())
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

    /// Overrides the pre-turn compaction reserve and the retained-tail
    /// budget of automatic compaction; `None` keeps the core default.
    pub fn with_compaction_budget(mut self, reserve: Option<usize>, keep: Option<usize>) -> Self {
        self.compact_reserve = reserve;
        self.compact_keep = keep;
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

    /// Idle-host view of background jobs (the REPL between turns): `Some`
    /// wake future while this session still has unfinished jobs — it resolves
    /// `Some(())` once one settles — so the host can start a turn on its own.
    pub fn background_wake(
        &self,
        ctx: &ToolContext,
        timeout: Duration,
    ) -> Option<BoxFuture<'static, Option<()>>> {
        self.executor
            .has_pending_background(ctx)
            .then(|| self.executor.wait_for_notification(ctx, timeout))
    }

    /// Takes finished background-job notices for an idle host, which turns
    /// them into the next turn's input (the loop drains them only mid-run).
    pub fn drain_background_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.executor.drain_notifications(ctx)
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

    /// Tools advertised to the model. `pub` (not `pub(crate)`): headless
    /// and REPL surfaces seed display-only labels from these defs, and the
    /// compaction-v2 trigger call reuses them verbatim.
    pub fn tool_defs(&self) -> &[ToolDef] {
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
            max_tokens: None,
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
            StreamEvent::MessageComplete { usage, .. } => {
                usage.unwrap_or_default().log_request("compaction");
                break;
            }
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
