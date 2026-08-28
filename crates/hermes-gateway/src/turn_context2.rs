//! Per-turn context shared between `GatewayRunner._run_agent_inner` and the
//! `TurnRunner` collaborator (gateway/run.py).
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/turn_context.py` (145 LOC).
//!
//! `_run_agent_inner` historically defined its tool-progress plumbing as nested
//! closures (`progress_callback` ~250 LOC, `send_progress_messages` ~353 LOC)
//! that closed over ~20 enclosing locals. `TurnContext` is the extraction seam:
//! each closed-over local becomes a field on this struct, so the closure bodies
//! can move onto `TurnRunner` methods unchanged modulo `name` -> `ctx.name`
//! rewrites.
//!
//! Field notes (preserved from Python):
//! - All fields are written once by `_run_agent_inner` while wiring up the turn
//!   (a few — `_progress_metadata`, `_progress_reply_to`, `agent_holder` —
//!   are computed slightly later than construction and assigned onto the ctx as
//!   soon as the original locals were bound). None of the original closures
//!   *rebound* their captured names (no `nonlocal`); mutable state uses the
//!   same single-element-list containers as before (`last_progress_msg`,
//!   `repeat_count`, ...), so mutation stays visible to the outer body through
//!   the shared objects exactly as it did through the shared closure cells.
//! - `_run_still_current` stays a callable (it captures `self`/
//!   `session_key`/`run_generation`); carrying the callable keeps the
//!   extracted bodies byte-identical.
//!
//! Python source docstring (preserved):
//! ```text
//! Per-turn context shared between ``GatewayRunner._run_agent_inner`` and the
//! ``TurnRunner`` collaborator (gateway/run.py).
//!
//! ``_run_agent_inner`` historically defined its tool-progress plumbing as nested
//! closures (``progress_callback`` ~250 LOC, ``send_progress_messages`` ~353 LOC)
//! that closed over ~20 enclosing locals.  ``TurnRunner`` is the extraction seam:
//! each closed-over local becomes a field on this dataclass, so the closure bodies
//! can move onto ``TurnRunner`` methods unchanged modulo ``name`` -> ``ctx.name``
//! rewrites.
//! ```

use std::collections::HashSet;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Callback type aliases — mirrors `Callable` fields in Python
// ---------------------------------------------------------------------------

/// Mirrors `_run_still_current: Callable[[], bool]` — captures `self`/
/// `session_key`/`run_generation` and returns whether the turn is still current.
pub type RunStillCurrentCallback = Arc<dyn Fn() -> bool + Send + Sync>;

/// Generic turn callback — mirrors `progress_callback`,
/// `voice_ack_callback`, `_step_callback_sync`, etc. Python types them as
/// `Optional[Callable]` with varying signatures; Rust erases to `serde_json::Value`
/// args for 1:1 fidelity without pinning a concrete signature.
pub type TurnCallback = Arc<dyn Fn(serde_json::Value) -> serde_json::Value + Send + Sync>;

/// Minimal void callback used for start/complete hooks where return is ignored.
pub type VoidCallback = Arc<dyn Fn(serde_json::Value) + Send + Sync>;

// ---------------------------------------------------------------------------
// TurnContext — mirrors `@dataclass class TurnContext`
// ---------------------------------------------------------------------------

/// Closed-over locals of `_run_agent_inner` needed by `TurnRunner`.
///
/// Mirrors `gateway/turn_context.py::TurnContext`. All `Any` fields are
/// `Option<serde_json::Value>` (JSON-native Any), `Callable` fields are
/// `Option<Arc<dyn Fn...>>`. Mutable single-element-list containers are
/// `Vec<T>` with one element (e.g. `Vec<Option<Value>>` for `[None]`), preserving
/// the shared-object mutation semantics of the original closure cells.
#[allow(non_snake_case)]
pub struct TurnContext {
    // --- read-only turn identity / wiring -------------------------------
    pub source: Option<serde_json::Value>,
    pub _run_still_current: Option<RunStillCurrentCallback>,
    pub _live_status_adapter: Option<serde_json::Value>,
    pub _live_status_mode: String,
    pub _thinking_enabled: bool,
    pub progress_mode: String,
    pub progress_grouping: String,
    pub tool_progress_enabled: bool,

