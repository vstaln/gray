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

    #[error("{class}")]
    Detailed {
        class: Box<ProviderError>,
        detail: Box<crate::error::ProviderDetail>,
    },
}

impl ProviderError {
    pub fn class(&self) -> &ProviderError {
        let mut cur = self;
        while let Self::Detailed { class, .. } = cur {
            cur = class;
        }
        cur
    }

    pub fn detail(&self) -> Option<&crate::error::ProviderDetail> {
        if let Self::Detailed { detail, .. } = self {
            Some(detail.as_ref())
        } else {
            None
        }
    }

    pub fn should_compress(&self) -> bool {
        matches!(self.class(), Self::ContextOverflow(_))
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self.class(),
            Self::RateLimited(_)
                | Self::ServerError(_)
                | Self::Stream(_)
                | Self::Connection(_)
                | Self::Timeout(_)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub content: String,
    pub is_error: bool,

    #[serde(default)]
    pub images: Vec<AttachedImage>,

    #[serde(default, alias = "videos")]
    pub media: Vec<AttachedMedia>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Box<Usage>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedImage {
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttachedMedia {
    pub media_type: String,
    pub data: String,

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
            usage: None,
        }
    }
    pub fn error(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            is_error: true,
            images: Vec::new(),
            media: Vec::new(),
            usage: None,
        }
    }

    pub fn image(content: impl Into<String>, media_type: String, data: String) -> Self {
        Self {
            content: content.into(),
            is_error: false,
            images: vec![AttachedImage { media_type, data }],
            media: Vec::new(),
            usage: None,
        }
    }

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

#[async_trait]
pub trait Provider: Send + Sync {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>>;

    fn model_id(&self) -> &str {
        ""
    }

    fn warm_output_cap(&self, _req: &ChatRequest) -> u32 {
        1
    }
}

pub type SteerHook = std::sync::Arc<dyn Fn() -> Option<String> + Send + Sync>;

pub type CheckpointHook = std::sync::Arc<
    dyn Fn(&[Message], u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        + Send
        + Sync,
>;

pub type ToolOutputHook = Arc<dyn Fn(&str, &[u8]) + Send + Sync>;

#[derive(Clone)]
pub struct ToolOutputSink {
    id: String,
    hook: ToolOutputHook,
}

impl ToolOutputSink {
    pub fn emit(&self, bytes: &[u8]) {
        (self.hook)(&self.id, bytes);
    }
}

tokio::task_local! {
    static TOOL_OUTPUT: ToolOutputSink;
}

pub fn current_tool_output() -> Option<ToolOutputSink> {
    TOOL_OUTPUT.try_with(Clone::clone).ok()
}

pub(crate) async fn scope_tool_output<T>(
    sink: ToolOutputSink,
    fut: impl std::future::Future<Output = T>,
) -> T {
    TOOL_OUTPUT.scope(sink, fut).await
}

#[derive(Clone, Default)]
pub struct IsolatedStream {
    pub session_id: Option<String>,
    pub cache_off: bool,
}

tokio::task_local! {
    static ISOLATED_STREAM: IsolatedStream;
}

pub fn current_isolated_stream() -> Option<IsolatedStream> {
    ISOLATED_STREAM.try_with(Clone::clone).ok()
}

pub(crate) async fn scope_isolated_stream<T>(
    iso: IsolatedStream,
    fut: impl std::future::Future<Output = T>,
) -> T {
    ISOLATED_STREAM.scope(iso, fut).await
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> crate::message::ToolDef;

    fn as_any(&self) -> &dyn std::any::Any;

    fn drain_notifications(&self, _ctx: &ToolContext) -> Vec<String> {
        Vec::new()
    }

    async fn execute(&self, ctx: &ToolContext, args: serde_json::Value) -> ToolOutput;
}

#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn drain_notifications(&self, _ctx: &ToolContext) -> Vec<String> {
        Vec::new()
    }

    fn live_defs(&self) -> Option<Vec<crate::message::ToolDef>> {
        None
    }

