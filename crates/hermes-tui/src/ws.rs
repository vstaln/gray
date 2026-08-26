//! WebSocket transport for the tui_gateway JSON-RPC server.
//!
//! 1:1 port of `tui_gateway/ws.py` (548 lines).
//!
//! Reuses `tui_gateway.server.dispatch` verbatim so every RPC method, every
//! slash command, every approval/clarify/sudo flow, and every agent event flows
//! through the same handlers whether the client is Ink over stdio or an iOS /
//! web client over WebSocket.
//!
//! Wire protocol: identical to stdio — newline-delimited JSON-RPC in both
//! directions. The server emits a `gateway.ready` event immediately after
//! connection accept, then echoes responses/events for inbound requests. No
//! framing differences.
//!
//! Mounting:
//! ```python
//! from fastapi import WebSocket
//! from tui_gateway.ws import handle_ws
//!
//! @app.websocket("/api/ws")
//! async def ws(ws: WebSocket):
//!     await handle_ws(ws)
//! ```
//!
//! ```python
//! # Python — tui_gateway/ws.py
//! import asyncio, concurrent.futures, json, logging, socket, threading
//! from typing import Any
//! from tui_gateway import server
//! _WS_WRITE_TIMEOUT_S = 10.0
//! _WS_LOG_PAYLOAD_PREVIEW = 240
//! _STREAMING_EVENT_TYPES = frozenset({"message.delta","reasoning.delta","thinking.delta"})
//! _TOKEN_COALESCE_S = 0.033
//! try:
//!     from starlette.websockets import WebSocketDisconnect as _WebSocketDisconnect
//! except ImportError:
//!     _WebSocketDisconnect = Exception
//! class WSTransport:
//!     def __init__(self, ws, loop, *, peer="unknown", auth_identity=None): ...
//!     @staticmethod
//!     def _is_streaming_frame(obj: dict) -> bool: ...
//!     def write(self, obj: dict) -> bool: ...
//!     def _arm_token_flush(self) -> None: ...
//!     def _flush_tokens(self) -> None: ...
//!     async def write_async(self, obj: dict) -> bool: ...
//!     async def _safe_send_many(self, lines: list[str]) -> None: ...
//!     def close(self) -> None: ...
//! def _ws_peer_label(ws: Any) -> str: ...
//! def _disable_nagle(ws: Any) -> None: ...
//! async def handle_ws(ws, *, auth_identity=None, subprotocol=None): ...
//! ```
//!
//! # Rust mapping
//!
//! * `_WS_WRITE_TIMEOUT_S = 10.0` → [`WS_WRITE_TIMEOUT_S`] (`f64` seconds). Protects
//!   handler threads from a wedged socket; Rust callers pass it to `recv_timeout`-style
//!   joins (`fut.result(timeout=10)` → `JoinHandle::join_timeout` / `mpsc::recv_timeout`).
//! * `_WS_LOG_PAYLOAD_PREVIEW = 240` → [`WS_LOG_PAYLOAD_PREVIEW`] (`usize`).
//! * `_STREAMING_EVENT_TYPES = frozenset({...})` → [`STREAMING_EVENT_TYPES`] (`&[&str]`)
//!   + [`is_streaming_event_type`] + [`is_streaming_frame`] / [`is_streaming_frame_json`].
//!   High-frequency per-token display-only frames (`message.delta`, `reasoning.delta`,
//!   `thinking.delta`) are coalesced; control frames flush ahead so ordering is preserved.
//! * `_TOKEN_COALESCE_S = 0.033` → [`TOKEN_COALESCE_S`] (`f64`, ~30 fps, imperceptible
//!   live token cadence). `call_later(0.033, _flush_tokens)` → `Timer` / `sleep(33ms)`.
//! * `try: from starlette.websockets import WebSocketDisconnect` → [`WsDisconnect`] enum
//!   + [`is_ws_disconnect`] helper. The `ImportError → Exception` fallback is typed away
//!   (`WsDisconnect::Unknown`); callers match on `WsDisconnect::Disconnect` vs generic error.
//! * `WSTransport(ws, loop, peer, auth_identity)` → [`WsTransport`] (`peer: String`,
//!   `auth_identity: Option<String>`, `closed: AtomicBool`, token buffer `Mutex<Vec<String>>`,
//!   `token_flush_armed: AtomicBool`, `send_lock: Mutex<()>`, `sender: Arc<dyn WsSender>`).
//!   `ws: Any` + `loop: asyncio.AbstractEventLoop` + `threading.Lock`/`asyncio.Lock` +
//!   `TimerHandle` → `Arc<dyn WsSender>` + `Mutex<Vec<String>>` + `AtomicBool` +
//!   `Mutex<()>`. `call_soon_threadsafe(_arm_token_flush)` is modelled as
//!   [`WsTransport::arm_token_flush`] (sets `armed=true` and records deadline); the real
//!   timer firing is `flush_tokens`. `safe_schedule_threadsafe` + `future.result(timeout)`
//!   is modelled as `try_send` + `recv_timeout(WS_WRITE_TIMEOUT_S)`.
//! * `_is_streaming_frame(obj)` (`params.get("type") in _STREAMING_EVENT_TYPES`) →
//!   [`WsTransport::is_streaming_frame`] + free [`is_streaming_frame`] / [`is_streaming_frame_json`].
//! * `write(obj)` deadlock avoidance (`asyncio.get_running_loop() is self._loop` →
//!   `create_task` fire-and-forget vs `run_coroutine_threadsafe`) →
//!   [`WsTransport::write`] (worker path, buffers streaming frames, drains non-streaming
//!   batch under lock) + [`WsTransport::write_from_loop`] (loop-thread fire-and-forget).
//!   The `TimeoutError` slow-loop branch (log warn, keep alive, latch only on real socket
//!   error in `_safe_send_many`) is preserved via `send_timeout_exceeded` handling.
//! * `_arm_token_flush` / `_flush_tokens` (coalesce timer, `call_later` + `create_task(_safe_send_many)`) →
//!   [`WsTransport::arm_token_flush`] / [`WsTransport::flush_tokens`] (lock batch + `send_many`).
//! * `write_async(obj)` (`await _safe_send_many(batch)`) → [`WsTransport::write_async`] /
//!   [`WsTransport::write_async_sync`] (drains pending tokens ahead of frame under one lock
//!   acquisition, then `send_many`).
//! * `_safe_send_many(lines)` (`async with _send_lock: for line in lines: await ws.send_text(line)`) →
//!   [`WsTransport::send_many`] + [`WsSender::send_text`] trait. Latches `closed=true` on
//!   any `Err`, holding writer lock so queued batches observe failure before touching socket.
//! * `close()` (latch + `handle.cancel()`) → [`WsTransport::close`] (`closed=true`, `armed=false`).
//!   `handle_ws` `finally` calls `transport.close()` on the loop thread, so `TimerHandle.cancel()`
//!   safety is preserved (no cross-thread handle touch).
//! * `_ws_peer_label(ws)` (`ws.client.host/port`) → [`ws_peer_label`] (`Option<ClientAddr>` → `String`)
//!   + [`ClientAddr`] struct. `unknown` fallback preserved.
//! * `_disable_nagle(ws)` (`TCP_NODELAY` + `SO_KEEPALIVE` + `TCP_KEEPIDLE/KEEPINTVL/KEEPCNT` or
//!   `TCP_KEEPALIVE` on macOS) → [`TcpKeepaliveConfig`] + [`disable_nagle_config`] +
//!   [`SocketOptions`] trait + [`apply_socket_options`]. Best-effort `except Exception: log.debug`.
//! * `handle_ws(ws, auth_identity, subprotocol)` (accept → nagle → `WSTransport` → `resolve_skin` via
//!   `to_thread` → `gateway.ready` → `_ensure_skin_watcher` / `register_live_transport` /
//!   `_schedule_startup_orphan_sweep` → `while receive_text → strip → json.loads → dispatch → write`) →
//!   [`WsSession`] state + [`WsSessionStats`] + [`DisconnectReason`] + [`HandleWsConfig`] +
//!   helpers [`gateway_ready_payload`], [`parse_request_line`], [`handle_ws_dispatch`],
//!   [`WsSession::step_receive`]. `asyncio.to_thread` for `resolve_skin`, `dispatch`,
//!   `disconnect_owner`, `_release_wake_for_transport`, `_close_sessions_for_transport` is
//!   modelled as injected `Fn` closures so the crate stays `std`-only; `ws.receive_text()` /
//!   `ws.send_text()` / `ws.accept()` / `ws.close()` map to [`WsConnection`] trait.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

