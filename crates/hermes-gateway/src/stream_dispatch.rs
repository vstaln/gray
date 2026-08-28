//! Adapter-driven dispatch of structured stream events to a delivery sink.
//!
//! 1:1 Rust port of `reference/NousResearch/hermes-agent/gateway/stream_dispatch.py` (132 LOC).
//!
//! `GatewayEventDispatcher` is the seam Tobi asked for: the agent emits typed
//! events (`gateway/stream_events.py`), and the *adapter* decides how each one is
//! delivered. The dispatcher holds an adapter + the stream consumer (sink) + the
//! resolved per-channel presentation settings (tool-progress mode, preview length)
//! and routes each event through the adapter's render hooks.
//!
//! Message/commentary/segment events flow into the consumer (native draft on
//! Telegram DMs, edit-in-place elsewhere). Tool events are formatted by the
//! adapter — which may return None to *eat* the event on platforms that can't
//! render tool chrome — and the rendered line is enqueued onto the same tool
//! progress queue the gateway already drains, so the two no longer race through
//! independent code paths.
//!
//! This module deliberately has no platform knowledge and no asyncio: it is a thin
//! synchronous router callable from the agent's worker thread, exactly like the
//! callbacks it replaces.
//!
//! Python source docstring (preserved):
//! ```text
//! Adapter-driven dispatch of structured stream events to a delivery sink.
//!
//! ``GatewayEventDispatcher`` is the seam Tobi asked for: the agent emits typed
//! events (gateway/stream_events.py), and the *adapter* decides how each one is
//! delivered.  The dispatcher holds an adapter + the stream consumer (sink) + the
//! resolved per-channel presentation settings (tool-progress mode, preview length)
//! and routes each event through the adapter's render hooks.
//!
//! Message/commentary/segment events flow into the consumer (native draft on
//! Telegram DMs, edit-in-place elsewhere).  Tool events are formatted by the
//! adapter — which may return None to *eat* the event on platforms that can't
//! render tool chrome — and the rendered line is enqueued onto the same tool
//! progress queue the gateway already drains, so the two no longer race through
//! independent code paths.
//!
//! This module deliberately has no platform knowledge and no asyncio: it is a thin
//! synchronous router callable from the agent's worker thread, exactly like the
//! callbacks it replaces.
//! ```

use std::collections::HashMap;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Stream event types — mirrors gateway/stream_events.py
// ---------------------------------------------------------------------------

/// A delta of streamed assistant text.
/// Mirrors `gateway/stream_events.py::MessageChunk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageChunk {
    pub text: String,
}

impl MessageChunk {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// The current assistant message segment is complete.
/// Mirrors `gateway/stream_events.py::MessageStop`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageStop {
    pub final_stop: bool,
}

impl MessageStop {
    pub fn new(final_stop: bool) -> Self {
        Self { final_stop }
    }
}

impl Default for MessageStop {
    fn default() -> Self {
        Self { final_stop: false }
    }
}

/// A complete interim assistant message emitted between tool iterations.
/// Mirrors `gateway/stream_events.py::Commentary`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commentary {
    pub text: String,
}

impl Commentary {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// A tool invocation has started (or its in-progress state changed).
/// Mirrors `gateway/stream_events.py::ToolCallChunk`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallChunk {
    pub tool_name: String,
    pub preview: Option<String>,
    pub args: Option<serde_json::Value>,
    pub index: usize,
}

impl ToolCallChunk {
    pub fn new(
        tool_name: impl Into<String>,
        preview: Option<String>,
        args: Option<serde_json::Value>,
        index: usize,
    ) -> Self {
        Self {
            tool_name: tool_name.into(),
            preview,
            args,
            index,
        }
    }
}

/// A tool invocation completed.
/// Mirrors `gateway/stream_events.py::ToolCallFinished`.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCallFinished {
    pub tool_name: String,
    pub duration: f64,
    pub ok: bool,
    pub index: usize,
}

impl ToolCallFinished {
    pub fn new(tool_name: impl Into<String>, duration: f64, ok: bool, index: usize) -> Self {
        Self {
            tool_name: tool_name.into(),
            duration,
            ok,
            index,
        }
    }
}

impl Default for ToolCallFinished {
    fn default() -> Self {
        Self {
            tool_name: String::new(),
            duration: 0.0,
            ok: true,
            index: 0,
        }
    }
}