    fn wait_for_notification(
        &self,
        _ctx: &ToolContext,
        _timeout: std::time::Duration,
    ) -> BoxFuture<'static, Option<()>> {
        Box::pin(std::future::ready(None))
    }

    fn has_pending_background(&self, _ctx: &ToolContext) -> bool {
        false
    }

    fn repin_session(&self, _id: &str) {}

    fn background_jobs(&self, _ctx: &ToolContext) -> Vec<BackgroundJob> {
        Vec::new()
    }

    fn running_jobs_note(&self, _ctx: &ToolContext) -> Option<String> {
        None
    }

    fn cancel_background(&self, _ctx: &ToolContext, _id: &str) -> bool {
        false
    }

    fn execute(
        &self,
        ctx: &ToolContext,
        name: &str,
        args: serde_json::Value,
    ) -> BoxFuture<'static, ToolOutput>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundJob {
    pub id: String,
    pub elapsed: Duration,

    pub stopping: bool,
}

pub type ProviderStream = BoxStream<'static, Result<StreamEvent, ProviderError>>;

#[derive(Debug, Clone, PartialEq)]
pub enum ToolBefore {
    Allow,

    Modify(serde_json::Value),

    Deny(String),
}

impl ToolBefore {
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

            Some("allow") => Self::Allow,
            _ => Self::Deny("plugin returned an unrecognized policy verdict".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum InputSubmit {
    Pass,

    Rewrite(String),

    Handled,
}

impl InputSubmit {
    pub fn from_result(v: &serde_json::Value) -> Self {
        if v.get("handled").and_then(|h| h.as_bool()) == Some(true) {
            return Self::Handled;
        }
        if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
            return Self::Rewrite(text.to_string());
        }
        Self::Pass
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCommand {
    pub name: String,
    pub description: String,
    /// First-argv verbs the plugin allows to run while a turn is in
    /// flight (manifest `turn_local`); `""` covers the bare command and
    /// `"*"` allows any argv.
    pub turn_local_verbs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelStyle {
    #[serde(default)]
    pub fg: Option<String>,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    #[serde(default)]
    pub dim: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSpan {
    pub text: String,
    #[serde(default)]
    pub style: Option<PanelStyle>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelPage {
    pub tab: String,
    pub title: String,
    pub lines: Vec<Vec<PanelSpan>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelSpec {
    pub title: String,
    pub pages: Vec<PanelPage>,
    #[serde(default)]
    pub refreshable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerItem {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickerSpec {
    pub title: String,
    pub items: Vec<PickerItem>,
    #[serde(default)]
    pub selected_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Say(String),
    Prompt(String),

    ModelPicker(String),

    AgentPicker(String),

    SubagentSettings(String),

    Panel(PanelSpec),

    Picker(PickerSpec),

    Terminal {
        argv: Vec<String>,
        text: Option<String>,
    },
}

#[async_trait]
pub trait PluginHooks: Send + Sync {
    async fn prompt_context(&self) -> Option<String> {
        None
    }

    async fn tool_before(&self, _name: &str, _args: &serde_json::Value) -> ToolBefore {
        ToolBefore::Allow
    }

    fn commands(&self) -> Vec<PluginCommand> {
        Vec::new()
    }

    async fn run_command(&self, _name: &str, _argv: Vec<String>) -> Option<CommandOutcome> {
        None
    }

    async fn input_submit(&self, _text: &str) -> InputSubmit {
        InputSubmit::Pass
    }

    async fn agent_before_start(&self) -> Option<String> {
        None
    }

    async fn context_build(&self, _messages: &[Message]) -> Option<Vec<Message>> {
        None
    }

    async fn tool_after(&self, _name: &str, _output: &ToolOutput) -> Option<ToolOutput> {
        None
    }
    async fn pre_tool(&self, _name: &str, _args: &serde_json::Value) {}
    async fn post_tool(&self, _name: &str, _output: &ToolOutput) {}
    async fn turn_end(&self, _usage: &Usage) {}

    async fn shutdown(&self) {}

    fn is_global(&self) -> bool {
        false
    }
}

impl From<ProviderError> for CoreError {
    fn from(e: ProviderError) -> Self {
        match e {
            ProviderError::RateLimited(msg) => CoreError::RateLimited(msg),
            ProviderError::Auth(msg) => CoreError::Auth(msg),
            ProviderError::BadRequest(msg) => CoreError::BadRequest(msg),
            ProviderError::ContextOverflow(msg) => CoreError::ContextOverflow(msg),
            ProviderError::Connection(msg) => CoreError::Connection(msg),
            ProviderError::Timeout(msg) => CoreError::Timeout(msg),
            ProviderError::ServerError(msg) => CoreError::ServerError(msg),
            ProviderError::Stream(msg) => CoreError::Stream(msg),
            ProviderError::Detailed { class, detail } => CoreError::Detailed {
                class: Box::new((*class).into()),
                detail,
            },
        }
    }
}

use futures::StreamExt as _;

use crate::message::{ContentBlock, Message, Role, ToolDef};

pub use super::agent_compact::{estimate_message_tokens, summary_message};

pub struct Agent {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) executor: std::sync::Arc<dyn ToolExecutor>,
    pub(crate) system: String,

    pub(crate) turn_system: Option<String>,
    pub(crate) tools: Vec<ToolDef>,

    pub(crate) tool_labels: std::collections::HashMap<String, String>,

    pub(crate) tool_previews: std::collections::HashMap<String, String>,
    pub(crate) messages: Vec<Message>,
    pub(crate) tool_timeout: Duration,
    pub(crate) hooks: Vec<Arc<dyn PluginHooks>>,
    pub(crate) context_window: Option<usize>,

    pub(crate) compact_reserve: Option<usize>,
    pub(crate) compact_keep: Option<usize>,

    pub(crate) price_cliff: Option<usize>,
    pub(crate) compact_ceiling: Option<usize>,

    pub(crate) soft_elide_rearm: usize,

    pub(crate) context_usage: Option<(usize, usize)>,
    history_revision: u64,
    history_rewrite_hook: Option<Arc<dyn Fn() + Send + Sync>>,

    pub(crate) steer: Option<SteerHook>,

    pub(crate) checkpoint: Option<CheckpointHook>,
    pub(crate) tool_output_hook: Option<ToolOutputHook>,

    pub(crate) session_id: Option<String>,

    pub(crate) contaminated: std::collections::BTreeSet<usize>,

    pub(crate) masked_prefix: usize,

    pub(crate) cache_warm: Option<crate::cache_warm::CacheWarmPolicy>,

    pub(crate) warm_run: Option<crate::cache_warm::ActiveWarm>,

    pub(crate) warm_hint: std::sync::Arc<std::sync::Mutex<crate::cache_warm::WarmHint>>,

    pub(crate) prefix_guard: crate::prefix_guard::PrefixGuard,

    pub(crate) warm_cache_reported: bool,

    pub(crate) cache_stats: crate::cache_stats::CacheStats,

    pub(crate) prefix_rewrite_ok: bool,

    pub(crate) lean_prompt: bool,
    pub(crate) must_act: bool,
    pub(crate) bench_nudges: bool,
    pub(crate) bench_thrash_marks: Vec<u32>,
}

struct SharedProvider(Arc<dyn Provider>);

impl Provider for SharedProvider {
    fn stream(&self, req: ChatRequest) -> BoxStream<'static, Result<StreamEvent, ProviderError>> {
        self.0.stream(req)
    }

    fn model_id(&self) -> &str {
        self.0.model_id()
    }

    fn warm_output_cap(&self, req: &ChatRequest) -> u32 {
        self.0.warm_output_cap(req)
    }
}

impl Agent {
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
            price_cliff: None,
            compact_ceiling: None,
            soft_elide_rearm: 0,
            context_usage: None,
            history_revision: 0,
            history_rewrite_hook: None,
            steer: None,
            checkpoint: None,
            tool_output_hook: None,
            session_id: None,
            contaminated: std::collections::BTreeSet::new(),
            masked_prefix: 0,
            cache_warm: None,
            warm_run: None,
            warm_hint: std::sync::Arc::new(std::sync::Mutex::new(
                crate::cache_warm::WarmHint::default(),
            )),
            prefix_guard: crate::prefix_guard::PrefixGuard::default(),
            warm_cache_reported: false,
            cache_stats: crate::cache_stats::CacheStats::default(),
            prefix_rewrite_ok: true,
            lean_prompt: false,
            must_act: false,
            bench_nudges: false,
            bench_thrash_marks: vec![60, 120],
        }
    }

    pub fn with_cache_warm(mut self, policy: Option<crate::cache_warm::CacheWarmPolicy>) -> Self {
        self.cache_warm = policy;
        self
    }

    pub fn map_provider(
        mut self,
        wrap: impl FnOnce(Box<dyn Provider>) -> Box<dyn Provider>,
    ) -> Self {
        self.provider = Arc::from(wrap(Box::new(SharedProvider(self.provider.clone()))));
        self
    }

    pub fn provider_handle(&self) -> Arc<dyn Provider> {
        self.provider.clone()
    }

    pub fn history_revision(&self) -> u64 {
        self.history_revision
    }

    pub fn prefix_rewrite_ok(&self) -> bool {
        self.prefix_rewrite_ok
    }

    pub fn set_prefix_rewrite_ok(&mut self, ok: bool) {
        self.prefix_rewrite_ok = ok;
    }

    pub fn set_lean_prompt(&mut self, lean: bool) {
        self.lean_prompt = lean;
    }

    pub fn set_must_act(&mut self, must: bool) {
        self.must_act = must;
    }

    pub fn set_bench_nudges(&mut self, on: bool) {
        self.bench_nudges = on;
    }

    pub fn set_bench_thrash_marks(&mut self, marks: Vec<u32>) {
        if !marks.is_empty() {
            self.bench_thrash_marks = marks;
        }
    }

    pub fn set_steer(&mut self, hook: SteerHook) {
        self.steer = Some(hook);
    }

    pub fn set_checkpoint(&mut self, hook: Option<CheckpointHook>) {
        self.checkpoint = hook;
    }

    pub fn set_tool_output_hook(&mut self, hook: Option<ToolOutputHook>) {
        self.tool_output_hook = hook;
    }

    pub(crate) fn tool_output_sink(&self, call_id: &str) -> Option<ToolOutputSink> {
        self.tool_output_hook.as_ref().map(|hook| ToolOutputSink {
            id: call_id.to_owned(),
            hook: hook.clone(),
        })
    }

    pub fn with_history_rewrite_hook(mut self, hook: Arc<dyn Fn() + Send + Sync>) -> Self {
        self.history_rewrite_hook = Some(hook);
        self
    }

    pub(crate) fn history_rewritten(&mut self) {
        self.context_usage = None;

        self.soft_elide_rearm = 0;

        self.contaminated.clear();

        self.masked_prefix = 0;
        self.prefix_guard.expect_change("history rewrite");
        self.history_revision = self.history_revision.wrapping_add(1);
        self.warm_run = None;
        if let Some(hook) = &self.history_rewrite_hook {
            hook();
        }
    }

    // Blocks replaced in place, indices unchanged: unlike `history_rewritten`
    // the stale mask and scrub set stay valid, and the revision is left alone
    // so the REPL doesn't append the whole history to the session file again.
    // The hook still runs: dedup must not answer a re-read of elided content.
    pub(crate) fn history_edited_in_place(&mut self, reason: &'static str) {
        self.context_usage = None;
        self.prefix_guard.expect_change(reason);
        self.warm_run = None;
        if let Some(hook) = &self.history_rewrite_hook {
            hook();
        }
    }

    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = system.into();
        self.turn_system = None;
        self
    }

    pub fn with_tools(mut self, tools: Vec<ToolDef>) -> Self {
        self.tools = tools;
        self
    }

    pub fn refresh_tools(&mut self) {
        if let Some(defs) = self.executor.live_defs() {
            self.tools = defs;
        }
    }

    pub fn repin_session_id(&self, id: &str) {
        self.executor.repin_session(id);
    }

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

    pub fn with_hooks(mut self, hooks: Vec<Arc<dyn PluginHooks>>) -> Self {
        self.hooks = hooks;
        self.turn_system = None;
        self
    }

    pub fn hooks(&self) -> &[Arc<dyn PluginHooks>] {
        &self.hooks
    }

    pub fn with_tool_surface_of(mut self, donor: Agent) -> Self {
        self.executor = donor.executor;
        self.tools = donor.tools;
        self.tool_labels = donor.tool_labels;
        self.tool_previews = donor.tool_previews;
        self.hooks = donor.hooks;
        self.history_rewrite_hook = donor.history_rewrite_hook;
        self
    }

    pub fn with_messages(mut self, messages: Vec<Message>) -> Self {
        self.messages = messages;
        self.history_rewritten();
        self
    }

    pub fn with_tool_timeout(mut self, timeout: Duration) -> Self {
        self.tool_timeout = timeout;
        self
    }

    pub fn with_context_window(mut self, window: Option<usize>) -> Self {
        self.context_window = window;
        self
    }

    pub fn with_compaction_budget(mut self, reserve: Option<usize>, keep: Option<usize>) -> Self {
        self.compact_reserve = reserve;
        self.compact_keep = keep;
        self
    }

    pub fn with_compact_ceiling(
        mut self,
        ceiling: Option<usize>,
        price_cliff: Option<usize>,
    ) -> Self {
        self.compact_ceiling = ceiling;
        self.price_cliff = if ceiling.is_some() { price_cliff } else { None };
        self
    }

    pub fn price_cliff(&self) -> Option<usize> {
        self.price_cliff
    }

    pub fn compact_ceiling(&self) -> Option<usize> {
        self.compact_ceiling
    }

    pub(crate) fn estimate_tokens(&self) -> usize {
        match self.context_usage {
            Some((tokens, covered)) if covered <= self.messages.len() => {
                tokens + crate::agent_compact::est_tokens(&self.messages[covered..])
            }
            _ => crate::agent_compact::est_tokens(&self.messages),
        }
    }

    pub(crate) fn record_context_usage(&mut self, usage: &crate::event::Usage) {
        let tokens = usage.total();
        if tokens > 0 {
            self.context_usage = Some((tokens, self.messages.len()));
        }
    }

    pub fn executor_handle(&self) -> std::sync::Arc<dyn ToolExecutor> {
        std::sync::Arc::clone(&self.executor)
    }

    pub fn background_jobs(&self, ctx: &ToolContext) -> Vec<BackgroundJob> {
        self.executor.background_jobs(ctx)
    }

    pub fn cancel_background(&self, ctx: &ToolContext, id: &str) -> bool {
        self.executor.cancel_background(ctx, id)
    }

    pub fn drain_background_notifications(&self, ctx: &ToolContext) -> Vec<String> {
        self.executor.drain_notifications(ctx)
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        self.cache_stats.reset();
        self.history_rewritten();
    }

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
                injected: false,
                content,
            });
        }
    }