    // --- queues ----------------------------------------------------------
    pub progress_queue: Option<serde_json::Value>,
    pub log_queue: Option<serde_json::Value>,

    // --- mutable single-element containers (shared with the outer body) --
    pub last_progress_msg: Vec<Option<serde_json::Value>>,
    pub last_tool: Vec<Option<serde_json::Value>>,
    pub last_was_terminal_block: Vec<bool>,
    pub repeat_count: Vec<i64>,
    pub long_tool_hint_fired: Vec<bool>,
    pub agent_holder: Vec<Option<serde_json::Value>>,

    // --- constants / cleanup bookkeeping ---------------------------------
    pub _LONG_TOOL_THRESHOLD_S: f64,
    pub _cleanup_progress: bool,
    pub _cleanup_msg_ids: Vec<String>,

    // --- progress threading metadata (assigned after construction, before
    //     send_progress_messages is scheduled) ----------------------------
    pub _progress_metadata: Option<serde_json::Value>,
    pub _progress_reply_to: Option<serde_json::Value>,

    // --- the ex-``nonlocal`` turn message (rebindable) --------------------
    pub message: Option<String>,

    // --- turn parameters / config snapshots (read-only in run_sync) -------
    pub history: Option<serde_json::Value>,
    pub context_prompt: Option<String>,
    pub channel_prompt: Option<String>,
    pub session_id: Option<String>,
    pub session_key: Option<String>,
    pub run_generation: Option<i64>,
    pub process_task_id: String,
    pub process_baseline: HashSet<String>,
    pub _interrupt_depth: i64,
    pub event_message_id: Option<String>,
    pub moa_config: Option<serde_json::Value>,
    pub persist_user_message: Option<serde_json::Value>,
    pub persist_user_timestamp: Option<f64>,
    // display_kind stamped on the persisted user row at turn start when this
    // turn was self-injected (MessageEvent.internal), e.g.
    // "internal_notification" for async-delegation/background notifications
    // (#82888). DB-only presentation metadata; never sent to the provider.
    pub persist_user_display_kind: Option<String>,
    pub user_config: Option<serde_json::Value>,
    pub enabled_toolsets: Option<serde_json::Value>,
    pub disabled_toolsets: Option<serde_json::Value>,
    pub log_mode_enabled: bool,
    pub interim_assistant_messages_enabled: bool,
    pub needs_progress_queue: bool,

    // --- lazy-imported callables captured from the outer body -------------
    pub AIAgent: Option<serde_json::Value>,
    pub resolve_display_setting: Option<serde_json::Value>,

    // --- mutable holder cells (shared-list pattern; outer body + the
    //     post-executor closures read mutations through the same objects) --
    pub result_holder: Vec<Option<serde_json::Value>>,
    pub tools_holder: Vec<Option<serde_json::Value>>,
    pub stream_consumer_holder: Vec<Option<serde_json::Value>>,
    pub streaming_tts_consumer_holder: Vec<Option<serde_json::Value>>,

    // --- voice-ack wiring --------------------------------------------------
    pub _voice_ack_fired: Vec<bool>,
    pub _voice_ack_guild: Vec<Option<serde_json::Value>>,
    pub _voice_ack_loop: Option<serde_json::Value>,

    // --- hook / status bridge wiring (published at original binding sites) -
    pub _loop_for_step: Option<serde_json::Value>,
    pub _hooks_ref: Option<serde_json::Value>,
    pub _status_adapter: Option<serde_json::Value>,
    pub _status_chat_id: Option<serde_json::Value>,
    pub _status_thread_metadata: Option<serde_json::Value>,