/// One-shot onboarding nudge when a tool runs longer than the threshold.
/// Mirrors `gateway/stream_events.py::LongToolHint`.
#[derive(Debug, Clone, PartialEq)]
pub struct LongToolHint {
    pub tool_name: String,
    pub duration: f64,
}

impl LongToolHint {
    pub fn new(tool_name: impl Into<String>, duration: f64) -> Self {
        Self {
            tool_name: tool_name.into(),
            duration,
        }
    }
}

impl Default for LongToolHint {
    fn default() -> Self {
        Self {
            tool_name: String::new(),
            duration: 0.0,
        }
    }
}

/// A gateway-originated control message (restart, online, long-run notice).
/// Mirrors `gateway/stream_events.py::GatewayNotice`.
#[derive(Debug, Clone, PartialEq)]
pub struct GatewayNotice {
    pub kind: String,
    pub text: String,
    pub extra: HashMap<String, serde_json::Value>,
}

impl GatewayNotice {
    pub fn new(
        kind: impl Into<String>,
        text: impl Into<String>,
        extra: HashMap<String, serde_json::Value>,
    ) -> Self {
        Self {
            kind: kind.into(),
            text: text.into(),
            extra,
        }
    }
}

/// Union of every event the consumer's dispatcher accepts.
/// Mirrors `gateway/stream_events.py::StreamEvent`.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    MessageChunk(MessageChunk),
    MessageStop(MessageStop),
    Commentary(Commentary),
    ToolCallChunk(ToolCallChunk),
    ToolCallFinished(ToolCallFinished),
    LongToolHint(LongToolHint),
    GatewayNotice(GatewayNotice),
}

// ---------------------------------------------------------------------------
// Adapter / sink traits — mirrors `Any` duck-typed adapter + sink in Python
// ---------------------------------------------------------------------------

/// Delivery sink for assistant-text events.
/// Mirrors `GatewayStreamConsumer` passed as `sink` in Python.
pub trait GatewaySink: Send + Sync + std::fmt::Debug {}

/// Platform adapter that decides how each event is rendered.
/// Mirrors `BasePlatformAdapter` duck interface used by the dispatcher:
/// `render_message_event` and `format_tool_event`.
pub trait GatewayAdapter: Send + Sync {
    /// Render a message/commentary/stop event into the sink.
    /// Mirrors `adapter.render_message_event(event, self.sink)`.
    fn render_message_event(&self, event: &StreamEvent, sink: &dyn GatewaySink);

    /// Format a tool event for the progress queue.
    /// Returns `None` to eat the event (platform can't render tool chrome).
    /// Mirrors `adapter.format_tool_event(event, mode=..., preview_max_len=...)`.
    fn format_tool_event(
        &self,
        event: &ToolCallChunk,
        mode: &str,
        preview_max_len: usize,
    ) -> Option<String>;
}

// ---------------------------------------------------------------------------
// Callback type aliases — mirrors Optional[Callable] fields in Python
// ---------------------------------------------------------------------------

/// Callback that places a rendered tool-progress line onto the gateway's progress queue.
pub type EnqueueToolLineCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Hook for LongToolHint events.
pub type LongToolCallback = Arc<dyn Fn(LongToolHint) + Send + Sync>;

/// Hook for GatewayNotice events.
pub type NoticeCallback = Arc<dyn Fn(GatewayNotice) + Send + Sync>;

// ---------------------------------------------------------------------------
// GatewayEventDispatcher — mirrors `class GatewayEventDispatcher`
// ---------------------------------------------------------------------------

/// Route typed stream events through an adapter onto a delivery sink.
///
/// Mirrors `gateway/stream_dispatch.py::GatewayEventDispatcher`.
pub struct GatewayEventDispatcher {
    pub adapter: Arc<dyn GatewayAdapter>,
    pub sink: Option<Arc<dyn GatewaySink>>,
    enqueue_tool_line: Option<EnqueueToolLineCallback>,
    pub tool_mode: String,
    pub preview_max_len: usize,
    on_long_tool: Option<LongToolCallback>,
    on_notice: Option<NoticeCallback>,
    last_tool: Option<String>,
}

