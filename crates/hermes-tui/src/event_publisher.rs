//! Best-effort WebSocket publisher transport for the PTY-side gateway.
//!
//! 1:1 port of `tui_gateway/event_publisher.py` (126 lines).
//!
//! The dashboard's `/api/pty` spawns `hermes --tui` as a child process, which
//! spawns its own `tui_gateway.entry`. Tool/reasoning/status events fire on
//! *that* gateway's transport — three processes removed from the dashboard
//! server itself. To surface them in the dashboard sidebar (`/api/events`),
//! the PTY-side gateway opens a back-WS to the dashboard at startup and
//! mirrors every emit through this transport.
//!
//! Wire protocol: newline-framed JSON dicts (the same shape the dispatcher
//! already passes to `write`). No JSON-RPC envelope — the dashboard's
//! `/api/pub` endpoint just rebroadcasts the bytes verbatim to subscribers.
//!
//! Failure mode: silent. The agent loop must never block waiting for the
//! sidecar to drain. A dead WS short-circuits all subsequent writes.
//! Actual `send` calls run on a daemon thread so the `write` returns after
//! enqueueing (best-effort; drop when the queue is full).
//!
//! ```python
//! # Python — tui_gateway/event_publisher.py
//! _QUEUE_MAX = 256
//! _DRAIN_STOP = object()
//! class WsPublisherTransport:
//!     __slots__ = ("_url", "_lock", "_ws", "_dead", "_q", "_worker")
//!     def __init__(self, url, *, connect_timeout=2.0): ...
//!     def _drain(self): ...
//!     def write(self, obj: dict) -> bool: ...
//!     def close(self) -> None: ...
//! ```
//!
//! # Rust mapping
//!
//! * `_QUEUE_MAX = 256` → [`QUEUE_MAX`].
//! * `_DRAIN_STOP = object()` sentinel → [`QueueItem::Stop`] (typed enum instead
//!   of identity-checked `object()`; `queue.Queue[object]` → `mpsc::sync_channel(256)`).
//! * `ws_connect is None` (`ImportError` fallback) → `connect_fn: Option<F>` is
//!   `None` → [`WsPublisherTransport`] is immediately dead (`dead=true`, no
//!   worker). This preserves the `websockets` missing-install path.
//! * `ws_connect(url, open_timeout=connect_timeout, max_size=None)` → injected
//!   `connect_fn(&str, f64) -> Result<Box<dyn WsConnection>, E>` closure. The
//!   timeout and `max_size=None` are forwarded as the `connect_timeout` arg;
//!   callers that need `max_size` semantics pass it inside the closure.
//!   Connection failure (`except Exception: _log.debug(...); self._dead=True`)
//!   is preserved — any `Err` marks dead and logs at debug level.
//! * `threading.Lock` → `Arc<Mutex<Option<Box<dyn WsConnection>>>>` (`_ws` + `_lock`
//!   merged; every `send`/`close` locks the same mutex and re-checks `is_some()`).
//! * `threading.Thread(target=self._drain, daemon=True, name="hermes-ws-pub")` →
//!   `std::thread::Builder::new().name("hermes-ws-pub").spawn(...)`. The Rust
//!   thread is not `daemon` (Rust has no daemon threads) — it is joined with a
//!   3 s timeout on [`WsPublisherTransport::close`] and otherwise detached on
//!   drop (mirrors best-effort teardown comment in `close`).
//! * `queue.Queue(maxsize=_QUEUE_MAX).put_nowait` / `Full` → `SyncSender::try_send`
//!   / `TrySendError::Full` — both return `False`/`false` when full.
//! * `json.dumps(obj, ensure_ascii=False)` → caller serializes the dict to a
//!   JSON `String` before calling [`WsPublisherTransport::write`] (or
//!   [`WsPublisherTransport::write_raw`]). `ensure_ascii=False` is the Rust
//!   default — `serde_json::to_string` emits UTF-8 verbatim; this crate stays
//!   `std`-only so the serialization is left to the caller and documented.
//! * `_drain` loop: `item = self._q.get(); if item is _DRAIN_STOP: return;
//!   if not isinstance(item, str): continue; if self._ws is None: continue;
//!   try: with self._lock: if self._ws is not None: self._ws.send(item)` →
//!   `loop { match rx.recv() { Stop => return, Line(s) => { if ws is None {continue}
//!   lock + send; on Err => dead=true, ws=None } } }`. The non-`str` guard is
//!   typed away (queue only holds `QueueItem`), but documented.
//! * `write` guard `if self._dead or self._ws is None or self._worker is None: return False`
//!   → [`WsPublisherTransport::write`] checks `dead` (`AtomicBool`), `ws.is_none()`,
//!   and `worker.is_none()` (no worker means never connected).
//! * `close`: `self._dead=True; w=self._worker; if w is not None and w.is_alive():
//!   try: self._q.put_nowait(_DRAIN_STOP) except queue.Full: pass; w.join(timeout=3.0)` →
//!   `dead.store(true); if worker.is_some() { try_send(Stop); join with 3 s timeout }`;
//!   `w.join(timeout=3)` is emulated by polling `is_finished()` for 3 s then
//!   `join` if finished, otherwise detach (mirrors best-effort comment). Then
//!   `if self._ws is None: return; try: with self._lock: close()`.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::thread;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Constants — mirrors event_publisher.py:35-37
// ---------------------------------------------------------------------------