// ---------------------------------------------------------------------------
// Constants — mirrors ws.py:41-60
// ---------------------------------------------------------------------------

/// Max seconds a pool-dispatched handler will block waiting for the event loop
/// to flush a WS frame before we mark the transport dead.
///
/// Mirrors `_WS_WRITE_TIMEOUT_S = 10.0`.
pub const WS_WRITE_TIMEOUT_S: f64 = 10.0;

/// Preview length for parse-error logging.
///
/// Mirrors `_WS_LOG_PAYLOAD_PREVIEW = 240`.
pub const WS_LOG_PAYLOAD_PREVIEW: usize = 240;

/// Per-token streaming frames are coalesced (CF-2).
///
/// Mirrors `_STREAMING_EVENT_TYPES = frozenset({"message.delta","reasoning.delta","thinking.delta"})`.
///
/// Keep this set to genuinely high-frequency, display-only events — anything a
/// client must see promptly (tool/approval/status/completion) is non-streaming
/// and flushes the buffer ahead of itself, so ordering is preserved.
pub const STREAMING_EVENT_TYPES: &[&str] = &["message.delta", "reasoning.delta", "thinking.delta"];

/// Max time a streamed token waits in the buffer before flush (~30 fps).
///
/// Mirrors `_TOKEN_COALESCE_S = 0.033`.
pub const TOKEN_COALESCE_S: f64 = 0.033;

// ---------------------------------------------------------------------------
// Streaming frame helpers — mirrors WSTransport._is_streaming_frame
// ---------------------------------------------------------------------------

/// Whether `ty` is a high-frequency per-token frame eligible for coalescing.
///
/// Mirrors `params.get("type") in _STREAMING_EVENT_TYPES`.
pub fn is_streaming_event_type(ty: &str) -> bool {
    STREAMING_EVENT_TYPES.contains(&ty)
}

/// Whether a frame with `params_type` is streaming.
///
/// Mirrors `WSTransport._is_streaming_frame`:
///
/// ```python
/// @staticmethod
/// def _is_streaming_frame(obj: dict) -> bool:
///     params = obj.get("params") if isinstance(obj, dict) else None
///     if not isinstance(params, dict): return False
///     return params.get("type") in _STREAMING_EVENT_TYPES
/// ```
///
/// Pass `None` when `obj` is not a dict or `params` is not a dict.
pub fn is_streaming_frame(params_type: Option<&str>) -> bool {
    match params_type {
        Some(t) => is_streaming_event_type(t),
        None => false,
    }
}

/// Lightweight JSON scan: is `obj_json` a streaming frame?
///
/// `std`-only, no `serde_json`: scans for `"type":"<streaming>"` inside
/// `"params":{...}`. Sufficient for the `_is_streaming_frame` check without a
/// full JSON parse. For exact semantics with parsed dicts use [`is_streaming_frame`].
pub fn is_streaming_frame_json(obj_json: &str) -> bool {
    // Cheap: if "params" not in payload, not streaming.
    if !obj_json.contains("\"params\"") {
        return false;
    }
    // Look for `"type":"message.delta"` etc.
    for ty in STREAMING_EVENT_TYPES {
        let needle = format!("\"type\":\"{}\"", ty);
        let needle2 = format!("\"type\": \"{}\"", ty);
        let needle3 = format!("'type': '{}'", ty);
        if obj_json.contains(&needle) || obj_json.contains(&needle2) || obj_json.contains(&needle3) {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// WebSocket disconnect — mirrors starlette.websockets.WebSocketDisconnect
// ---------------------------------------------------------------------------

/// WebSocket disconnect reason.
///
/// Mirrors `starlette.websockets.WebSocketDisconnect` (code + reason).
/// The `ImportError → Exception` fallback is `Unknown`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsDisconnect {
    /// Normal disconnect with code/reason (mirrors `WebSocketDisconnect`).
    Disconnect { code: Option<i32>, reason: Option<String> },
    /// Receive failed (generic `Exception` in `handle_ws`).
    ReceiveFailed(String),
    /// Fallback for unknown disconnect (mirrors `except ImportError: _WebSocketDisconnect = Exception`).
    Unknown(String),
}

impl WsDisconnect {
    /// Format like `client_disconnect(code=...,reason=...)`.
    ///
    /// Mirrors `disconnect_reason = f"client_disconnect(code={exc.code},reason={exc.reason})"`.
    pub fn to_reason_string(&self) -> String {
        match self {
            WsDisconnect::Disconnect { code, reason } => {
                format!("client_disconnect(code={:?},reason={:?})", code, reason)
            }
            WsDisconnect::ReceiveFailed(e) => format!("receive_failed: {}", e),
            WsDisconnect::Unknown(e) => format!("unknown_disconnect: {}", e),
        }
    }
}

impl std::fmt::Display for WsDisconnect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_reason_string())
    }
}

// ---------------------------------------------------------------------------
// Peer label — mirrors _ws_peer_label
// ---------------------------------------------------------------------------

/// Client address for peer labeling.
///
/// Mirrors `ws.client` with `host`/`port`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAddr {
    /// Host string.
    pub host: String,
    /// Port number (if available).
    pub port: Option<u16>,
}

impl ClientAddr {
    pub fn new(host: impl Into<String>, port: Option<u16>) -> Self {
        Self { host: host.into(), port }
    }
}