impl std::fmt::Debug for GatewayEventDispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayEventDispatcher")
            .field("adapter", &"<adapter>")
            .field("sink", &self.sink.as_ref().map(|_| "<sink>").unwrap_or("None"))
            .field(
                "enqueue_tool_line",
                &self.enqueue_tool_line.as_ref().map(|_| "<callback>"),
            )
            .field("tool_mode", &self.tool_mode)
            .field("preview_max_len", &self.preview_max_len)
            .field(
                "on_long_tool",
                &self.on_long_tool.as_ref().map(|_| "<callback>"),
            )
            .field("on_notice", &self.on_notice.as_ref().map(|_| "<callback>"))
            .field("last_tool", &self.last_tool)
            .finish()
    }
}

impl GatewayEventDispatcher {
    /// Create a new dispatcher.
    ///
    /// Mirrors `GatewayEventDispatcher.__init__`:
    /// `adapter`, `sink=None`, `enqueue_tool_line=None`, `tool_mode="all"`,
    /// `preview_max_len=40`, `on_long_tool=None`, `on_notice=None`.
    pub fn new(
        adapter: Arc<dyn GatewayAdapter>,
        sink: Option<Arc<dyn GatewaySink>>,
        enqueue_tool_line: Option<EnqueueToolLineCallback>,
        tool_mode: impl Into<String>,
        preview_max_len: usize,
        on_long_tool: Option<LongToolCallback>,
        on_notice: Option<NoticeCallback>,
    ) -> Self {
        let mut mode = tool_mode.into();
        if mode.is_empty() {
            mode = "all".to_string();
        }
        Self {
            adapter,
            sink,
            enqueue_tool_line: enqueue_tool_line,
            tool_mode: mode,
            preview_max_len,
            on_long_tool,
            on_notice,
            last_tool: None,
        }
    }

    /// Convenience constructor with defaults (`tool_mode="all"`, `preview_max_len=40`).
    /// Mirrors Python defaults.
    pub fn with_adapter(adapter: Arc<dyn GatewayAdapter>) -> Self {
        Self::new(adapter, None, None, "all", 40, None, None)
    }

    /// Route a single event. Never raises into the agent's worker thread.
    /// Mirrors `GatewayEventDispatcher.dispatch`.
    pub fn dispatch(&mut self, event: StreamEvent) {
        // `presentation must never break the agent loop` — catch panics and log.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self._dispatch(event);
        }));
        if let Err(_) = result {
            log::debug!("stream-event dispatch error");
        }
    }

    /// Internal dispatch without the panic guard.
    /// Mirrors `GatewayEventDispatcher._dispatch`.
    fn _dispatch(&mut self, event: StreamEvent) {
        match &event {
            StreamEvent::MessageChunk(_) | StreamEvent::MessageStop(_) | StreamEvent::Commentary(_) => {
                if let Some(sink) = &self.sink {
                    self.adapter.render_message_event(&event, sink.as_ref());
                }
                return;
            }
            _ => {}
        }

        if let StreamEvent::ToolCallChunk(chunk) = &event {
            if self.tool_mode == "off" || self.enqueue_tool_line.is_none() {
                return;
            }
            // "new" mode: only emit when the tool changes.
            if self.tool_mode == "new" {
                if let Some(last) = &self.last_tool {
                    if last == &chunk.tool_name {
                        return;
                    }
                }
            }
            self.last_tool = Some(chunk.tool_name.clone());
            let line = self
                .adapter
                .format_tool_event(chunk, &self.tool_mode, self.preview_max_len);
            // None == adapter chose to eat this event (can't render tool chrome).
            if let Some(line) = line {
                if !line.is_empty() {
                    if let Some(cb) = &self.enqueue_tool_line {
                        cb(line);
                    }
                }
            }
            return;
        }

        if let StreamEvent::ToolCallFinished(_) = &event {
            // Default: no chrome on completion (matches today — the gateway only
            // rendered "started" events). Completion drives onboarding hints.
            return;
        }

        if let StreamEvent::LongToolHint(hint) = &event {
            if let Some(cb) = &self.on_long_tool {
                cb(hint.clone());
            }
            return;
        }

        if let StreamEvent::GatewayNotice(notice) = &event {
            if let Some(cb) = &self.on_notice {
                cb(notice.clone());
            }
            return;
        }
    }
}

// Provide private aliases mirroring Python's underscore-prefixed helpers for traceability
#[allow(dead_code)]
fn _gateway_event_dispatcher_new(
    adapter: Arc<dyn GatewayAdapter>,
    sink: Option<Arc<dyn GatewaySink>>,
) -> GatewayEventDispatcher {
    GatewayEventDispatcher::with_adapter(adapter)
}