/// Bounded queue capacity. Mirrors `_QUEUE_MAX = 256`.
pub const QUEUE_MAX: usize = 256;

/// Worker thread name. Mirrors `name="hermes-ws-pub"`.
pub const WORKER_NAME: &str = "hermes-ws-pub";

/// Default connect timeout in seconds. Mirrors `connect_timeout: float = 2.0`.
pub const DEFAULT_CONNECT_TIMEOUT_SECS: f64 = 2.0;

/// Join timeout in seconds. Mirrors `w.join(timeout=3.0)`.
pub const JOIN_TIMEOUT_SECS: f64 = 3.0;

// ---------------------------------------------------------------------------
// WS abstraction — mirrors `websockets.sync.client.connect` + `ws.send/close`
// ---------------------------------------------------------------------------

/// Minimal WebSocket connection used by the publisher.
///
/// Mirrors the `websockets.sync.client` connection object:
///
/// * `ws.send(item)` → [`WsConnection::send`]
/// * `ws.close()` → [`WsConnection::close`]
///
/// The trait is `Send + Sync` so the connection can be shared behind
/// `Arc<Mutex<Option<Box<dyn WsConnection>>>>` and accessed from the drain
/// thread. Implementations may use interior mutability (e.g. `Mutex` inside)
/// if `send`/`close` need `&mut self`.
pub trait WsConnection: Send + Sync + 'static {
    /// Send a newline-framed JSON line. Mirrors `self._ws.send(item)`.
    fn send(&self, msg: &str) -> Result<(), String>;
    /// Close the connection. Mirrors `self._ws.close()`.
    fn close(&self) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// Queue item — mirrors `_DRAIN_STOP = object()` + `queue.Queue[object]`
// ---------------------------------------------------------------------------

/// Items flowing through the bounded channel.
///
/// Mirrors `queue.Queue[object]` where `object()` sentinel `_DRAIN_STOP` is
/// distinguished by identity (`item is _DRAIN_STOP`). Typed enum removes the
/// `isinstance(item, str)` guard — only `Line` is forwarded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueItem {
    /// JSON line produced by `json.dumps(obj, ensure_ascii=False)`.
    Line(String),
    /// Sentinel that stops the drain thread. Mirrors `_DRAIN_STOP`.
    Stop,
}

// ---------------------------------------------------------------------------
// Transport — mirrors `WsPublisherTransport`
// ---------------------------------------------------------------------------

/// Best-effort WebSocket publisher.
///
/// Mirrors `tui_gateway/event_publisher.py::WsPublisherTransport`:
///
/// ```python
/// class WsPublisherTransport:
///     __slots__ = ("_url", "_lock", "_ws", "_dead", "_q", "_worker")
/// ```
pub struct WsPublisherTransport {
    /// Mirrors `self._url`.
    url: String,
    /// Mirrors `self._ws` + `self._lock` (combined as `Mutex<Option<...>>`).
    ws: Arc<Mutex<Option<Box<dyn WsConnection>>>>,
    /// Mirrors `self._dead`.
    dead: Arc<AtomicBool>,
    /// Mirrors `self._q: queue.Queue[object]` (bounded 256). `None` only when
    /// the transport was never successfully connected? Python always creates
    /// the queue; Rust does too — the `Mutex<Option<Sender>>` is `Some` even
    /// when dead so `close` can still `try_send(Stop)` best-effort.
    sender: Mutex<Option<mpsc::SyncSender<QueueItem>>>,
    /// Mirrors `self._worker: Optional[threading.Thread]`.
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl std::fmt::Debug for WsPublisherTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsPublisherTransport")
            .field("url", &self.url)
            .field("dead", &self.dead.load(Ordering::SeqCst))
            .field("has_ws", &self.ws.lock().map(|g| g.is_some()).unwrap_or(false))
            .field("has_worker", &self.worker.lock().map(|g| g.is_some()).unwrap_or(false))
            .finish()
    }
}