    // --- extracted sibling callbacks (bound TurnRunner methods; run_sync
    //     reads them through the ctx exactly where it used to close over
    //     the sibling closures) ---------------------------------------------
    pub progress_callback: Option<TurnCallback>,
    pub voice_ack_callback: Option<TurnCallback>,
    pub _step_callback_sync: Option<TurnCallback>,
    pub _event_callback_sync: Option<TurnCallback>,
    pub _status_callback_sync: Option<TurnCallback>,

    // --- Slack-native task-card progress (opt-in; #29483) ------------------
    // True when the Slack adapter's `native_task_cards_enabled()` opt-in is
    // set for this turn's platform. The ID-bearing lifecycle callbacks are
    // published by TurnRunner (like voice_ack_callback above) so tool starts
    // and completions correlate by real tool-call ID instead of tool name.
    pub _native_slack_task_cards: bool,
    pub native_tool_start_callback: Option<TurnCallback>,
    pub native_tool_complete_callback: Option<TurnCallback>,
}

// Manual Debug — trait objects have no Debug, so we render them as
// `Some(<callback>)` / `None` to keep `TurnContext` debuggable.
impl std::fmt::Debug for TurnContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("TurnContext");
        s.field("source", &self.source)
            .field(
                "_run_still_current",
                &self._run_still_current.as_ref().map(|_| "<callback>"),
            )
            .field("_live_status_adapter", &self._live_status_adapter)
            .field("_live_status_mode", &self._live_status_mode)
            .field("_thinking_enabled", &self._thinking_enabled)
            .field("progress_mode", &self.progress_mode)
            .field("progress_grouping", &self.progress_grouping)
            .field("tool_progress_enabled", &self.tool_progress_enabled)
            .field("progress_queue", &self.progress_queue)
            .field("log_queue", &self.log_queue)
            .field("last_progress_msg", &self.last_progress_msg)
            .field("last_tool", &self.last_tool)
            .field("last_was_terminal_block", &self.last_was_terminal_block)
            .field("repeat_count", &self.repeat_count)
            .field("long_tool_hint_fired", &self.long_tool_hint_fired)
            .field("agent_holder", &self.agent_holder)
            .field("_LONG_TOOL_THRESHOLD_S", &self._LONG_TOOL_THRESHOLD_S)
            .field("_cleanup_progress", &self._cleanup_progress)
            .field("_cleanup_msg_ids", &self._cleanup_msg_ids)
            .field("_progress_metadata", &self._progress_metadata)
            .field("_progress_reply_to", &self._progress_reply_to)
            .field("message", &self.message)
            .field("history", &self.history)
            .field("context_prompt", &self.context_prompt)
            .field("channel_prompt", &self.channel_prompt)
            .field("session_id", &self.session_id)
            .field("session_key", &self.session_key)
            .field("run_generation", &self.run_generation)
            .field("process_task_id", &self.process_task_id)
            .field("process_baseline", &self.process_baseline)
            .field("_interrupt_depth", &self._interrupt_depth)
            .field("event_message_id", &self.event_message_id)
            .field("moa_config", &self.moa_config)
            .field("persist_user_message", &self.persist_user_message)
            .field("persist_user_timestamp", &self.persist_user_timestamp)
            .field(
                "persist_user_display_kind",
                &self.persist_user_display_kind,
            )
            .field("user_config", &self.user_config)
            .field("enabled_toolsets", &self.enabled_toolsets)
            .field("disabled_toolsets", &self.disabled_toolsets)
            .field("log_mode_enabled", &self.log_mode_enabled)
            .field(
                "interim_assistant_messages_enabled",
                &self.interim_assistant_messages_enabled,
            )
            .field("needs_progress_queue", &self.needs_progress_queue)
            .field("AIAgent", &self.AIAgent)
            .field("resolve_display_setting", &self.resolve_display_setting)
            .field("result_holder", &self.result_holder)
            .field("tools_holder", &self.tools_holder)
            .field("stream_consumer_holder", &self.stream_consumer_holder)
            .field(
                "streaming_tts_consumer_holder",
                &self.streaming_tts_consumer_holder,
            )
            .field("_voice_ack_fired", &self._voice_ack_fired)
            .field("_voice_ack_guild", &self._voice_ack_guild)
            .field("_voice_ack_loop", &self._voice_ack_loop)
            .field("_loop_for_step", &self._loop_for_step)
            .field("_hooks_ref", &self._hooks_ref)
            .field("_status_adapter", &self._status_adapter)
            .field("_status_chat_id", &self._status_chat_id)
            .field("_status_thread_metadata", &self._status_thread_metadata)
            .field(
                "progress_callback",
                &self.progress_callback.as_ref().map(|_| "<callback>"),
            )
            .field(
                "voice_ack_callback",
                &self.voice_ack_callback.as_ref().map(|_| "<callback>"),
            )
            .field(
                "_step_callback_sync",
                &self._step_callback_sync.as_ref().map(|_| "<callback>"),
            )
            .field(
                "_event_callback_sync",
                &self._event_callback_sync.as_ref().map(|_| "<callback>"),
            )
            .field(
                "_status_callback_sync",
                &self._status_callback_sync.as_ref().map(|_| "<callback>"),
            )
            .field("_native_slack_task_cards", &self._native_slack_task_cards)
            .field(
                "native_tool_start_callback",
                &self
                    .native_tool_start_callback
                    .as_ref()
                    .map(|_| "<callback>"),
            )
            .field(
                "native_tool_complete_callback",
                &self
                    .native_tool_complete_callback
                    .as_ref()
                    .map(|_| "<callback>"),
            )
            .finish()
    }
}