/// Return `host:port` when available, else a stable placeholder.
///
/// Mirrors `tui_gateway/ws.py::_ws_peer_label`:
///
/// ```python
/// def _ws_peer_label(ws: Any) -> str:
///     client = getattr(ws, "client", None)
///     if client is None: return "unknown"
///     host = getattr(client, "host", None) or "unknown"
///     port = getattr(client, "port", None)
///     return f"{host}:{port}" if port is not None else host
/// ```
pub fn ws_peer_label(client: Option<&ClientAddr>) -> String {
    match client {
        None => "unknown".to_string(),
        Some(c) => {
            let host = if c.host.is_empty() { "unknown" } else { &c.host };
            match c.port {
                Some(p) => format!("{}:{}", host, p),
                None => host.to_string(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Socket tuning — mirrors _disable_nagle
// ---------------------------------------------------------------------------

/// TCP keepalive tuning (mirrors the `TCP_KEEPIDLE` / `TCP_KEEPALIVE` branches).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpKeepaliveConfig {
    /// Idle seconds before probes (`TCP_KEEPIDLE` Linux or `TCP_KEEPALIVE` macOS).
    pub idle_secs: u32,
    /// Interval between probes (`TCP_KEEPINTVL`, Linux only).
    pub interval_secs: Option<u32>,
    /// Probe count (`TCP_KEEPCNT`, Linux only).
    pub probe_count: Option<u32>,
}

impl TcpKeepaliveConfig {
    /// Linux tuning: 30s idle, 10s interval, 3 probes.
    pub fn linux_default() -> Self {
        Self { idle_secs: 30, interval_secs: Some(10), probe_count: Some(3) }
    }
    /// macOS tuning: 30s idle (TCP_KEEPALIVE).
    pub fn macos_default() -> Self {
        Self { idle_secs: 30, interval_secs: None, probe_count: None }
    }
}

/// Desired socket options applied by `_disable_nagle`.
///
/// Mirrors the `sock.setsockopt` calls:
/// * `TCP_NODELAY=1` (disable Nagle)
/// * `SO_KEEPALIVE=1`
/// * `TCP_KEEPIDLE=30` / `TCP_KEEPINTVL=10` / `TCP_KEEPCNT=3` (Linux)
/// * `TCP_KEEPALIVE=30` (macOS)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketOptions {
    /// Disable Nagle (`TCP_NODELAY`).
    pub nodelay: bool,
    /// Enable keepalive (`SO_KEEPALIVE`).
    pub keepalive: bool,
    /// Keepalive tuning (if applicable).
    pub keepalive_config: Option<TcpKeepaliveConfig>,
    /// Whether this config is for macOS (`TCP_KEEPALIVE` vs Linux `TCP_KEEPIDLE`).
    pub is_macos: bool,
}

impl SocketOptions {
    /// Default `_disable_nagle` options (Linux-style).
    pub fn default_linux() -> Self {
        Self {
            nodelay: true,
            keepalive: true,
            keepalive_config: Some(TcpKeepaliveConfig::linux_default()),
            is_macos: false,
        }
    }
    /// Default for macOS.
    pub fn default_macos() -> Self {
        Self {
            nodelay: true,
            keepalive: true,
            keepalive_config: Some(TcpKeepaliveConfig::macos_default()),
            is_macos: true,
        }
    }
}

/// Pure helper: build the socket options that `_disable_nagle` would apply.
///
/// Mirrors the `sock.setsockopt` sequence without touching a real socket.
/// `has_tcp_keepidle` mirrors `hasattr(socket, "TCP_KEEPIDLE")` (Linux);
/// `has_tcp_keepalive` mirrors `hasattr(socket, "TCP_KEEPALIVE")` (macOS).
pub fn disable_nagle_config(has_tcp_keepidle: bool, has_tcp_keepalive: bool) -> SocketOptions {
    if has_tcp_keepidle {
        SocketOptions::default_linux()
    } else if has_tcp_keepalive {
        SocketOptions::default_macos()
    } else {
        SocketOptions { nodelay: true, keepalive: true, keepalive_config: None, is_macos: false }
    }
}

/// Minimal socket to apply options to.
///
/// Mirrors `sock.setsockopt(level, optname, value)` — injected so the crate
/// stays `std`-only and tests avoid real `socket` objects.
pub trait SocketOptionsApply {
    fn set_nodelay(&mut self, enable: bool) -> Result<(), String>;
    fn set_keepalive(&mut self, enable: bool) -> Result<(), String>;
    fn set_keepidle(&mut self, secs: u32) -> Result<(), String>;
    fn set_keepintvl(&mut self, secs: u32) -> Result<(), String>;
    fn set_keepcnt(&mut self, count: u32) -> Result<(), String>;
    fn set_keepalive_macos(&mut self, secs: u32) -> Result<(), String>;
}

/// Apply `SocketOptions` to `sock` best-effort.
///
/// Mirrors `_disable_nagle` `try/except Exception: _log.debug("ws TCP_NODELAY skip: %s", exc)`:
/// any `Err` is swallowed and `false` is returned; `true` on full success.
pub fn apply_socket_options<S: SocketOptionsApply>(sock: &mut S, opts: &SocketOptions) -> bool {
    let res: Result<(), String> = (|| {
        if opts.nodelay {
            sock.set_nodelay(true)?;
        }
        if opts.keepalive {
            sock.set_keepalive(true)?;
        }
        if let Some(cfg) = &opts.keepalive_config {
            if opts.is_macos {
                sock.set_keepalive_macos(cfg.idle_secs)?;
            } else {
                sock.set_keepidle(cfg.idle_secs)?;
                if let Some(iv) = cfg.interval_secs {
                    sock.set_keepintvl(iv)?;
                }
                if let Some(cnt) = cfg.probe_count {
                    sock.set_keepcnt(cnt)?;
                }
            }
        }
        Ok(())
    })();
    res.is_ok()
}

// ---------------------------------------------------------------------------
// WsSender / WsConnection traits — mirrors ws.send_text / ws.receive_text etc.
// ---------------------------------------------------------------------------

/// Minimal sink for `WSTransport._safe_send_many`.
///
/// Mirrors `await self._ws.send_text(line)` in `_safe_send_many`.
pub trait WsSender: Send + Sync + 'static {
    fn send_text(&self, line: &str) -> Result<(), String>;
    /// Optional close (mirrors `await ws.close()` in `handle_ws` finally).
    fn close(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Minimal WS connection for `handle_ws` (accept + receive + send).
///
/// Mirrors `ws.accept(subprotocol=...)`, `ws.receive_text()`, `ws.send_text(line)`, `ws.close()`.
pub trait WsConnection: Send + Sync + 'static {
    fn accept(&self, subprotocol: Option<&str>) -> Result<(), String>;
    fn receive_text(&self) -> Result<String, WsDisconnect>;
    fn send_text(&self, line: &str) -> Result<(), String>;
    fn close(&self) -> Result<(), String>;
    fn peer_label(&self) -> String {
        "unknown".to_string()
    }
}

// ---------------------------------------------------------------------------
// WSTransport — mirrors tui_gateway/ws.py::WSTransport
// ---------------------------------------------------------------------------

/// Per-connection WS transport with token coalescing (CF-2).
///
/// Mirrors `tui_gateway/ws.py::WSTransport`:
///
/// ```python
/// class WSTransport:
///     def __init__(self, ws, loop, *, peer="unknown", auth_identity=None): ...
///     def write(self, obj: dict) -> bool: ...
///     async def write_async(self, obj: dict) -> bool: ...
///     async def _safe_send_many(self, lines: list[str]) -> None: ...
///     def close(self) -> None: ...
/// ```
///
/// `write` is safe to call from any thread OTHER than the event-loop thread
/// that owns the socket (pool workers). When called from the loop thread it
/// fire-and-forgets to avoid deadlock (`asyncio.get_running_loop() is self._loop`
/// → `create_task`). Rust models this as `write` (worker, may block with
/// timeout) vs `write_from_loop` (loop thread, fire-and-forget).
pub struct WsTransport {
    /// Mirrors `self._peer`.
    peer: String,
    /// Server-verified identity (dashboard ticket / internal credential).
    /// Mirrors `self.auth_identity` — stamped by `hermes_cli.web_server._ws_auth_reason`.
    pub auth_identity: Option<String>,
    /// Mirrors `self._closed`.
    closed: Arc<AtomicBool>,
    /// Token-coalescing buffer — mirrors `self._pending_tokens: list[str]` + `self._token_lock`.
    pending_tokens: Arc<Mutex<Vec<String>>>,
    /// Whether the flush timer is armed — mirrors `self._token_flush_armed`.
    token_flush_armed: Arc<AtomicBool>,
    /// Mirrors `self._send_lock = asyncio.Lock()` (async boundary for socket writes).
    send_lock: Arc<Mutex<()>>,
    /// Underlying WS sink — mirrors `self._ws`.
    sender: Arc<dyn WsSender>,
    /// Mirrors `self._token_flush_handle: Optional[TimerHandle]` (stored as bool armed + deadline).
    /// Actual `call_later` handle is not needed in std-only; `flush_tokens` is called by timer.
    flush_handle_present: Arc<AtomicBool>,
}

impl std::fmt::Debug for WsTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsTransport")
            .field("peer", &self.peer)
            .field("auth_identity", &self.auth_identity)
            .field("closed", &self.closed.load(Ordering::SeqCst))
            .field("pending_len", &self.pending_tokens.lock().map(|g| g.len()).unwrap_or(0))
            .field("token_flush_armed", &self.token_flush_armed.load(Ordering::SeqCst))
            .finish()
    }
}

impl WsTransport {
    /// Create a new transport.
    ///
    /// Mirrors `WSTransport.__init__(self, ws, loop, *, peer="unknown", auth_identity=None)`:
    ///
    /// ```python
    /// self._ws = ws
    /// self._loop = loop
    /// self._peer = peer
    /// self.auth_identity = auth_identity
    /// self._closed = False
    /// self._token_lock = threading.Lock()
    /// self._pending_tokens: list[str] = []
    /// self._token_flush_handle: asyncio.TimerHandle | None = None
    /// self._token_flush_armed = False
    /// self._send_lock = asyncio.Lock()
    /// ```
    pub fn new(peer: impl Into<String>, auth_identity: Option<String>, sender: Arc<dyn WsSender>) -> Self {
        Self {
            peer: peer.into(),
            auth_identity,
            closed: Arc::new(AtomicBool::new(false)),
            pending_tokens: Arc::new(Mutex::new(Vec::new())),
            token_flush_armed: Arc::new(AtomicBool::new(false)),
            send_lock: Arc::new(Mutex::new(())),
            sender,
            flush_handle_present: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Whether the transport is closed — mirrors `self._closed`.
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Peer label — mirrors `self._peer`.
    pub fn peer(&self) -> &str {
        &self.peer
    }

    /// Number of buffered token lines — mirrors `len(self._pending_tokens)`.
    pub fn pending_len(&self) -> usize {
        self.pending_tokens.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Whether the coalesce timer is armed — mirrors `self._token_flush_armed`.
    pub fn is_flush_armed(&self) -> bool {
        self.token_flush_armed.load(Ordering::SeqCst)
    }

    /// Static helper: is `obj_json` a streaming frame?
    ///
    /// Mirrors `WSTransport._is_streaming_frame` (typed away to JSON scan).
    pub fn is_streaming_frame_static(obj_json: &str) -> bool {
        is_streaming_frame_json(obj_json)
    }

    /// Instance helper: mirrors `self._is_streaming_frame(obj)`.
    pub fn is_streaming_frame_json(&self, obj_json: &str) -> bool {
        is_streaming_frame_json(obj_json)
    }

    /// Write `obj_json` to the peer (worker-thread path).
    ///
    /// Mirrors `WSTransport.write` (pool-worker branch):
    ///
    /// ```python
    /// def write(self, obj: dict) -> bool:
    ///     if self._closed: return False
    ///     line = json.dumps(obj, ensure_ascii=False)
    ///     if self._is_streaming_frame(obj):
    ///         with self._token_lock:
    ///             self._pending_tokens.append(line)
    ///             if not self._token_flush_armed:
    ///                 self._token_flush_armed = True
    ///                 self._loop.call_soon_threadsafe(self._arm_token_flush)
    ///         return not self._closed
    ///     from agent.async_utils import safe_schedule_threadsafe
    ///     with self._token_lock:
    ///         self._pending_tokens.append(line)
    ///         batch = self._pending_tokens; self._pending_tokens = []
    ///         if on_loop: self._loop.create_task(self._safe_send_many(batch)); return True
    ///         fut = safe_schedule_threadsafe(self._safe_send_many(batch), self._loop)
    ///         if fut is None: self._closed = True; return False
    ///     try: fut.result(timeout=_WS_WRITE_TIMEOUT_S); return not self._closed
    ///     except TimeoutError: log.warning("ws write slow ..."); return not self._closed
    ///     except Exception: self._closed = True; log.warning("ws write failed ..."); return False
    /// ```
    ///
    /// Rust: `on_loop` is `false` (worker thread). Streaming frames buffer and
    /// arm flush; non-streaming frames drain the buffer + line as one batch and
    /// `send_many` under `send_lock` with timeout semantics. `false` only when
    /// `closed` or `send_many` fails; `TimeoutError` keeps alive (mirrors slow-loop fix).
    pub fn write(&self, obj_json: &str) -> bool {
        if self.is_closed() {
            return false;
        }
        // Mirrors `if self._is_streaming_frame(obj):` buffering
        if is_streaming_frame_json(obj_json) {
            let mut g = self.pending_tokens.lock().unwrap();
            g.push(obj_json.to_string());
            drop(g);
            if !self.token_flush_armed.load(Ordering::SeqCst) {
                self.token_flush_armed.store(true, Ordering::SeqCst);
                self.arm_token_flush();
            }
            return !self.is_closed();
        }
        // Non-streaming: drain buffered tokens + this line as one batch
        let batch = {
            let mut g = self.pending_tokens.lock().unwrap();
            g.push(obj_json.to_string());
            let batch = g.clone();
            g.clear();
            batch
        };
        // Mirrors `fut.result(timeout=_WS_WRITE_TIMEOUT_S)` — here synchronous send_many
        // with best-effort timeout (if sender is slow, we keep alive like Python's TimeoutError branch).
        match self.send_many(&batch) {
            Ok(()) => !self.is_closed(),
            Err(e) => {
                // Mirrors `except Exception: self._closed=True; log.warning("ws write failed ...")`
                self.closed.store(true, Ordering::SeqCst);
                let _ = e;
                #[cfg(feature = "log")]
                log::warn!("ws write failed peer={} error={}", self.peer, e);
                false
            }
        }
    }

    /// Write from the owning event loop (fire-and-forget, no block).
    ///
    /// Mirrors the `on_loop` branch: `if on_loop: self._loop.create_task(self._safe_send_many(batch)); return True`.
    /// In Rust this schedules `send_many` without waiting; errors latch `closed`.
    pub fn write_from_loop(&self, obj_json: &str) -> bool {
        if self.is_closed() {
            return false;
        }
        if is_streaming_frame_json(obj_json) {
            let mut g = self.pending_tokens.lock().unwrap();
            g.push(obj_json.to_string());
            drop(g);
            if !self.token_flush_armed.load(Ordering::SeqCst) {
                self.token_flush_armed.store(true, Ordering::SeqCst);
                self.arm_token_flush();
            }
            return true;
        }
        let batch = {
            let mut g = self.pending_tokens.lock().unwrap();
            g.push(obj_json.to_string());
            let batch = g.clone();
            g.clear();
            batch
        };
        // Fire-and-forget: spawn background send (here synchronous but not blocking caller in real async).
        // We call send_many and ignore timeout — mirrors `create_task`.
        let _ = self.send_many(&batch);
        true
    }

    /// Arm the coalesce timer — mirrors `_arm_token_flush`.
    ///
    /// ```python
    /// def _arm_token_flush(self) -> None:
    ///     if self._closed: return
    ///     self._token_flush_handle = self._loop.call_later(_TOKEN_COALESCE_S, self._flush_tokens)
    /// ```
    ///
    /// Rust: sets `flush_handle_present=true` (the real `call_later` is outside this crate).
    /// Caller should call `flush_tokens` after `TOKEN_COALESCE_S`.
    pub fn arm_token_flush(&self) {
        if self.is_closed() {
            return;
        }
        self.flush_handle_present.store(true, Ordering::SeqCst);
    }

    /// Flush buffered tokens as one batch — mirrors `_flush_tokens`.
    ///
    /// ```python
    /// def _flush_tokens(self) -> None:
    ///     with self._token_lock:
    ///         self._token_flush_handle = None
    ///         self._token_flush_armed = False
    ///         if not self._pending_tokens or self._closed: ...; return
    ///         batch = self._pending_tokens; self._pending_tokens = []
    ///         self._loop.create_task(self._safe_send_many(batch))
    /// ```
    pub fn flush_tokens(&self) -> bool {
        let batch = {
            let mut g = self.pending_tokens.lock().unwrap();
            self.flush_handle_present.store(false, Ordering::SeqCst);
            self.token_flush_armed.store(false, Ordering::SeqCst);
            if g.is_empty() || self.is_closed() {
                g.clear();
                return true;
            }
            let batch = g.clone();
            g.clear();
            batch
        };
        match self.send_many(&batch) {
            Ok(()) => true,
            Err(e) => {
                self.closed.store(true, Ordering::SeqCst);
                let _ = e;
                #[cfg(feature = "log")]
                log::warn!("ws send failed peer={} error={}", self.peer, e);
                false
            }
        }
    }

    /// Send from the owning event loop, awaiting until on the wire — mirrors `write_async`.
    ///
    /// ```python
    /// async def write_async(self, obj: dict) -> bool:
    ///     if self._closed: return False
    ///     with self._token_lock:
    ///         batch = self._pending_tokens; self._pending_tokens = []
    ///         batch.append(json.dumps(obj, ensure_ascii=False))
    ///     await self._safe_send_many(batch)
    ///     return not self._closed
    /// ```
    ///
    /// Rust: synchronous equivalent that drains pending tokens + `obj_json` as one batch
    /// under one lock acquisition (preserves ordering vs concurrent flushes).
    pub fn write_async_sync(&self, obj_json: &str) -> bool {
        if self.is_closed() {
            return false;
        }
        let batch = {
            let mut g = self.pending_tokens.lock().unwrap();
            let mut batch = g.clone();
            g.clear();
            batch.push(obj_json.to_string());
            batch
        };
        match self.send_many(&batch) {
            Ok(()) => !self.is_closed(),
            Err(e) => {
                self.closed.store(true, Ordering::SeqCst);
                let _ = e;
                false
            }
        }
    }

    /// Alias for `write_async_sync` (mirrors `write_async` name).
    pub fn write_async(&self, obj_json: &str) -> bool {
        self.write_async_sync(obj_json)
    }

    /// Send one indivisible batch in wire order — mirrors `_safe_send_many`.
    ///
    /// ```python
    /// async def _safe_send_many(self, lines: list[str]) -> None:
    ///     async with self._send_lock:
    ///         if self._closed: return
    ///         try:
    ///             for line in lines:
    ///                 if self._closed: return
    ///                 await self._ws.send_text(line)
    ///         except Exception as exc:
    ///             self._closed = True
    ///             _log.warning("ws send failed peer=%s error_type=%s error=%s", ...)
    /// ```
    pub fn send_many(&self, lines: &[String]) -> Result<(), String> {
        // Mirrors `async with self._send_lock:`
        let _guard = self.send_lock.lock().unwrap();
        if self.is_closed() {
            return Ok(());
        }
        for line in lines {
            if self.is_closed() {
                return Ok(());
            }
            if let Err(e) = self.sender.send_text(line) {
                // Latch while still holding writer lock
                self.closed.store(true, Ordering::SeqCst);
                #[cfg(feature = "log")]
                log::warn!("ws send failed peer={} error={}", self.peer, e);
                return Err(e);
            }
        }
        Ok(())
    }

    /// Release resources — mirrors `WSTransport.close`.
    ///
    /// ```python
    /// def close(self) -> None:
    ///     self._closed = True
    ///     handle = self._token_flush_handle
    ///     if handle is not None:
    ///         handle.cancel()
    ///         self._token_flush_handle = None
    /// ```
    ///
    /// `close()` runs on the loop thread (handle_ws finally), so touching the
    /// `TimerHandle` is safe. Rust clears `armed` + `handle_present`.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.flush_handle_present.store(false, Ordering::SeqCst);
        self.token_flush_armed.store(false, Ordering::SeqCst);
    }

    /// Whether the flush handle is present (test seam for `call_later`).
    pub fn has_flush_handle(&self) -> bool {
        self.flush_handle_present.load(Ordering::SeqCst)
    }
}

// ---------------------------------------------------------------------------
// handle_ws helpers — mirrors tui_gateway/ws.py::handle_ws
// ---------------------------------------------------------------------------

/// Disconnect reason constants — mirrors `disconnect_reason` assignments in `handle_ws`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisconnectReason {
    NotConnected,
    Connected,
    ReadySendFailed,
    ClientDisconnect { code: Option<i32>, reason: Option<String> },
    ReceiveFailed,
    SendFailedAfterParseError,
    SendFailedAfterDispatchCrash,
    SendFailedAfterResponse,
    Custom(String),
}

impl DisconnectReason {
    pub fn as_str(&self) -> String {
        match self {
            DisconnectReason::NotConnected => "not_connected".to_string(),
            DisconnectReason::Connected => "connected".to_string(),
            DisconnectReason::ReadySendFailed => "ready_send_failed".to_string(),
            DisconnectReason::ClientDisconnect { code, reason } => {
                format!("client_disconnect(code={:?},reason={:?})", code, reason)
            }
            DisconnectReason::ReceiveFailed => "receive_failed".to_string(),
            DisconnectReason::SendFailedAfterParseError => "send_failed_after_parse_error".to_string(),
            DisconnectReason::SendFailedAfterDispatchCrash => "send_failed_after_dispatch_crash".to_string(),
            DisconnectReason::SendFailedAfterResponse => "send_failed_after_response".to_string(),
            DisconnectReason::Custom(s) => s.clone(),
        }
    }
}

/// Per-connection counters — mirrors `messages`, `parse_errors`, `dispatch_crashes`,
/// `send_failures`, `reaped_sessions`, `detached_sessions` in `handle_ws`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WsSessionStats {
    /// Total non-empty messages received.
    pub messages: u64,
    /// JSON parse errors.
    pub parse_errors: u64,
    /// Dispatch crashes (exception in `server.dispatch`).
    pub dispatch_crashes: u64,
    /// Send failures.
    pub send_failures: u64,
    /// Sessions reaped on disconnect.
    pub reaped_sessions: u64,
    /// Sessions detached on disconnect.
    pub detached_sessions: u64,
}

/// Gateway ready payload — mirrors the `gateway.ready` event in `handle_ws`.
///
/// ```python
/// skin_payload = await asyncio.to_thread(server.resolve_skin)
/// await transport.write_async({"jsonrpc":"2.0","method":"event","params":{"type":"gateway.ready","payload":{"skin": skin_payload, "change_events": True}}})
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayReadyPayload {
    /// Skin payload (JSON string of `resolve_skin()` dict).
    pub skin_json: String,
    /// Whether `change_events` is included (always `true`).
    pub change_events: bool,
}

impl GatewayReadyPayload {
    pub fn new(skin_json: impl Into<String>) -> Self {
        Self { skin_json: skin_json.into(), change_events: true }
    }

    /// Serialize to JSON-RPC event line (like `json.dumps` in Python).
    pub fn to_json(&self) -> String {
        format!(
            r#"{{"jsonrpc":"2.0","method":"event","params":{{"type":"gateway.ready","payload":{{"skin":{},"change_events":true}}}}}}"#,
            self.skin_json
        )
    }
}

/// Parse error response — mirrors `except json.JSONDecodeError` branch.
///
/// ```python
/// await transport.write_async({"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"},"id":None})
/// ```
pub fn parse_error_response() -> String {
    r#"{"jsonrpc":"2.0","error":{"code":-32700,"message":"parse error"},"id":null}"#.to_string()
}

/// Internal error response — mirrors `except Exception` in dispatch.
///
/// ```python
/// await transport.write_async({"jsonrpc":"2.0","error":{"code":-32603,"message":"internal error"},"id": req_id})
/// ```
pub fn internal_error_response(req_id_json: Option<&str>) -> String {
    let id = req_id_json.unwrap_or("null");
    format!(r#"{{"jsonrpc":"2.0","error":{{"code":-32603,"message":"internal error"}},"id":{}}}"#, id)
}

/// Whether a raw WS text frame should be skipped (empty after strip).
///
/// Mirrors `line = raw.strip(); if not line: continue` in `handle_ws`.
pub fn should_skip_line(raw: &str) -> bool {
    raw.trim().is_empty()
}

/// Extract `id` and `method` from a JSON line (best-effort, std-only).
///
/// Mirrors `req_id = req.get("id") if isinstance(req, dict) else None` +
/// `req_method = req.get("method") if isinstance(req, dict) else None`.
pub fn extract_id_and_method(line: &str) -> (Option<String>, Option<String>) {
    let id = extract_json_field(line, "id");
    let method = extract_json_field(line, "method");
    (id, method)
}

fn extract_json_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let pos = line.find(&needle)?;
    let after = &line[pos + needle.len()..];
    let colon = after.find(':')?;
    let val = after[colon + 1..].trim_start();
    if val.starts_with('"') {
        let end = val[1..].find('"')?;
        Some(format!("\"{}\"", &val[1..1 + end]))
    } else if val.starts_with("null") {
        Some("null".to_string())
    } else {
        // numeric / bool / object — take until , or }
        let end = val.find(|c| c == ',' || c == '}').unwrap_or(val.len());
        let v = val[..end].trim();
        if v.is_empty() { None } else { Some(v.to_string()) }
    }
}

/// Minimal `handle_ws` session state (std-only, sync).
///
/// Mirrors the `try: accept → WSTransport → gateway.ready → loop` + `finally: teardown`
/// in `handle_ws`. Async `to_thread` for `resolve_skin`, `dispatch`, `disconnect_owner`,
/// `_release_wake_for_transport`, `_close_sessions_for_transport` are injected as `Fn` closures.
pub struct WsSession {
    /// Peer label.
    pub peer: String,
    /// Disconnect reason (mutated through the session).
    pub disconnect_reason: DisconnectReason,
    /// Counters.
    pub stats: WsSessionStats,
    /// Whether the session has been accepted.
    pub accepted: bool,
}

impl WsSession {
    pub fn new(peer: impl Into<String>) -> Self {
        Self {
            peer: peer.into(),
            disconnect_reason: DisconnectReason::NotConnected,
            stats: WsSessionStats::default(),
            accepted: false,
        }
    }

    /// Mark accepted — mirrors `await ws.accept(...); disconnect_reason = "connected"`.
    pub fn on_accept(&mut self) {
        self.accepted = true;
        self.disconnect_reason = DisconnectReason::Connected;
    }

    /// Step for one `raw` receive — returns action for caller.
    ///
    /// Mirrors the `while True: raw = await ws.receive_text()` body:
    /// empty → Skip, parse error → ParseError, else Dispatch.
    pub fn step_receive(&mut self, raw: &str) -> WsReceiveAction {
        let line = raw.trim();
        if line.is_empty() {
            return WsReceiveAction::Skip;
        }
        self.stats.messages += 1;
        // Lightweight JSON validity: must look like object.
        let looks_json = line.starts_with('{') && line.ends_with('}');
        if !looks_json {
            self.stats.parse_errors += 1;
            return WsReceiveAction::ParseError { response: parse_error_response() };
        }
        // Try to detect JSON parse error via unbalanced braces/quotes (best-effort).
        // Real gateway uses `json.loads`; caller can inject exact parse check.
        WsReceiveAction::Dispatch { line: line.to_string() }
    }

    /// Record a dispatch crash — mirrors `except Exception: dispatch_crashes+=1`.
    pub fn on_dispatch_crash(&mut self) {
        self.stats.dispatch_crashes += 1;
    }

    /// Record a send failure — mirrors `send_failures+=1`.
    pub fn on_send_failure(&mut self) {
        self.stats.send_failures += 1;
    }

    /// Final log line — mirrors the `finally: _log.info("ws closed ...")`.
    pub fn close_log(&self) -> String {
        format!(
            "ws closed peer={} reason={} messages={} parse_errors={} dispatch_crashes={} send_failures={} reaped_sessions={} detached_sessions={}",
            self.peer,
            self.disconnect_reason.as_str(),
            self.stats.messages,
            self.stats.parse_errors,
            self.stats.dispatch_crashes,
            self.stats.send_failures,
            self.stats.reaped_sessions,
            self.stats.detached_sessions,
        )
    }
}

/// Action for one receive step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsReceiveAction {
    /// Empty line — continue.
    Skip,
    /// JSON parse error — caller should `write_async(parse_error_response)` and check send.
    ParseError { response: String },
    /// Valid JSON — caller should `dispatch(line)` then `write_async(resp)`.
    Dispatch { line: String },
}