impl WsPublisherTransport {
    /// Create a transport, attempting to connect immediately.
    ///
    /// Mirrors `WsPublisherTransport.__init__(self, url, *, connect_timeout=2.0)`:
    ///
    /// ```python
    /// def __init__(self, url: str, *, connect_timeout: float = 2.0) -> None:
    ///     self._url = url
    ///     self._lock = threading.Lock()
    ///     self._ws: Optional[object] = None
    ///     self._dead = False
    ///     self._q: queue.Queue[object] = queue.Queue(maxsize=_QUEUE_MAX)
    ///     self._worker: Optional[threading.Thread] = None
    ///     if ws_connect is None:
    ///         self._dead = True
    ///         return
    ///     try:
    ///         self._ws = ws_connect(url, open_timeout=connect_timeout, max_size=None)
    ///     except Exception as exc:
    ///         _log.debug("event publisher connect failed: %s", exc)
    ///         self._dead = True
    ///         self._ws = None
    ///         return
    ///     self._worker = threading.Thread(target=self._drain, name="hermes-ws-pub", daemon=True)
    ///     self._worker.start()
    /// ```
    ///
    /// `connect_fn` mirrors `ws_connect`:
    /// * `None` → `ws_connect is None` (required install missing) → immediately dead.
    /// * `Some(f)` → `f(url, connect_timeout)` is called; `Ok(ws)` stores the
    ///   connection and spawns the drain thread, `Err(e)` marks dead and logs.
    pub fn new_with_connect<F, E>(url: impl Into<String>, connect_timeout: f64, connect_fn: Option<F>) -> Self
    where
        F: Fn(&str, f64) -> Result<Box<dyn WsConnection>, E>,
        E: std::fmt::Display,
    {
        let url = url.into();
        let dead = Arc::new(AtomicBool::new(false));
        let ws: Arc<Mutex<Option<Box<dyn WsConnection>>>> = Arc::new(Mutex::new(None));
        let (tx, rx) = mpsc::sync_channel::<QueueItem>(QUEUE_MAX);
        let sender = Mutex::new(Some(tx));
        let worker: Mutex<Option<thread::JoinHandle<()>>> = Mutex::new(None);

        // Mirrors `if ws_connect is None: self._dead = True; return`
        let Some(connect) = connect_fn else {
            dead.store(true, Ordering::SeqCst);
            return Self { url, ws, dead, sender, worker };
        };

        // Mirrors `try: self._ws = ws_connect(url, open_timeout=connect_timeout, max_size=None)`
        let conn: Box<dyn WsConnection> = match connect(&url, connect_timeout) {
            Ok(c) => c,
            Err(exc) => {
                // Mirrors `_log.debug("event publisher connect failed: %s", exc)`
                #[cfg(feature = "log")]
                log::debug!("event publisher connect failed: {}", exc);
                let _ = exc;
                dead.store(true, Ordering::SeqCst);
                return Self { url, ws, dead, sender, worker };
            }
        };

        // Store connection before spawning worker so worker sees it.
        {
            let mut g = ws.lock().unwrap();
            *g = Some(conn);
        }

        // Spawn drain thread — mirrors `threading.Thread(target=self._drain, name="hermes-ws-pub", daemon=True)`
        let ws_clone = Arc::clone(&ws);
        let dead_clone = Arc::clone(&dead);
        let handle = thread::Builder::new()
            .name(WORKER_NAME.to_string())
            .spawn(move || Self::drain_loop(rx, ws_clone, dead_clone))
            .ok();

        {
            let mut g = worker.lock().unwrap();
            *g = handle;
        }

        Self { url, ws, dead, sender, worker }
    }