impl Default for TurnContext {
    fn default() -> Self {
        Self {
            source: None,
            _run_still_current: None,
            _live_status_adapter: None,
            _live_status_mode: "off".to_string(),
            _thinking_enabled: false,
            progress_mode: "off".to_string(),
            progress_grouping: "grouped".to_string(),
            tool_progress_enabled: false,
            progress_queue: None,
            log_queue: None,
            last_progress_msg: vec![None],
            last_tool: vec![None],
            last_was_terminal_block: vec![false],
            repeat_count: vec![0],
            long_tool_hint_fired: vec![false],
            agent_holder: vec![None],
            _LONG_TOOL_THRESHOLD_S: 30.0,
            _cleanup_progress: false,
            _cleanup_msg_ids: Vec::new(),
            _progress_metadata: None,
            _progress_reply_to: None,
            message: None,
            history: None,
            context_prompt: None,
            channel_prompt: None,
            session_id: None,
            session_key: None,
            run_generation: None,
            process_task_id: String::new(),
            process_baseline: HashSet::new(),
            _interrupt_depth: 0,
            event_message_id: None,
            moa_config: None,
            persist_user_message: None,
            persist_user_timestamp: None,
            persist_user_display_kind: None,
            user_config: None,
            enabled_toolsets: None,
            disabled_toolsets: None,
            log_mode_enabled: false,
            interim_assistant_messages_enabled: false,
            needs_progress_queue: false,
            AIAgent: None,
            resolve_display_setting: None,
            result_holder: vec![None],
            tools_holder: vec![None],
            stream_consumer_holder: vec![None],
            streaming_tts_consumer_holder: vec![None],
            _voice_ack_fired: vec![false],
            _voice_ack_guild: vec![None],
            _voice_ack_loop: None,
            _loop_for_step: None,
            _hooks_ref: None,
            _status_adapter: None,
            _status_chat_id: None,
            _status_thread_metadata: None,
            progress_callback: None,
            voice_ack_callback: None,
            _step_callback_sync: None,
            _event_callback_sync: None,
            _status_callback_sync: None,
            _native_slack_task_cards: false,
            native_tool_start_callback: None,
            native_tool_complete_callback: None,
        }
    }
}

impl TurnContext {
    /// Create a new `TurnContext` with Python-equivalent defaults.
    /// Mirrors `TurnContext()` dataclass construction.
    pub fn new() -> Self {
        Self::default()
    }
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _default_long_tool_threshold() -> f64 {
    30.0
}