    pub(crate) fn system_text(&self) -> &str {
        self.turn_system.as_deref().unwrap_or(&self.system)
    }

    pub fn tool_defs(&self) -> &[ToolDef] {
        &self.tools
    }

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
        drain_reply_text(self.provider.stream(req), self.session_id.as_deref()).await
    }

    pub(crate) async fn complete_isolated(
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
        let iso = IsolatedStream {
            session_id: Some(uuid::Uuid::new_v4().to_string()),
            cache_off: true,
        };
        let session = self.session_id.clone();
        scope_isolated_stream(iso, async {
            drain_reply_text(self.provider.stream(req), session.as_deref()).await
        })
        .await
    }
}

async fn drain_reply_text(
    mut stream: ProviderStream,
    session: Option<&str>,
) -> Result<String, CoreError> {
    let mut result = String::new();
    while let Some(event) = stream.next().await {
        match event? {
            StreamEvent::TextDelta { delta } => {
                result.push_str(&delta);
            }
            StreamEvent::MessageComplete { usage, .. } => {
                usage.unwrap_or_default().log_request("compaction", session);
                break;
            }
            _ => {}
        }
    }
    Ok(result)
}

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

pub(crate) fn salvage_partial_text(
    messages: &mut Vec<Message>,
    thinking: String,
    text: String,
    pending_reasoning: &Option<(String, String)>,
    model: &str,
) {
    let mut content = Vec::new();
    if !thinking.is_empty() || pending_reasoning.is_some() {
        content.push(thinking_block(thinking, pending_reasoning, model));
    }
    content.push(ContentBlock::Text { text });
    messages.push(Message {
        role: Role::Assistant,
        injected: false,
        content,
    });
}

#[path = "agent_tests.rs"]
#[cfg(test)]
mod agent_tests;

#[path = "agent_repair_tests.rs"]
#[cfg(test)]
mod repair_tests;

#[path = "agent_surface_tests.rs"]
#[cfg(test)]
mod surface_tests;