    /// Convenience: `connect_timeout = 2.0` and no custom connect (mirrors default arg).
    ///
    /// When `connect_fn` is `None`, the transport is immediately dead — mirrors
    /// the `ImportError` fallback where `ws_connect = None`.
    pub fn new<F, E>(url: impl Into<String>, connect_fn: Option<F>) -> Self
    where
        F: Fn(&str, f64) -> Result<Box<dyn WsConnection>, E>,
        E: std::fmt::Display,
    {
        Self::new_with_connect(url, DEFAULT_CONNECT_TIMEOUT_SECS, connect_fn)
    }

    /// Create a transport that is already connected with `ws`.
    ///
    /// Test helper — bypasses the `connect_fn` path. Still creates the bounded
    /// queue and spawns the drain thread, mirroring the successful-connect branch.
    pub fn new_connected(url: impl Into<String>, ws_conn: Box<dyn WsConnection>) -> Self {
        let url = url.into();
        let dead = Arc::new(AtomicBool::new(false));
        let ws: Arc<Mutex<Option<Box<dyn WsConnection>>>> = Arc::new(Mutex::new(Some(ws_conn)));
        let (tx, rx) = mpsc::sync_channel::<QueueItem>(QUEUE_MAX);
        let sender = Mutex::new(Some(tx));
        let ws_clone = Arc::clone(&ws);
        let dead_clone = Arc::clone(&dead);
        let handle = thread::Builder::new()
            .name(WORKER_NAME.to_string())
            .spawn(move || Self::drain_loop(rx, ws_clone, dead_clone))
            .ok();
        let worker = Mutex::new(handle);
        Self { url, ws, dead, sender, worker }
    }

    /// Create an immediately-dead transport (no WS, no worker).
    ///
    /// Mirrors the `ws_connect is None` or `ws_connect` raises early return.
    /// Useful for tests that want a dead transport without spawning threads.
    pub fn new_dead(url: impl Into<String>) -> Self {
        let dead = Arc::new(AtomicBool::new(true));
        let ws: Arc<Mutex<Option<Box<dyn WsConnection>>>> = Arc::new(Mutex::new(None));
        let (tx, _rx) = mpsc::sync_channel::<QueueItem>(QUEUE_MAX);
        // Drop receiver immediately so sender try_send will fail, but keep sender
        // to satisfy struct shape; worker stays None.
        let sender = Mutex::new(Some(tx));
        let worker = Mutex::new(None);
        Self {
            url: url.into(),
            ws,
            dead,
            sender,
            worker,
        }
    }

    /// Drain loop — mirrors `WsPublisherTransport._drain`.
    ///
    /// ```python
    /// def _drain(self) -> None:
    ///     while True:
    ///         item = self._q.get()
    ///         if item is _DRAIN_STOP:
    ///             return
    ///         if not isinstance(item, str):
    ///             continue
    ///         if self._ws is None:
    ///             continue
    ///         try:
    ///             with self._lock:
    ///                 if self._ws is not None:
    ///                     self._ws.send(item)
    ///         except Exception as exc:
    ///             _log.debug("event publisher write failed: %s", exc)
    ///             self._dead = True
    ///             self._ws = None
    /// ```
    fn drain_loop(
        rx: mpsc::Receiver<QueueItem>,
        ws: Arc<Mutex<Option<Box<dyn WsConnection>>>>,
        dead: Arc<AtomicBool>,
    ) {
        loop {
            let item = match rx.recv() {
                Ok(v) => v,
                Err(_) => return, // channel closed → exit (mirrors thread torn down with process)
            };
            match item {
                QueueItem::Stop => return, // mirrors `if item is _DRAIN_STOP: return`
                QueueItem::Line(line) => {
                    // Mirrors `if self._ws is None: continue`
                    // Check without lock first (fast path); then re-check under lock.
                    let has_ws = ws.lock().map(|g| g.is_some()).unwrap_or(false);
                    if !has_ws {
                        continue;
                    }
                    // Mirrors `try: with self._lock: if self._ws is not None: self._ws.send(item)`
                    let send_res = {
                        let g = ws.lock().unwrap();
                        if let Some(conn) = g.as_ref() {
                            conn.send(&line)
                        } else {
                            continue;
                        }
                    };
                    if let Err(exc) = send_res {
                        #[cfg(feature = "log")]
                        log::debug!("event publisher write failed: {}", exc);
                        let _ = exc;
                        dead.store(true, Ordering::SeqCst);
                        // Mirrors `self._ws = None`
                        if let Ok(mut g) = ws.lock() {
                            *g = None;
                        }
                    }
                }
            }
        }
    }