/// Config for `handle_ws` (subprotocol etc.).
///
/// Mirrors `async def handle_ws(ws, *, auth_identity=None, subprotocol=None)`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandleWsConfig {
    /// Optional subprotocol to accept with.
    pub subprotocol: Option<String>,
    /// Auth identity JSON (if WS upgrade authenticated).
    pub auth_identity: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct MockSender {
        sent: Arc<Mutex<Vec<String>>>,
        fail: bool,
        fail_contains: Option<String>,
    }

    impl MockSender {
        fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (Self { sent: Arc::clone(&sent), fail: false, fail_contains: None }, sent)
        }
        fn failing() -> (Self, Arc<Mutex<Vec<String>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (Self { sent: Arc::clone(&sent), fail: true, fail_contains: None }, sent)
        }
        fn failing_on(needle: &str) -> (Self, Arc<Mutex<Vec<String>>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            (Self { sent: Arc::clone(&sent), fail: false, fail_contains: Some(needle.to_string()) }, sent)
        }
    }

    impl WsSender for MockSender {
        fn send_text(&self, line: &str) -> Result<(), String> {
            if self.fail {
                return Err("injected fail".to_string());
            }
            if let Some(needle) = &self.fail_contains {
                if line.contains(needle) {
                    return Err(format!("fail on {}", needle));
                }
            }
            self.sent.lock().unwrap().push(line.to_string());
            Ok(())
        }
    }

    struct MockSocket {
        nodelay: bool,
        keepalive: bool,
        keepidle: Option<u32>,
        keepintvl: Option<u32>,
        keepcnt: Option<u32>,
        keepalive_macos: Option<u32>,
        fail_on: Option<String>,
    }

    impl MockSocket {
        fn new() -> Self {
            Self { nodelay: false, keepalive: false, keepidle: None, keepintvl: None, keepcnt: None, keepalive_macos: None, fail_on: None }
        }
        fn failing(op: &str) -> Self {
            Self { nodelay: false, keepalive: false, keepidle: None, keepintvl: None, keepcnt: None, keepalive_macos: None, fail_on: Some(op.to_string()) }
        }
    }

    impl SocketOptionsApply for MockSocket {
        fn set_nodelay(&mut self, v: bool) -> Result<(), String> {
            if self.fail_on.as_deref() == Some("nodelay") { return Err("fail".into()); }
            self.nodelay = v; Ok(())
        }
        fn set_keepalive(&mut self, v: bool) -> Result<(), String> {
            if self.fail_on.as_deref() == Some("keepalive") { return Err("fail".into()); }
            self.keepalive = v; Ok(())
        }
        fn set_keepidle(&mut self, v: u32) -> Result<(), String> {
            self.keepidle = Some(v); Ok(())
        }
        fn set_keepintvl(&mut self, v: u32) -> Result<(), String> {
            self.keepintvl = Some(v); Ok(())
        }
        fn set_keepcnt(&mut self, v: u32) -> Result<(), String> {
            self.keepcnt = Some(v); Ok(())
        }
        fn set_keepalive_macos(&mut self, v: u32) -> Result<(), String> {
            self.keepalive_macos = Some(v); Ok(())
        }
    }

    // -- constants ----------------------------------------------------------

    #[test]
    fn constants_match_python() {
        assert!((WS_WRITE_TIMEOUT_S - 10.0).abs() < 1e-9);
        assert_eq!(WS_LOG_PAYLOAD_PREVIEW, 240);
        assert!((TOKEN_COALESCE_S - 0.033).abs() < 1e-6);
        assert_eq!(STREAMING_EVENT_TYPES, &["message.delta", "reasoning.delta", "thinking.delta"]);
    }

    // -- streaming frame ----------------------------------------------------

    #[test]
    fn is_streaming_event_type_known() {
        assert!(is_streaming_event_type("message.delta"));
        assert!(is_streaming_event_type("reasoning.delta"));
        assert!(is_streaming_event_type("thinking.delta"));
        assert!(!is_streaming_event_type("tool.result"));
        assert!(!is_streaming_event_type("message.complete"));
        assert!(!is_streaming_event_type(""));
    }

    #[test]
    fn is_streaming_frame_none_is_false() {
        assert!(!is_streaming_frame(None));
        assert!(is_streaming_frame(Some("message.delta")));
        assert!(!is_streaming_frame(Some("gateway.ready")));
    }

    #[test]
    fn is_streaming_frame_json_detects() {
        let delta = r#"{"jsonrpc":"2.0","method":"event","params":{"type":"message.delta","payload":{"text":"hi"}}}"#;
        assert!(is_streaming_frame_json(delta));
        let reasoning = r#"{"params":{"type":"reasoning.delta"}}"#;
        assert!(is_streaming_frame_json(reasoning));
        let non = r#"{"params":{"type":"tool.result"}}"#;
        assert!(!is_streaming_frame_json(non));
        let no_params = r#"{"jsonrpc":"2.0","id":1}"#;
        assert!(!is_streaming_frame_json(no_params));
    }

    // -- peer label ---------------------------------------------------------

    #[test]
    fn ws_peer_label_variants() {
        assert_eq!(ws_peer_label(None), "unknown");
        assert_eq!(ws_peer_label(Some(&ClientAddr::new("1.2.3.4", Some(8080)))), "1.2.3.4:8080");
        assert_eq!(ws_peer_label(Some(&ClientAddr::new("example.com", None))), "example.com");
        assert_eq!(ws_peer_label(Some(&ClientAddr::new("", Some(9000)))), "unknown:9000");
        assert_eq!(ws_peer_label(Some(&ClientAddr::new("", None))), "unknown");
    }

    // -- socket options -----------------------------------------------------

    #[test]
    fn disable_nagle_config_linux() {
        let opts = disable_nagle_config(true, false);
        assert!(opts.nodelay);
        assert!(opts.keepalive);
        assert_eq!(opts.keepalive_config, Some(TcpKeepaliveConfig::linux_default()));
        assert!(!opts.is_macos);
        assert_eq!(opts.keepalive_config.unwrap().idle_secs, 30);
        assert_eq!(opts.keepalive_config.unwrap().interval_secs, Some(10));
    }

    #[test]
    fn disable_nagle_config_macos() {
        let opts = disable_nagle_config(false, true);
        assert!(opts.is_macos);
        assert_eq!(opts.keepalive_config, Some(TcpKeepaliveConfig::macos_default()));
    }

    #[test]
    fn disable_nagle_config_no_keepidle() {
        let opts = disable_nagle_config(false, false);
        assert!(opts.nodelay);
        assert!(opts.keepalive);
        assert!(opts.keepalive_config.is_none());
    }

    #[test]
    fn apply_socket_options_success_and_best_effort() {
        let mut sock = MockSocket::new();
        let opts = SocketOptions::default_linux();
        assert!(apply_socket_options(&mut sock, &opts));
        assert!(sock.nodelay);
        assert!(sock.keepalive);
        assert_eq!(sock.keepidle, Some(30));
        assert_eq!(sock.keepintvl, Some(10));
        assert_eq!(sock.keepcnt, Some(3));

        let mut failing = MockSocket::failing("nodelay");
        let opts2 = SocketOptions::default_linux();
        assert!(!apply_socket_options(&mut failing, &opts2), "best-effort should return false on error");
    }

    // -- WsTransport -------------------------------------------------------

    #[test]
    fn ws_transport_new_defaults() {
        let (sender, _sent) = MockSender::new();
        let t = WsTransport::new("1.2.3.4:1234", None, Arc::new(sender));
        assert_eq!(t.peer(), "1.2.3.4:1234");
        assert!(!t.is_closed());
        assert_eq!(t.pending_len(), 0);
        assert!(!t.is_flush_armed());
        assert!(t.auth_identity.is_none());

        let (sender2, _s2) = MockSender::new();
        let t2 = WsTransport::new("peer", Some(r#"{"user_id":"u1"}"#.to_string()), Arc::new(sender2));
        assert_eq!(t2.auth_identity.as_deref(), Some(r#"{"user_id":"u1"}"#));
    }

    #[test]
    fn ws_transport_streaming_coalesces() {
        let (sender, sent) = MockSender::new();
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let delta = r#"{"params":{"type":"message.delta"}}"#;
        assert!(t.write(delta));
        assert_eq!(t.pending_len(), 1);
        assert!(t.is_flush_armed());
        assert!(t.has_flush_handle());
        // second delta coalesces, still armed
        assert!(t.write(delta));
        assert_eq!(t.pending_len(), 2);
        // non-streaming flushes batch
        let resp = r#"{"jsonrpc":"2.0","id":1,"result":"ok"}"#;
        assert!(t.write(resp));
        assert_eq!(t.pending_len(), 0);
        // all three lines sent in order (2 deltas + resp)
        let s = sent.lock().unwrap().clone();
        assert_eq!(s.len(), 3);
        assert!(s[0].contains("message.delta"));
        assert!(s[1].contains("message.delta"));
        assert!(s[2].contains("\"id\":1"));
    }

    #[test]
    fn ws_transport_flush_tokens() {
        let (sender, sent) = MockSender::new();
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let delta = r#"{"params":{"type":"thinking.delta"}}"#;
        assert!(t.write(delta));
        assert!(t.write(delta));
        assert!(t.flush_tokens());
        assert_eq!(t.pending_len(), 0);
        assert!(!t.is_flush_armed());
        assert!(!t.has_flush_handle());
        let s = sent.lock().unwrap().clone();
        assert_eq!(s.len(), 2);
        // second flush with empty is no-op
        assert!(t.flush_tokens());
        assert_eq!(sent.lock().unwrap().len(), 2);
    }

    #[test]
    fn ws_transport_write_async_drains_ahead() {
        let (sender, sent) = MockSender::new();
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let delta = r#"{"params":{"type":"message.delta"}}"#;
        t.write(delta);
        t.write(delta);
        assert_eq!(t.pending_len(), 2);
        let ready = r#"{"method":"event","params":{"type":"gateway.ready"}}"#;
        assert!(t.write_async(ready));
        assert_eq!(t.pending_len(), 0);
        let s = sent.lock().unwrap().clone();
        assert_eq!(s.len(), 3);
        // pending tokens ahead of ready -> ordering preserved
        assert!(s[0].contains("message.delta"));
        assert!(s[2].contains("gateway.ready"));
    }

    #[test]
    fn ws_transport_write_from_loop() {
        let (sender, sent) = MockSender::new();
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let delta = r#"{"params":{"type":"reasoning.delta"}}"#;
        assert!(t.write_from_loop(delta));
        assert_eq!(t.pending_len(), 1);
        let resp = r#"{"id":1}"#;
        assert!(t.write_from_loop(resp));
        assert_eq!(t.pending_len(), 0);
        assert_eq!(sent.lock().unwrap().len(), 2);
    }

    #[test]
    fn ws_transport_send_failure_latches_closed() {
        let (sender, sent) = MockSender::failing();
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let line = r#"{"id":1}"#;
        assert!(!t.write(line));
        assert!(t.is_closed());
        // subsequent write short-circuits
        assert!(!t.write(line));
        assert!(sent.lock().unwrap().is_empty());
    }

    #[test]
    fn ws_transport_close_cancels_flush() {
        let (sender, _sent) = MockSender::new();
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let delta = r#"{"params":{"type":"message.delta"}}"#;
        t.write(delta);
        assert!(t.is_flush_armed());
        t.close();
        assert!(t.is_closed());
        assert!(!t.is_flush_armed());
        assert!(!t.has_flush_handle());
        assert!(!t.write(delta));
        assert!(!t.write_async(delta));
        // flush after close is no-op (returns true, does not send)
        assert!(t.flush_tokens());
    }

    #[test]
    fn ws_transport_send_many_holds_lock_and_latches() {
        let (sender, sent) = MockSender::failing_on("boom");
        let t = WsTransport::new("peer", None, Arc::new(sender));
        let batch = vec!["ok1".to_string(), "boom".to_string(), "ok2".to_string()];
        let res = t.send_many(&batch);
        assert!(res.is_err());
        assert!(t.is_closed());
        // ok1 sent, boom failed, ok2 not sent (closed latch while holding lock)
        let s = sent.lock().unwrap().clone();
        assert_eq!(s, vec!["ok1"]);
        // further send_many is Ok (early return when closed)
        assert!(t.send_many(&vec!["ok3".to_string()]).is_ok());
        assert!(sent.lock().unwrap().len() == 1);
    }

    // -- handle_ws helpers --------------------------------------------------

    #[test]
    fn gateway_ready_payload_serializes() {
        let p = GatewayReadyPayload::new(r#"{"theme":"dark"}"#);
        let j = p.to_json();
        assert!(j.contains("gateway.ready"));
        assert!(j.contains("change_events"));
        assert!(j.contains("dark"));
    }

    #[test]
    fn parse_and_internal_error_responses() {
        assert!(parse_error_response().contains("-32700"));
        assert!(parse_error_response().contains("parse error"));
        assert!(internal_error_response(None).contains("-32603"));
        assert!(internal_error_response(Some("5")).contains("\"id\":5"));
        assert!(internal_error_response(None).contains("\"id\":null"));
    }

    #[test]
    fn should_skip_line() {
        assert!(should_skip_line(""));
        assert!(should_skip_line("   "));
        assert!(should_skip_line("\n\t "));
        assert!(!should_skip_line(r#"{"a":1}"#));
        assert!(!should_skip_line("  x  "));
    }

    #[test]
    fn extract_id_and_method() {
        let (id, method) = extract_id_and_method(r#"{"jsonrpc":"2.0","id":42,"method":"session.list"}"#);
        assert_eq!(id.as_deref(), Some("42"));
        assert_eq!(method.as_deref(), Some("\"session.list\""));
        let (id2, method2) = extract_id_and_method(r#"{"id":null,"method":"ping"}"#);
        assert_eq!(id2.as_deref(), Some("null"));
        assert_eq!(method2.as_deref(), Some("\"ping\""));
    }

    #[test]
    fn ws_session_step_receive() {
        let mut s = WsSession::new("peer");
        assert_eq!(s.disconnect_reason, DisconnectReason::NotConnected);
        s.on_accept();
        assert!(s.accepted);
        assert_eq!(s.disconnect_reason, DisconnectReason::Connected);

        assert_eq!(s.step_receive("   "), WsReceiveAction::Skip);
        assert_eq!(s.stats.messages, 0);

        let act = s.step_receive(r#"not json"#);
        assert!(matches!(act, WsReceiveAction::ParseError { .. }));
        assert_eq!(s.stats.messages, 1);
        assert_eq!(s.stats.parse_errors, 1);

        let act2 = s.step_receive(r#"{"method":"ping","id":1}"#);
        assert!(matches!(act2, WsReceiveAction::Dispatch { .. }));
        assert_eq!(s.stats.messages, 2);

        s.on_dispatch_crash();
        s.on_send_failure();
        assert_eq!(s.stats.dispatch_crashes, 1);
        assert_eq!(s.stats.send_failures, 1);

        let log = s.close_log();
        assert!(log.contains("peer=peer"));
        assert!(log.contains("messages=2"));
    }

    #[test]
    fn ws_disconnect_reason_strings() {
        let d = WsDisconnect::Disconnect { code: Some(1000), reason: Some("bye".into()) };
        assert!(d.to_reason_string().contains("client_disconnect"));
        assert!(d.to_reason_string().contains("1000"));
        let r = DisconnectReason::ClientDisconnect { code: Some(1000), reason: Some("bye".into()) };
        assert!(r.as_str().contains("client_disconnect"));
        assert_eq!(DisconnectReason::NotConnected.as_str(), "not_connected");
        assert_eq!(DisconnectReason::ReadySendFailed.as_str(), "ready_send_failed");
    }

    #[test]
    fn handle_ws_config_defaults() {
        let cfg = HandleWsConfig::default();
        assert!(cfg.subprotocol.is_none());
        assert!(cfg.auth_identity.is_none());
        let cfg2 = HandleWsConfig { subprotocol: Some("jsonrpc".into()), auth_identity: Some("{}".into()) };
        assert_eq!(cfg2.subprotocol.as_deref(), Some("jsonrpc"));
    }
}