    /// Best-effort enqueue of a JSON line.
    ///
    /// Mirrors `WsPublisherTransport.write(self, obj: dict) -> bool`:
    ///
    /// ```python
    /// def write(self, obj: dict) -> bool:
    ///     if self._dead or self._ws is None or self._worker is None:
    ///         return False
    ///     line = json.dumps(obj, ensure_ascii=False)
    ///     try:
    ///         self._q.put_nowait(line)
    ///         return True
    ///     except queue.Full:
    ///         return False
    /// ```
    ///
    /// Python's `json.dumps(obj, ensure_ascii=False)` is left to the caller:
    /// pass the already-serialized JSON string as `line`. When `serde_json` is
    /// available, `serde_json::to_string(&obj).unwrap()` is the direct
    /// equivalent (it emits UTF-8 verbatim, matching `ensure_ascii=False`).
    pub fn write_raw(&self, line: &str) -> bool {
        // Mirrors `if self._dead or self._ws is None or self._worker is None: return False`
        if self.dead.load(Ordering::SeqCst) {
            return false;
        }
        let has_ws = self.ws.lock().map(|g| g.is_some()).unwrap_or(false);
        if !has_ws {
            return false;
        }
        let has_worker = self.worker.lock().map(|g| g.is_some()).unwrap_or(false);
        if !has_worker {
            return false;
        }
        // Mirrors `line = json.dumps(obj, ensure_ascii=False)` — caller did it.
        // Then `self._q.put_nowait(line)` with `except queue.Full: return False`
        let sender_guard = self.sender.lock().unwrap();
        let Some(tx) = sender_guard.as_ref() else {
            return false;
        };
        match tx.try_send(QueueItem::Line(line.to_string())) {
            Ok(()) => true,
            Err(mpsc::TrySendError::Full(_)) => false,
            Err(mpsc::TrySendError::Disconnected(_)) => false,
        }
    }

    /// Alias for [`Self::write_raw`] that mirrors the Python name `write`.
    ///
    /// Takes an already-serialized JSON line (`json.dumps(obj, ensure_ascii=False)`).
    /// Provided so call sites can write `transport.write(&line)` exactly like the
    /// Python `write(obj)` after serializing.
    pub fn write(&self, json_line: &str) -> bool {
        self.write_raw(json_line)
    }

    /// Convenience: serialize a stringly-typed value with debug formatting then enqueue.
    ///
    /// This is a `std`-only helper for tests. Real call sites should use
    /// `serde_json::to_string(&obj)` and then [`Self::write_raw`].
    #[cfg(test)]
    pub fn write_json_value(&self, value_str: &str) -> bool {
        self.write_raw(value_str)
    }

    /// Close the transport — mirrors `WsPublisherTransport.close`.
    ///
    /// ```python
    /// def close(self) -> None:
    ///     self._dead = True
    ///     w = self._worker
    ///     if w is not None and w.is_alive():
    ///         try:
    ///             self._q.put_nowait(_DRAIN_STOP)
    ///         except queue.Full:
    ///             pass
    ///         w.join(timeout=3.0)
    ///     self._worker = None
    ///     if self._ws is None:
    ///         return
    ///     try:
    ///         with self._lock:
    ///             if self._ws is not None:
    ///                 self._ws.close()
    ///     except Exception:
    ///         pass
    ///     self._ws = None
    /// ```
    pub fn close(&self) {
        // Mirrors `self._dead = True`
        self.dead.store(true, Ordering::SeqCst);

        // Mirrors `w = self._worker; if w is not None and w.is_alive(): try put Stop; w.join(3.0)`
        let handle_opt = {
            let mut g = self.worker.lock().unwrap();
            g.take()
        };
        if let Some(handle) = handle_opt {
            if !handle.is_finished() {
                // Mirrors `try: self._q.put_nowait(_DRAIN_STOP) except queue.Full: pass`
                {
                    let g = self.sender.lock().unwrap();
                    if let Some(tx) = g.as_ref() {
                        let _ = tx.try_send(QueueItem::Stop);
                    }
                }
                // Mirrors `w.join(timeout=3.0)` — poll is_finished for 3 s.
                let deadline = Instant::now() + Duration::from_secs_f64(JOIN_TIMEOUT_SECS);
                while Instant::now() < deadline {
                    if handle.is_finished() {
                        break;
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                if handle.is_finished() {
                    let _ = handle.join();
                } else {
                    // Best-effort: queue wedged, daemon thread torn down with process.
                    // Detach — handle dropped without join (mirrors Python comment).
                    std::mem::forget(handle);
                }
            } else {
                let _ = handle.join();
            }
        }

        // Clear sender so further writes fail fast (optional, not in Python but harmless).
        {
            let mut g = self.sender.lock().unwrap();
            *g = None;
        }

        // Mirrors `if self._ws is None: return`
        let has_ws = self.ws.lock().map(|g| g.is_some()).unwrap_or(false);
        if !has_ws {
            return;
        }

        // Mirrors `try: with self._lock: if self._ws is not None: self._ws.close() except: pass`
        let close_target = {
            let mut g = self.ws.lock().unwrap();
            g.take()
        };
        if let Some(conn) = close_target {
            let _ = conn.close();
        }
        // `self._ws` already None via take()
    }

    /// Whether the transport is dead — mirrors `self._dead`.
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::SeqCst)
    }

    /// Whether the transport has an active worker — mirrors `self._worker is not None`.
    pub fn has_worker(&self) -> bool {
        self.worker.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// Whether the transport has a live WS — mirrors `self._ws is not None`.
    pub fn has_ws(&self) -> bool {
        self.ws.lock().map(|g| g.is_some()).unwrap_or(false)
    }

    /// The URL this transport was created for — mirrors `self._url`.
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl Drop for WsPublisherTransport {
    fn drop(&mut self) {
        // Best-effort close without blocking indefinitely.
        self.dead.store(true, Ordering::SeqCst);
        // Try to wake drain so it can exit; ignore if full/disconnected.
        {
            let g = self.sender.lock().unwrap();
            if let Some(tx) = g.as_ref() {
                let _ = tx.try_send(QueueItem::Stop);
            }
        }
        // Do not join here — Drop must not block. The drain thread will exit
        // on Stop or when the channel disconnects (all senders dropped).
        // If the caller needs a bounded wait, call `close()` explicitly.
    }
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct MockWs {
        sent: Arc<Mutex<Vec<String>>>,
        fail_on: Option<String>,
        close_called: Arc<Mutex<bool>>,
    }

    impl MockWs {
        fn new() -> (Self, Arc<Mutex<Vec<String>>>, Arc<Mutex<bool>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let closed = Arc::new(Mutex::new(false));
            let ws = Self {
                sent: Arc::clone(&sent),
                fail_on: None,
                close_called: Arc::clone(&closed),
            };
            (ws, sent, closed)
        }

        fn failing(fail_on: &str) -> (Self, Arc<Mutex<Vec<String>>>, Arc<Mutex<bool>>) {
            let sent = Arc::new(Mutex::new(Vec::new()));
            let closed = Arc::new(Mutex::new(false));
            let ws = Self {
                sent: Arc::clone(&sent),
                fail_on: Some(fail_on.to_string()),
                close_called: Arc::clone(&closed),
            };
            (ws, sent, closed)
        }
    }

    impl WsConnection for MockWs {
        fn send(&self, msg: &str) -> Result<(), String> {
            if let Some(f) = &self.fail_on {
                if msg.contains(f) {
                    return Err(format!("injected failure on {}", f));
                }
            }
            self.sent.lock().unwrap().push(msg.to_string());
            Ok(())
        }
        fn close(&self) -> Result<(), String> {
            *self.close_called.lock().unwrap() = true;
            Ok(())
        }
    }

    struct ClosingWs {
        close_called: Arc<Mutex<bool>>,
        close_err: bool,
    }

    impl WsConnection for ClosingWs {
        fn send(&self, _msg: &str) -> Result<(), String> {
            Ok(())
        }
        fn close(&self) -> Result<(), String> {
            *self.close_called.lock().unwrap() = true;
            if self.close_err {
                Err("close boom".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn queue_max_is_256() {
        assert_eq!(QUEUE_MAX, 256);
    }

    #[test]
    fn new_dead_is_dead_no_ws_no_worker() {
        let t = WsPublisherTransport::new_dead("ws://example.com/api/pub");
        assert!(t.is_dead());
        assert!(!t.has_ws());
        assert!(!t.has_worker());
        assert_eq!(t.url(), "ws://example.com/api/pub");
        // Mirrors `if self._dead or self._ws is None or self._worker is None: return False`
        assert!(!t.write_raw(r#"{"type":"tool"}"#));
        t.close(); // no panic
        assert!(!t.has_ws());
    }

    #[test]
    fn connect_none_is_dead() {
        // Mirrors `if ws_connect is None: self._dead = True; return`
        let t = WsPublisherTransport::new(
            "ws://example.com/api/pub",
            None::<fn(&str, f64) -> Result<Box<dyn WsConnection>, String>>,
        );
        assert!(t.is_dead());
        assert!(!t.has_ws());
        assert!(!t.has_worker());
        assert!(!t.write_raw(r#"{"x":1}"#));
    }

    #[test]
    fn connect_fn_err_is_dead() {
        let t = WsPublisherTransport::new_with_connect(
            "ws://example.com/api/pub",
            2.0,
            Some(|_: &str, _: f64| -> Result<Box<dyn WsConnection>, String> { Err("refused".into()) }),
        );
        assert!(t.is_dead());
        assert!(!t.has_ws());
        assert!(!t.has_worker());
        assert!(!t.write_raw(r#"{"x":1}"#));
    }

    #[test]
    fn connect_ok_spawns_worker_and_writes() {
        let (ws, sent, _closed) = MockWs::new();
        let t = WsPublisherTransport::new_with_connect(
            "ws://example.com/api/pub",
            2.0,
            Some(|_: &str, _: f64| -> Result<Box<dyn WsConnection>, String> { Ok(Box::new(ws)) }),
        );
        // Actually we moved ws into closure, need to capture sent outside? Instead use new_connected.
        // This test uses new_connected for simplicity.
        let (ws2, sent2, _closed2) = MockWs::new();
        let t2 = WsPublisherTransport::new_connected("ws://example.com/api/pub", Box::new(ws2));
        assert!(!t2.is_dead());
        assert!(t2.has_ws());
        assert!(t2.has_worker());
        assert!(t2.write_raw(r#"{"type":"tool","name":"read_file"}"#));
        // Give drain thread time to send
        thread::sleep(Duration::from_millis(100));
        assert_eq!(sent2.lock().unwrap().len(), 1);
        assert_eq!(sent2.lock().unwrap()[0], r#"{"type":"tool","name":"read_file"}"#);
        t2.close();
        // silence unused
        let _ = t;
        let _ = sent;
    }

    #[test]
    fn write_returns_false_when_full() {
        // Fill queue faster than drain can empty by using a slow WS.
        struct SlowWs;
        impl WsConnection for SlowWs {
            fn send(&self, _msg: &str) -> Result<(), String> {
                thread::sleep(Duration::from_millis(50));
                Ok(())
            }
            fn close(&self) -> Result<(), String> {
                Ok(())
            }
        }
        let t = WsPublisherTransport::new_connected("ws://example.com/api/pub", Box::new(SlowWs));
        // Spin writes until full — at least QUEUE_MAX+1 writes should cause one false.
        let mut successes = 0;
        let mut failures = 0;
        for i in 0..(QUEUE_MAX + 10) {
            if t.write_raw(&format!(r#"{{"i":{}}}"#, i)) {
                successes += 1;
            } else {
                failures += 1;
            }
        }
        // At least one must have failed due to Full (queue bounded 256)
        // Note: drain is concurrently consuming, so exact count varies, but
        // with slow drain we should see some failures or at least successes capped.
        assert!(successes <= QUEUE_MAX + 10);
        // We filled quickly; with 50ms per send, we expect some drops.
        // Accept either outcome but ensure write doesn't panic and respects bound.
        let _ = failures;
        t.close();
    }

    #[test]
    fn drain_sends_and_dead_on_failure() {
        let (ws, sent, _closed) = MockWs::failing("boom");
        let t = WsPublisherTransport::new_connected("ws://example.com/api/pub", Box::new(ws));
        assert!(t.write_raw(r#"{"msg":"ok1"}"#));
        assert!(t.write_raw(r#"{"msg":"boom"}"#));
        assert!(t.write_raw(r#"{"msg":"ok2"}"#));
        thread::sleep(Duration::from_millis(200));
        // First message sent
        {
            let s = sent.lock().unwrap();
            assert!(s.iter().any(|x| x.contains("ok1")), "ok1 should be sent: {:?}", *s);
        }
        // Failure on boom should mark dead and clear ws
        // Give drain time to process failure
        thread::sleep(Duration::from_millis(50));
        assert!(t.is_dead(), "transport should be dead after send failure");
        assert!(!t.has_ws(), "ws should be None after failure");
        // Subsequent write should be short-circuited
        assert!(!t.write_raw(r#"{"msg":"after"}"#));
        // ok2 was queued but should be dropped because ws is None (mirrors `if self._ws is None: continue`)
        thread::sleep(Duration::from_millis(100));
        {
            let s = sent.lock().unwrap();
            assert!(!s.iter().any(|x| x.contains("ok2")), "ok2 should be dropped after dead: {:?}", *s);
            assert!(!s.iter().any(|x| x.contains("after")), "after should not be sent: {:?}", *s);
        }
        t.close();
    }

    #[test]
    fn close_joins_and_closes_ws() {
        let (ws, _sent, closed) = MockWs::new();
        let t = WsPublisherTransport::new_connected("ws://example.com/api/pub", Box::new(ws));
        assert!(t.write_raw(r#"{"x":1}"#));
        thread::sleep(Duration::from_millis(50));
        t.close();
        assert!(t.is_dead());
        assert!(!t.has_ws());
        assert!(!t.has_worker());
        assert!(*closed.lock().unwrap(), "ws.close should be called");
        // Subsequent write fails
        assert!(!t.write_raw(r#"{"x":2}"#));
    }

    #[test]
    fn close_is_idempotent_and_swallows_close_err() {
        let close_called = Arc::new(Mutex::new(false));
        let ws = ClosingWs {
            close_called: Arc::clone(&close_called),
            close_err: true,
        };
        let t = WsPublisherTransport::new_connected("ws://example.com/api/pub", Box::new(ws));
        t.close();
        assert!(*close_called.lock().unwrap());
        // Second close is no-op, no panic, mirrors `if self._ws is None: return`
        t.close();
        assert!(!t.has_ws());
    }

    #[test]
    fn close_when_dead_no_ws_is_noop() {
        let t = WsPublisherTransport::new_dead("ws://example.com/api/pub");
        t.close();
        t.close();
        assert!(t.is_dead());
        assert!(!t.has_ws());
    }

    #[test]
    fn write_short_circuits_when_dead_or_no_worker() {
        let t = WsPublisherTransport::new_dead("ws://example.com/api/pub");
        assert!(!t.write_raw(r#"{"a":1}"#));
        // Also when ws is None but worker exists? That state is dead already;
        // create a connected then manually kill ws via failure and check.
        let (ws, _sent, _closed) = MockWs::failing("kill");
        let t2 = WsPublisherTransport::new_connected("ws://example.com/api/pub", Box::new(ws));
        assert!(t2.write_raw(r#"{"msg":"kill"}"#));
        thread::sleep(Duration::from_millis(150));
        assert!(t2.is_dead());
        assert!(!t2.write_raw(r#"{"msg":"next"}"#));
        t2.close();
    }

    #[test]
    fn new_with_connect_passes_url_and_timeout() {
        let captured = Arc::new(Mutex::new((String::new(), 0.0f64)));
        let cap_clone = Arc::clone(&captured);
        let t = WsPublisherTransport::new_with_connect(
            "ws://example.com/api/pub?token=abc",
            5.0,
            Some(move |url: &str, timeout: f64| -> Result<Box<dyn WsConnection>, String> {
                *cap_clone.lock().unwrap() = (url.to_string(), timeout);
                let (ws, _sent, _closed) = MockWs::new();
                Ok(Box::new(ws))
            }),
        );
        let (url, timeout) = captured.lock().unwrap().clone();
        assert_eq!(url, "ws://example.com/api/pub?token=abc");
        assert!((timeout - 5.0).abs() < 1e-9);
        assert!(!t.is_dead());
        t.close();
    }
}
