//! Transport abstraction for the tui_gateway JSON-RPC server.
//!
//! 1:1 port of `tui_gateway/transport.py` (219 lines).
//!
//! Historically the gateway wrote every JSON frame directly to real stdout.
//! This module decouples the I/O sink from the handler logic so the same
//! dispatcher can be driven over stdio (`tui_gateway.entry`) or WebSocket
//! (`tui_gateway.ws`) without duplicating code.
//!
//! A [`Transport`] is anything that can accept a JSON-serialisable dict and
//! forward it to its peer. The active transport for the current request is
//! tracked in a thread-local (mirrors `contextvars.ContextVar`) so handlers
//! — including those dispatched onto the worker pool — route their writes to
//! the right peer.
//!
//! Backward compatibility: `tui_gateway.server.write_json` still works without
//! any transport bound. When nothing is on the contextvar and no session-level
//! transport is found, it falls back to the module-level [`StdioTransport`],
//! which wraps the original `_real_stdout` + `_stdout_lock` pair. Tests that
//! monkey-patch `server._real_stdout` continue to work because the stdio
//! transport resolves the stream lazily through a callback.
//!
//! ```python
//! # Python — tui_gateway/transport.py
//! _PEER_GONE_ERRNOS = frozenset({errno.EPIPE, errno.ECONNRESET, errno.EBADF,
//!                                errno.ESHUTDOWN, WSAECONNRESET, WSAESHUTDOWN} - {-1})
//! _DISABLE_FLUSH = (os.environ.get("HERMES_TUI_GATEWAY_NO_FLUSH","") or "").strip().lower() in {"1","true","yes","on"}
//! @runtime_checkable
//! class Transport(Protocol):
//!     def write(self, obj: dict) -> bool: ...
//!     def close(self) -> None: ...
//! _current_transport: ContextVar[Optional[Transport]] = ContextVar("hermes_gateway_transport", default=None)
//! def current_transport() -> Optional[Transport]: ...
//! def bind_transport(transport: Optional[Transport]): ...
//! def reset_transport(token) -> None: ...
//! class StdioTransport:
//!     __slots__ = ("_stream_getter", "_lock")
//!     def __init__(self, stream_getter, lock): ...
//!     def write(self, obj: dict) -> bool: ...  # serialization outside lock, peer-gone vs re-raise
//!     def close(self) -> None: ...
//! class TeeTransport:
//!     __slots__ = ("_primary", "_secondaries")
//!     def __init__(self, primary, *secondaries): ...
//!     def write(self, obj: dict) -> bool: ...
//!     def close(self) -> None: ...
//! ```
//!
//! # Rust mapping
//!
//! * `_PEER_GONE_ERRNOS` → [`PEER_GONE_ERRNOS`] / [`PEER_GONE_ERRNOS_ALL`] slice +
//!   [`is_peer_gone_errno`] / [`is_peer_gone_error`] helpers. Errno values are
//!   the POSIX `EPIPE`/`ECONNRESET`/`EBADF`/`ESHUTDOWN` (Linux 32/104/9/108 and
//!   macOS 32/54/9/58) plus Windows `WSAECONNRESET`/`WSAESHUTDOWN` (10054/10058);
//!   the `{-1}` filter for missing `WSA*` on POSIX is typed away (all constants
//!   are known at compile time).
//! * `_DISABLE_FLUSH` → [`is_disable_flush`] (reads `HERMES_TUI_GATEWAY_NO_FLUSH`
//!   and checks `{"1","true","yes","on"}` case-insensitively, trimmed, like
//!   Python's `strip().lower() in {...}`) plus pure [`is_disable_flush_with`]
//!   for tests. Python evaluates once at import; Rust evaluates on each
//!   [`StdioTransport::write`] so tests can flip the env without reload.
//! * `Transport(Protocol)` (`write`/`close`) → [`Transport`] trait
//!   (`Send + Sync + 'static`, `write(&self, obj_json: &str) -> bool` +
//!   `close(&self)`). Python takes `dict` and calls `json.dumps(obj,
//!   ensure_ascii=False)` inside `write`; Rust is `std`-only so the caller
//!   serializes to a JSON `&str` first (`serde_json::to_string(&obj)` with
//!   `ensure_ascii=False` is the direct equivalent — Rust's default is UTF-8
//!   verbatim). `write` returns `false` ONLY when the peer is gone; other
//!   errors re-raise (in Rust: `panic!`, mirroring Python `raise` so the
//!   crash log captures it). [`StdioTransport::try_write`] exposes the
//!   `Result<bool, io::Error>` version for callers/tests that want to
//!   distinguish the re-raise without catching a panic.
//! * `ContextVar[Optional[Transport]]` → `thread_local! { RefCell<Option<Arc<dyn Transport>>> }`.
//!   Python's `ContextVar` is per-task/per-thread; Rust's `thread_local!` is
//!   per-thread (the gateway's worker pool is thread-based, so this is the
//!   correct analogue). [`current_transport`]/[`bind_transport`]/[`reset_transport`]
//!   preserve the `get`/`set`/`reset(token)` shape; `token` is the previous
//!   `Option<Arc<dyn Transport>>` (Python's `Token` holds the previous value).
//! * `StdioTransport` (`stream_getter: Callable[[], Any]`, `lock: threading.Lock`)
//!   → [`StdioTransport`] (`stream_getter: Arc<dyn Fn() -> Arc<Mutex<Box<dyn Write+Send>>> + Send+Sync>`,
//!   `lock: Arc<Mutex<()>>`). The callable indirection is preserved so runtime
//!   monkey-patches of the underlying stream continue to work (tests that swap
//!   `server._real_stdout`). Serialization is outside the lock (large payloads
//!   don't block other threads), then `with self._lock:` + `stream.write(line)`
//!   + `stream.flush()` (unless disabled) is mirrored by holding `self.lock`
//!   across `write_all` + optional `flush`. Error classification mirrors Python
//!   exactly (see [`is_peer_gone_io_error`]):
//!   `BrokenPipeError` → `ErrorKind::BrokenPipe` → `false`; `ValueError("I/O operation on closed file")`
//!   → `err.to_string().contains("closed file")` (and NOT `UnicodeEncodeError`,
//!   which is a `ValueError` subclass for misconfigured locales — detected via
//!   `"encode"` in the message and re-raised); `OSError` with `errno in _PEER_GONE_ERRNOS`
//!   → `raw_os_error()` in [`PEER_GONE_ERRNOS`] → `false` + `log::debug!` when
//!   the `log` feature is enabled; other `OSError` errnos / `UnicodeEncodeError` →
//!   re-raise (`panic!` in Rust, `raise` in Python).
//! * `TeeTransport` (`primary`, `*secondaries`, primary-first, secondaries best-effort)
//!   → [`TeeTransport`] (`primary: Arc<dyn Transport>`, `secondaries: Vec<Arc<dyn Transport>>`).
//!   `write` calls `primary.write` first (slow sidecar never delays Ink/stdio) and
//!   swallows secondary exceptions via `catch_unwind`; `close` closes primary
//!   then each secondary, swallowing exceptions (`try { primary.close() } finally { for sec... }`).

use std::cell::RefCell;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Constants — mirrors transport.py:36-63
// ---------------------------------------------------------------------------

/// Env var for the flush-disable knob. Mirrors `HERMES_TUI_GATEWAY_NO_FLUSH`.
pub const ENV_NO_FLUSH: &str = "HERMES_TUI_GATEWAY_NO_FLUSH";

/// POSIX `EPIPE` — write to closed pipe.
pub const EPIPE: i32 = 32;
/// POSIX `EBADF` — fd closed under us.
pub const EBADF: i32 = 9;
/// Linux `ECONNRESET` — peer reset.
pub const ECONNRESET_LINUX: i32 = 104;
/// Linux `ESHUTDOWN` — transport endpoint shut down.
pub const ESHUTDOWN_LINUX: i32 = 108;
/// macOS `ECONNRESET` — peer reset (macOS value differs from Linux).
pub const ECONNRESET_MACOS: i32 = 54;
/// macOS `ESHUTDOWN` — transport endpoint shut down (macOS value).
pub const ESHUTDOWN_MACOS: i32 = 58;
/// Windows `WSAECONNRESET` — win32 mapping (no-op on POSIX).
pub const WSAECONNRESET: i32 = 10054;
/// Windows `WSAESHUTDOWN` — win32 mapping (no-op on POSIX).
pub const WSAESHUTDOWN: i32 = 10058;

/// Peer-gone errnos. Mirrors `_PEER_GONE_ERRNOS = frozenset({...} - {-1})`.
///
/// Contains Linux, macOS, and Windows values so the same slice works on every
/// host (the Python `getattr(errno, "WSA*", -1) - {-1}` dance is typed away).
pub const PEER_GONE_ERRNOS: &[i32] = &[
    EPIPE,
    EBADF,
    ECONNRESET_LINUX,
    ESHUTDOWN_LINUX,
    ECONNRESET_MACOS,
    ESHUTDOWN_MACOS,
    WSAECONNRESET,
    WSAESHUTDOWN,
];

/// Alias for completeness (mirrors the filtered frozenset name).
pub const PEER_GONE_ERRNOS_ALL: &[i32] = PEER_GONE_ERRNOS;

/// Whether `errno` means "the peer is gone".
///
/// Mirrors `e.errno in _PEER_GONE_ERRNOS`.
pub fn is_peer_gone_errno(errno: i32) -> bool {
    PEER_GONE_ERRNOS.contains(&errno)
}

/// Whether `err`'s raw OS errno is peer-gone.
///
/// Mirrors `except OSError as e: if e.errno not in _PEER_GONE_ERRNOS: raise`.
pub fn is_peer_gone_error(err: &io::Error) -> bool {
    match err.raw_os_error() {
        Some(n) => is_peer_gone_errno(n),
        None => false,
    }
}

/// Whether `err` is a peer-gone I/O error (BrokenPipe + closed-file + OSError errno).
///
/// Mirrors the full `StdioTransport.write` peer-gone branch:
///
/// * `BrokenPipeError` → `ErrorKind::BrokenPipe` → peer gone
/// * `ValueError("I/O operation on closed file")` → `to_string().contains("closed file")`
///   (but `UnicodeEncodeError` — a `ValueError` subclass for misconfigured locales —
///   is NOT peer gone; detected via `"encode"` in the message and treated as real bug)
/// * `OSError` with `errno in _PEER_GONE_ERRNOS` → peer gone
pub fn is_peer_gone_io_error(err: &io::Error) -> bool {
    if err.kind() == io::ErrorKind::BrokenPipe {
        return true;
    }
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    // UnicodeEncodeError is a ValueError subclass — re-raise (not peer gone)
    if lower.contains("encode") {
        return false;
    }
    if msg.contains("closed file") {
        return true;
    }
    is_peer_gone_error(err)
}

// ---------------------------------------------------------------------------
// _DISABLE_FLUSH — mirrors transport.py:58-63 (+ doc 48-57)
// ---------------------------------------------------------------------------

/// Pure helper: is `raw` a truthy flush-disable value?
///
/// Mirrors `(os.environ.get("HERMES_TUI_GATEWAY_NO_FLUSH","") or "").strip().lower() in {"1","true","yes","on"}`.
///
/// `None` / empty / whitespace-only → `false`.
///
/// ```python
/// _DISABLE_FLUSH = (os.environ.get("HERMES_TUI_GATEWAY_NO_FLUSH", "") or "").strip().lower() in {"1","true","yes","on"}
/// ```
pub fn is_disable_flush_with(raw: Option<&str>) -> bool {
    match raw {
        None => false,
        Some(s) => {
            let t = s.trim().to_ascii_lowercase();
            matches!(t.as_str(), "1" | "true" | "yes" | "on")
        }
    }
}

/// Read `HERMES_TUI_GATEWAY_NO_FLUSH` from the process env.
///
/// Python evaluates `_DISABLE_FLUSH` once at import; Rust evaluates on each
/// call so tests can flip the env without reloading the module. The default
/// is `false` (flush-after-write stays on) — see the `PYTHONUNBUFFERED` note
/// in the module docstring: disabling flush only makes sense with `-u` or
/// `PYTHONUNBUFFERED=1`, otherwise JSON frames buffer and the TUI hangs.
pub fn is_disable_flush() -> bool {
    let raw = std::env::var(ENV_NO_FLUSH).ok();
    is_disable_flush_with(raw.as_deref())
}

// ---------------------------------------------------------------------------
// Transport trait — mirrors transport.py:66-75
// ---------------------------------------------------------------------------

/// Minimal interface every transport implements.
///
/// Mirrors `tui_gateway/transport.py::Transport`:
///
/// ```python
/// @runtime_checkable
/// class Transport(Protocol):
///     def write(self, obj: dict) -> bool: ...
///     def close(self) -> None: ...
/// ```
///
/// `write` emits one JSON frame. Return `false` when the peer is gone
/// (the dispatcher's "broken stdout pipe" signal — `entry.py` calls
/// `sys.exit(0)` when `write_json` reports `false`). Real bugs
/// (non-JSON-safe payloads, encoding misconfig, `ENOSPC`, etc.) MUST NOT
/// return `false`; they re-raise (`panic!` in Rust) so the crash log
/// records the traceback.
///
/// `close` releases any resources owned by this transport.
pub trait Transport: Send + Sync + 'static {
    /// Emit one JSON frame.
    ///
    /// `obj_json` is the already-serialized JSON string (mirrors
    /// `json.dumps(obj, ensure_ascii=False)` in `StdioTransport.write`).
    /// Implementations append `"\n"` and forward it to their peer.
    /// Return `false` ONLY when the peer is gone; real I/O problems
    /// must `panic!` (mirrors Python `raise`).
    fn write(&self, obj_json: &str) -> bool;

    /// Release any resources owned by this transport.
    fn close(&self);
}

// ---------------------------------------------------------------------------
// ContextVar — mirrors transport.py:77-98
// ---------------------------------------------------------------------------

thread_local! {
    static CURRENT_TRANSPORT: RefCell<Option<Arc<dyn Transport>>> = const { RefCell::new(None) };
}

/// Return the transport bound for the current request, if any.
///
/// Mirrors `tui_gateway/transport.py::current_transport`:
///
/// ```python
/// def current_transport() -> Optional[Transport]:
///     return _current_transport.get()
/// ```
pub fn current_transport() -> Option<Arc<dyn Transport>> {
    CURRENT_TRANSPORT.with(|c| c.borrow().clone())
}

/// Bind `transport` for the current context. Returns a token for [`reset_transport`].
///
/// Mirrors `tui_gateway/transport.py::bind_transport`:
///
/// ```python
/// def bind_transport(transport: Optional[Transport]):
///     return _current_transport.set(transport)
/// ```
///
/// The token is the previous value (Python returns a `contextvars.Token`
/// holding the old value; the observable payload is the same).
pub fn bind_transport(transport: Option<Arc<dyn Transport>>) -> Option<Arc<dyn Transport>> {
    CURRENT_TRANSPORT.with(|c| {
        let prev = c.borrow().clone();
        *c.borrow_mut() = transport;
        prev
    })
}

/// Restore the transport binding captured by [`bind_transport`].
///
/// Mirrors `tui_gateway/transport.py::reset_transport`:
///
/// ```python
/// def reset_transport(token) -> None:
///     _current_transport.reset(token)
/// ```
pub fn reset_transport(token: Option<Arc<dyn Transport>>) {
    CURRENT_TRANSPORT.with(|c| {
        *c.borrow_mut() = token;
    })
}

/// Convenience: bind a concrete `Arc<dyn Transport>` and return the token.
pub fn bind_transport_arc(transport: Arc<dyn Transport>) -> Option<Arc<dyn Transport>> {
    bind_transport(Some(transport))
}

/// Clear the current transport (bind `None`). Returns the previous value.
pub fn clear_transport() -> Option<Arc<dyn Transport>> {
    bind_transport(None)
}

// ---------------------------------------------------------------------------
// StdioTransport — mirrors transport.py:100-183
// ---------------------------------------------------------------------------

/// Writes JSON frames to a stream (usually `sys.stdout`).
///
/// The stream is resolved via a callable so runtime monkey-patches of the
/// underlying stream continue to work — this preserves the behaviour the
/// existing test suite relies on (`monkeypatch.setattr(server, "_real_stdout", ...)`).
///
/// Mirrors `tui_gateway/transport.py::StdioTransport`:
///
/// ```python
/// class StdioTransport:
///     __slots__ = ("_stream_getter", "_lock")
///     def __init__(self, stream_getter: Callable[[], Any], lock: threading.Lock) -> None:
///         self._stream_getter = stream_getter
///         self._lock = lock
/// ```
pub struct StdioTransport {
    /// Mirrors `self._stream_getter`.
    stream_getter: Arc<dyn Fn() -> Arc<Mutex<Box<dyn Write + Send>>> + Send + Sync>,
    /// Mirrors `self._lock`.
    lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for StdioTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StdioTransport")
            .field("has_stream_getter", &true)
            .field("has_lock", &true)
            .finish()
    }
}

impl StdioTransport {
    /// Create a transport from `stream_getter` and `lock`.
    ///
    /// Mirrors `StdioTransport.__init__(self, stream_getter, lock)`:
    ///
    /// ```python
    /// def __init__(self, stream_getter: Callable[[], Any], lock: threading.Lock) -> None:
    ///     self._stream_getter = stream_getter
    ///     self._lock = lock
    /// ```
    pub fn new<F>(stream_getter: F, lock: Arc<Mutex<()>>) -> Self
    where
        F: Fn() -> Arc<Mutex<Box<dyn Write + Send>>> + Send + Sync + 'static,
    {
        Self {
            stream_getter: Arc::new(stream_getter),
            lock,
        }
    }

    /// Create a transport with a fresh lock (`Arc::new(Mutex::new(()))`).
    ///
    /// Convenience when the caller doesn't share a lock with other transports.
    pub fn with_getter<F>(stream_getter: F) -> Self
    where
        F: Fn() -> Arc<Mutex<Box<dyn Write + Send>>> + Send + Sync + 'static,
    {
        Self::new(stream_getter, Arc::new(Mutex::new(())))
    }

    /// Create a transport that always returns `shared` from its getter.
    ///
    /// Test helper — mirrors a `stream_getter` that always returns the same
    /// `Arc<Mutex<Box<dyn Write>>>`.
    pub fn with_shared(shared: Arc<Mutex<Box<dyn Write + Send>>>) -> Self {
        Self::with_shared_and_lock(shared, Arc::new(Mutex::new(())))
    }

    /// Create a transport with explicit `shared` writer and `lock`.
    pub fn with_shared_and_lock(
        shared: Arc<Mutex<Box<dyn Write + Send>>>,
        lock: Arc<Mutex<()>>,
    ) -> Self {
        let getter = move || Arc::clone(&shared);
        Self::new(getter, lock)
    }

    /// Try to write `obj_json` as a JSON line, returning `Ok(true)` on success,
    /// `Ok(false)` ONLY when the peer is gone, and `Err(e)` for real bugs.
    ///
    /// This is the `Result`-bearing core of [`Transport::write`] — the trait
    /// method wraps it and `panic!` on `Err` to mirror Python's `raise` for
    /// non-peer-gone errors. Tests should call `try_write` directly to assert
    /// the `Err` path without catching a panic.
    ///
    /// Mirrors `StdioTransport.write`:
    ///
    /// ```python
    /// def write(self, obj: dict) -> bool:
    ///     line = json.dumps(obj, ensure_ascii=False) + "\n"
    ///     with self._lock:
    ///         stream = self._stream_getter()
    ///         try: stream.write(line)
    ///         except BrokenPipeError: return False
    ///         except ValueError as e:
    ///             if isinstance(e, UnicodeEncodeError) or "closed file" not in str(e): raise
    ///             return False
    ///         except OSError as e:
    ///             if e.errno not in _PEER_GONE_ERRNOS: raise
    ///             logger.debug("StdioTransport write peer gone: %s", e)
    ///             return False
    ///         if not _DISABLE_FLUSH:
    ///             try: stream.flush()
    ///             except BrokenPipeError: return False
    ///             except ValueError as e:
    ///                 if isinstance(e, UnicodeEncodeError) or "closed file" not in str(e): raise
    ///                 return False
    ///             except OSError as e:
    ///                 if e.errno not in _PEER_GONE_ERRNOS: raise
    ///                 logger.debug("StdioTransport flush peer gone: %s", e)
    ///                 return False
    ///     return True
    /// ```
    ///
    /// Serialization is OUTSIDE the lock so a large payload can't block other
    /// threads emitting their own frames (mirrors the comment in Python). The
    /// caller passes `obj_json` already-serialized; this method appends `"\n"`
    /// (matching `json.dumps(...) + "\n"`).
    pub fn try_write(&self, obj_json: &str) -> io::Result<bool> {
        // Serialization outside the lock — `obj_json` is already serialized by the caller.
        // Mirror `line = json.dumps(obj, ensure_ascii=False) + "\n"` — ensure trailing newline.
        let line = if obj_json.ends_with('\n') {
            obj_json.to_string()
        } else {
            format!("{}\n", obj_json)
        };
        let line_bytes = line.as_bytes();

        // Mirrors `with self._lock:`
        let _guard = self.lock.lock().unwrap();
        let stream_arc = (self.stream_getter)();
        let mut stream = stream_arc.lock().unwrap();

        // --- write phase ---
        if let Err(e) = stream.write_all(line_bytes) {
            if is_peer_gone_io_error(&e) {
                #[cfg(feature = "log")]
                log::debug!("StdioTransport write peer gone: {}", e);
                return Ok(false);
            } else {
                // Programming error / host I/O bug — re-raise (Err) so crash log captures it.
                return Err(e);
            }
        }

        // --- flush phase (unless disabled) ---
        // Mirrors the `_DISABLE_FLUSH` knob: when true, skip `stream.flush()` entirely.
        // Python's note: text stdout is fully buffered on a pipe, so disabling flush
        // only makes sense with `-u` / `PYTHONUNBUFFERED=1`; otherwise frames buffer
        // and the TUI hangs.
        if is_disable_flush() {
            return Ok(true);
        }
        if let Err(e) = stream.flush() {
            if is_peer_gone_io_error(&e) {
                #[cfg(feature = "log")]
                log::debug!("StdioTransport flush peer gone: {}", e);
                return Ok(false);
            } else {
                return Err(e);
            }
        }

        Ok(true)
    }

    /// Like [`Self::try_write`] but with an injected flush-disable flag.
    ///
    /// Test seam for the `_DISABLE_FLUSH` env knob without touching `std::env`.
    pub fn try_write_with_flush(&self, obj_json: &str, disable_flush: bool) -> io::Result<bool> {
        let line = if obj_json.ends_with('\n') {
            obj_json.to_string()
        } else {
            format!("{}\n", obj_json)
        };
        let line_bytes = line.as_bytes();
        let _guard = self.lock.lock().unwrap();
        let stream_arc = (self.stream_getter)();
        let mut stream = stream_arc.lock().unwrap();
        if let Err(e) = stream.write_all(line_bytes) {
            if is_peer_gone_io_error(&e) {
                #[cfg(feature = "log")]
                log::debug!("StdioTransport write peer gone: {}", e);
                return Ok(false);
            } else {
                return Err(e);
            }
        }
        if disable_flush {
            return Ok(true);
        }
        if let Err(e) = stream.flush() {
            if is_peer_gone_io_error(&e) {
                #[cfg(feature = "log")]
                log::debug!("StdioTransport flush peer gone: {}", e);
                return Ok(false);
            } else {
                return Err(e);
            }
        }
        Ok(true)
    }

    /// Access the lock (test helper).
    pub fn lock(&self) -> &Arc<Mutex<()>> {
        &self.lock
    }
}

impl Transport for StdioTransport {
    fn write(&self, obj_json: &str) -> bool {
        match self.try_write(obj_json) {
            Ok(v) => v,
            Err(e) => panic!("StdioTransport write re-raised (not peer gone): {}", e),
        }
    }

    fn close(&self) {
        // Mirrors `def close(self) -> None: return None` — no resources to release.
    }
}

// ---------------------------------------------------------------------------
// TeeTransport — mirrors transport.py:186-219
// ---------------------------------------------------------------------------

/// Mirrors writes to one primary plus N best-effort secondaries.
///
/// The primary's return value (and exceptions) determine the result —
/// secondaries swallow failures so a wedged sidecar never stalls the
/// main IO path. Used by the PTY child so every dispatcher emit lands
/// on stdio (Ink) AND on a back-WS feeding the dashboard sidebar.
///
/// Mirrors `tui_gateway/transport.py::TeeTransport`:
///
/// ```python
/// class TeeTransport:
///     __slots__ = ("_primary", "_secondaries")
///     def __init__(self, primary: "Transport", *secondaries: "Transport") -> None:
///         self._primary = primary
///         self._secondaries = secondaries
///     def write(self, obj: dict) -> bool:
///         ok = self._primary.write(obj)
///         for sec in self._secondaries:
///             try: sec.write(obj)
///             except Exception: pass
///         return ok
///     def close(self) -> None:
///         try: self._primary.close()
///         finally:
///             for sec in self._secondaries:
///                 try: sec.close()
///                 except Exception: pass
/// ```
pub struct TeeTransport {
    /// Mirrors `self._primary`.
    primary: Arc<dyn Transport>,
    /// Mirrors `self._secondaries`.
    secondaries: Vec<Arc<dyn Transport>>,
}

impl std::fmt::Debug for TeeTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeeTransport")
            .field("primary", &format_args!("Transport"))
            .field("secondaries_len", &self.secondaries.len())
            .finish()
    }
}

impl TeeTransport {
    /// Create a tee with `primary` and `secondaries`.
    ///
    /// Mirrors `TeeTransport.__init__(self, primary, *secondaries)`.
    pub fn new(primary: Arc<dyn Transport>, secondaries: Vec<Arc<dyn Transport>>) -> Self {
        Self { primary, secondaries }
    }

    /// Create a tee with one primary and one secondary.
    pub fn with_one_secondary(primary: Arc<dyn Transport>, secondary: Arc<dyn Transport>) -> Self {
        Self {
            primary,
            secondaries: vec![secondary],
        }
    }

    /// Create a tee with no secondaries (degenerate — just forwards to primary).
    pub fn with_primary_only(primary: Arc<dyn Transport>) -> Self {
        Self {
            primary,
            secondaries: Vec::new(),
        }
    }

    /// The primary transport.
    pub fn primary(&self) -> &Arc<dyn Transport> {
        &self.primary
    }

    /// The secondary transports.
    pub fn secondaries(&self) -> &[Arc<dyn Transport>] {
        &self.secondaries
    }

    /// Number of secondaries.
    pub fn secondaries_len(&self) -> usize {
        self.secondaries.len()
    }
}

impl Transport for TeeTransport {
    fn write(&self, obj_json: &str) -> bool {
        // Primary first so a slow sidecar (WS publisher) never delays Ink/stdio.
        let ok = self.primary.write(obj_json);
        for sec in &self.secondaries {
            // Mirrors `try: sec.write(obj) except Exception: pass` — swallow panics too.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sec.write(obj_json);
            }));
        }
        ok
    }

    fn close(&self) {
        // Mirrors `try: self._primary.close() finally: for sec in self._secondaries: try: sec.close() except: pass`
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.primary.close();
        }));
        for sec in &self.secondaries {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                sec.close();
            }));
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — mirror Python invariants
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{self, Write};
    use std::sync::{Arc, Mutex};

    // -- helpers -------------------------------------------------------------

    fn shared_buf() -> (Arc<Mutex<Vec<u8>>>, Arc<Mutex<Box<dyn Write + Send>>>) {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = Arc::clone(&buf);
        struct BufWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for BufWriter {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let writer: Box<dyn Write + Send> = Box::new(BufWriter(buf_clone));
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        (buf, shared)
    }

    fn tracking_writer(
        buf: Arc<Mutex<Vec<u8>>>,
        flush_called: Arc<Mutex<bool>>,
        write_fail: Option<FailMode>,
        flush_fail: Option<FailMode>,
        write_fail2: Option<FailMode>,
    ) -> Box<dyn Write + Send> {
        struct Tracking {
            buf: Arc<Mutex<Vec<u8>>>,
            flush_called: Arc<Mutex<bool>>,
            write_fail: Option<FailMode>,
            flush_fail: Option<FailMode>,
        }
        impl Write for Tracking {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                if let Some(fail) = self.write_fail.take() {
                    return Err(fail.to_err());
                }
                self.buf.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                *self.flush_called.lock().unwrap() = true;
                if let Some(fail) = self.flush_fail.take() {
                    return Err(fail.to_err());
                }
                Ok(())
            }
        }
        let _ = write_fail2;
        Box::new(Tracking {
            buf,
            flush_called,
            write_fail,
            flush_fail,
        })
    }

    #[derive(Debug, Clone)]
    enum FailMode {
        BrokenPipe,
        ClosedFile,
        EncodeError,
        PeerGoneErrno(i32),
        NonPeerGoneErrno(i32),
        Other(String),
    }

    impl FailMode {
        fn to_err(&self) -> io::Error {
            match self {
                FailMode::BrokenPipe => io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe"),
                FailMode::ClosedFile => {
                    io::Error::new(io::ErrorKind::Other, "I/O operation on closed file")
                }
                FailMode::EncodeError => {
                    io::Error::new(io::ErrorKind::Other, "encode error: 'utf-8' codec can't encode")
                }
                FailMode::PeerGoneErrno(n) => io::Error::from_raw_os_error(*n),
                FailMode::NonPeerGoneErrno(n) => io::Error::from_raw_os_error(*n),
                FailMode::Other(s) => io::Error::new(io::ErrorKind::Other, s.clone()),
            }
        }
    }

    // -- PEER_GONE_ERRNOS ----------------------------------------------------

    #[test]
    fn peer_gone_errnos_contains_expected() {
        // Mirrors Python: {EPIPE, ECONNRESET, EBADF, ESHUTDOWN, WSAECONNRESET, WSAESHUTDOWN}
        assert!(is_peer_gone_errno(EPIPE));
        assert!(is_peer_gone_errno(EBADF));
        assert!(is_peer_gone_errno(ECONNRESET_LINUX));
        assert!(is_peer_gone_errno(ESHUTDOWN_LINUX));
        assert!(is_peer_gone_errno(ECONNRESET_MACOS));
        assert!(is_peer_gone_errno(ESHUTDOWN_MACOS));
        assert!(is_peer_gone_errno(WSAECONNRESET));
        assert!(is_peer_gone_errno(WSAESHUTDOWN));
        // Non-peer-gone examples
        assert!(!is_peer_gone_errno(28)); // ENOSPC
        assert!(!is_peer_gone_errno(13)); // EACCES
        assert!(!is_peer_gone_errno(-1)); // filtered sentinel
    }

    #[test]
    fn is_peer_gone_error_uses_raw_os_error() {
        let e = io::Error::from_raw_os_error(EPIPE);
        assert!(is_peer_gone_error(&e));
        let e2 = io::Error::from_raw_os_error(28);
        assert!(!is_peer_gone_error(&e2));
        let e3 = io::Error::new(io::ErrorKind::Other, "no errno");
        assert!(!is_peer_gone_error(&e3));
    }

    #[test]
    fn is_peer_gone_io_error_classification() {
        // BrokenPipe => peer gone
        let e = io::Error::new(io::ErrorKind::BrokenPipe, "broken pipe");
        assert!(is_peer_gone_io_error(&e));
        // closed file => peer gone
        let e2 = io::Error::new(io::ErrorKind::Other, "I/O operation on closed file");
        assert!(is_peer_gone_io_error(&e2));
        // encode error (even with closed file substring) => NOT peer gone (re-raise)
        let e3 = io::Error::new(io::ErrorKind::Other, "closed file but encode error");
        // contains "encode" => false
        assert!(!is_peer_gone_io_error(&e3));
        let e4 = io::Error::new(io::ErrorKind::Other, "encode boom");
        assert!(!is_peer_gone_io_error(&e4));
        // peer gone errno
        let e5 = io::Error::from_raw_os_error(EPIPE);
        assert!(is_peer_gone_io_error(&e5));
        // non-peer gone errno
        let e6 = io::Error::from_raw_os_error(28);
        assert!(!is_peer_gone_io_error(&e6));
        // plain other without closed file
        let e7 = io::Error::new(io::ErrorKind::Other, "something else");
        assert!(!is_peer_gone_io_error(&e7));
    }

    // -- _DISABLE_FLUSH ------------------------------------------------------

    #[test]
    fn disable_flush_parsing() {
        assert!(!is_disable_flush_with(None));
        assert!(!is_disable_flush_with(Some("")));
        assert!(!is_disable_flush_with(Some("   ")));
        assert!(is_disable_flush_with(Some("1")));
        assert!(is_disable_flush_with(Some("true")));
        assert!(is_disable_flush_with(Some("True")));
        assert!(is_disable_flush_with(Some("TRUE")));
        assert!(is_disable_flush_with(Some("yes")));
        assert!(is_disable_flush_with(Some("YES")));
        assert!(is_disable_flush_with(Some("on")));
        assert!(is_disable_flush_with(Some("ON")));
        assert!(is_disable_flush_with(Some("  yes  ")));
        assert!(!is_disable_flush_with(Some("0")));
        assert!(!is_disable_flush_with(Some("false")));
        assert!(!is_disable_flush_with(Some("no")));
        assert!(!is_disable_flush_with(Some("off")));
        assert!(!is_disable_flush_with(Some("2")));
    }

    // -- ContextVar ----------------------------------------------------------

    struct MockTransport {
        name: String,
        writes: Arc<Mutex<Vec<String>>>,
        fail: bool,
    }

    impl MockTransport {
        fn new(name: &str) -> (Arc<Self>, Arc<Mutex<Vec<String>>>) {
            let writes = Arc::new(Mutex::new(Vec::new()));
            let t = Arc::new(Self {
                name: name.to_string(),
                writes: Arc::clone(&writes),
                fail: false,
            });
            (t, writes)
        }
        fn failing(name: &str) -> Arc<Self> {
            Arc::new(Self {
                name: name.to_string(),
                writes: Arc::new(Mutex::new(Vec::new())),
                fail: true,
            })
        }
    }

    impl Transport for MockTransport {
        fn write(&self, obj_json: &str) -> bool {
            if self.fail {
                panic!("mock fail {}", self.name);
            }
            self.writes.lock().unwrap().push(obj_json.to_string());
            true
        }
        fn close(&self) {}
    }

    struct PanicOnWrite;
    impl Transport for PanicOnWrite {
        fn write(&self, _obj_json: &str) -> bool {
            panic!("secondary boom");
        }
        fn close(&self) {
            panic!("close boom");
        }
    }

    struct PeerGoneTransport;
    impl Transport for PeerGoneTransport {
        fn write(&self, _obj_json: &str) -> bool {
            false
        }
        fn close(&self) {}
    }

    #[test]
    fn context_var_bind_reset() {
        // Ensure clean start
        reset_transport(None);
        assert!(current_transport().is_none());

        let (t1, _w1) = MockTransport::new("t1");
        let t1_dyn: Arc<dyn Transport> = t1.clone();
        let tok = bind_transport(Some(Arc::clone(&t1_dyn)));
        assert!(tok.is_none());
        assert!(current_transport().is_some());

        let (t2, _w2) = MockTransport::new("t2");
        let t2_dyn: Arc<dyn Transport> = t2.clone();
        let tok2 = bind_transport(Some(Arc::clone(&t2_dyn)));
        assert!(tok2.is_some()); // previous was t1
        // current is t2
        let cur = current_transport().unwrap();
        // Check by writing and seeing which transport's buffer gets the write
        cur.write(r#"{"x":1}"#);
        assert_eq!(_w2.lock().unwrap().len(), 1);
        assert_eq!(_w1.lock().unwrap().len(), 0);

        // reset to t1
        reset_transport(tok2);
        let cur2 = current_transport().unwrap();
        cur2.write(r#"{"y":2}"#);
        assert_eq!(_w1.lock().unwrap().len(), 1);
        assert_eq!(_w2.lock().unwrap().len(), 1); // still 1

        // clear
        reset_transport(tok);
        assert!(current_transport().is_none());
    }

    #[test]
    fn bind_none_clears() {
        reset_transport(None);
        let (t, _) = MockTransport::new("x");
        let td: Arc<dyn Transport> = t;
        bind_transport(Some(td));
        assert!(current_transport().is_some());
        let prev = clear_transport();
        assert!(prev.is_some());
        assert!(current_transport().is_none());
        reset_transport(None);
    }

    // -- StdioTransport ------------------------------------------------------

    #[test]
    fn stdio_write_success_appends_newline_and_flushes() {
        let (buf, shared) = shared_buf();
        let flush_called = Arc::new(Mutex::new(false));
        // Use tracking writer to assert flush is called
        let buf_clone = Arc::clone(&buf);
        let flush_clone = Arc::clone(&flush_called);
        struct FlushTrack(Arc<Mutex<Vec<u8>>>, Arc<Mutex<bool>>);
        impl Write for FlushTrack {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                *self.1.lock().unwrap() = true;
                Ok(())
            }
        }
        let writer: Box<dyn Write + Send> = Box::new(FlushTrack(buf_clone, flush_clone));
        let shared2: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(Arc::clone(&shared2));
        let ok = transport.try_write(r#"{"a":1}"#).unwrap();
        assert!(ok);
        // buf contains line + newline
        let data = buf.lock().unwrap().clone();
        assert_eq!(String::from_utf8(data).unwrap(), "{\"a\":1}\n");
        assert!(*flush_called.lock().unwrap());
        // already-newline input should not double
        let ok2 = transport.try_write("{\"b\":2}\n").unwrap();
        assert!(ok2);
        let data2 = buf.lock().unwrap().clone();
        assert!(String::from_utf8(data2).unwrap().ends_with("{\"b\":2}\n"));
    }

    #[test]
    fn stdio_write_peer_gone_broken_pipe() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let flush_called = Arc::new(Mutex::new(false));
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::clone(&flush_called),
            Some(FailMode::BrokenPipe),
            None,
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write(r#"{"x":1}"#).unwrap();
        assert!(!res, "BrokenPipe should return false");
        assert!(!*flush_called.lock().unwrap(), "flush should not be called after write failure");
    }

    #[test]
    fn stdio_write_peer_gone_closed_file() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let flush_called = Arc::new(Mutex::new(false));
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::clone(&flush_called),
            Some(FailMode::ClosedFile),
            None,
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write(r#"{"x":1}"#).unwrap();
        assert!(!res);
    }

    #[test]
    fn stdio_write_peer_gone_errno() {
        for errno in [EPIPE, EBADF, ECONNRESET_LINUX, ESHUTDOWN_LINUX, WSAECONNRESET] {
            let buf = Arc::new(Mutex::new(Vec::new()));
            let flush_called = Arc::new(Mutex::new(false));
            let writer = tracking_writer(
                Arc::clone(&buf),
                Arc::clone(&flush_called),
                Some(FailMode::PeerGoneErrno(errno)),
                None,
                None,
            );
            let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
            let transport = StdioTransport::with_shared(shared);
            let res = transport.try_write(r#"{"x":1}"#).unwrap();
            assert!(!res, "errno {} should be peer gone", errno);
        }
    }

    #[test]
    fn stdio_write_non_peer_gone_is_err() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let flush_called = Arc::new(Mutex::new(false));
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::clone(&flush_called),
            Some(FailMode::NonPeerGoneErrno(28)), // ENOSPC
            None,
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write(r#"{"x":1}"#);
        assert!(res.is_err(), "ENOSPC should be Err (re-raise)");
        // trait write should panic
        let buf2 = Arc::new(Mutex::new(Vec::new()));
        let writer2 = tracking_writer(
            Arc::clone(&buf2),
            Arc::new(Mutex::new(false)),
            Some(FailMode::NonPeerGoneErrno(28)),
            None,
            None,
        );
        let shared2: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer2));
        let transport2 = StdioTransport::with_shared(shared2);
        let panicked = std::panic::catch_unwind(|| transport2.write(r#"{"x":1}"#));
        assert!(panicked.is_err());
    }

    #[test]
    fn stdio_write_encode_error_is_err_not_peer_gone() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::new(Mutex::new(false)),
            Some(FailMode::EncodeError),
            None,
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write(r#"{"x":1}"#);
        assert!(res.is_err(), "encode error should re-raise");
    }

    #[test]
    fn stdio_flush_peer_gone_returns_false() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let flush_called = Arc::new(Mutex::new(false));
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::clone(&flush_called),
            None,
            Some(FailMode::BrokenPipe),
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write_with_flush(r#"{"x":1}"#, false).unwrap();
        assert!(!res);
        assert!(*flush_called.lock().unwrap());
    }

    #[test]
    fn stdio_flush_non_peer_gone_is_err() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::new(Mutex::new(false)),
            None,
            Some(FailMode::NonPeerGoneErrno(28)),
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write_with_flush(r#"{"x":1}"#, false);
        assert!(res.is_err());
    }

    #[test]
    fn stdio_flush_disabled_skips_flush() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let flush_called = Arc::new(Mutex::new(false));
        // flush would fail with BrokenPipe, but we disable flush so it should succeed
        let writer = tracking_writer(
            Arc::clone(&buf),
            Arc::clone(&flush_called),
            None,
            Some(FailMode::BrokenPipe),
            None,
        );
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let transport = StdioTransport::with_shared(shared);
        let res = transport.try_write_with_flush(r#"{"x":1}"#, true).unwrap();
        assert!(res, "disable_flush should skip flush failure");
        assert!(!*flush_called.lock().unwrap(), "flush should not be called when disabled");
        // also check that data was written
        assert_eq!(String::from_utf8(buf.lock().unwrap().clone()).unwrap(), "{\"x\":1}\n");
    }

    #[test]
    fn stdio_close_is_noop() {
        let (_buf, shared) = shared_buf();
        let transport = StdioTransport::with_shared(shared);
        transport.close();
        transport.close(); // idempotent
        // write still works after close (Python close is no-op)
        assert!(transport.try_write(r#"{"a":1}"#).unwrap());
    }

    #[test]
    fn stdio_getter_called_lazily_each_write() {
        let call_count = Arc::new(Mutex::new(0usize));
        let buf = Arc::new(Mutex::new(Vec::new()));
        let buf_clone = Arc::clone(&buf);
        let count_clone = Arc::clone(&call_count);
        struct Simple(Arc<Mutex<Vec<u8>>>);
        impl Write for Simple {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let writer: Box<dyn Write + Send> = Box::new(Simple(buf_clone));
        let shared: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(writer));
        let shared_clone = Arc::clone(&shared);
        let getter = move || {
            *count_clone.lock().unwrap() += 1;
            Arc::clone(&shared_clone)
        };
        let transport = StdioTransport::with_getter(getter);
        assert_eq!(*call_count.lock().unwrap(), 0);
        transport.try_write(r#"{"a":1}"#).unwrap();
        assert_eq!(*call_count.lock().unwrap(), 1);
        transport.try_write(r#"{"b":2}"#).unwrap();
        assert_eq!(*call_count.lock().unwrap(), 2);
    }

    // -- TeeTransport -------------------------------------------------------

    #[test]
    fn tee_primary_ok_secondaries_best_effort() {
        let (p, pw) = MockTransport::new("primary");
        let (s1, sw1) = MockTransport::new("sec1");
        let s2: Arc<dyn Transport> = Arc::new(PanicOnWrite);
        let tee = TeeTransport::new(p.clone() as Arc<dyn Transport>, vec![s1.clone() as Arc<dyn Transport>, s2]);
        let ok = tee.write(r#"{"a":1}"#);
        assert!(ok);
        assert_eq!(pw.lock().unwrap().len(), 1);
        assert_eq!(sw1.lock().unwrap().len(), 1);
        // s2 panicked but was swallowed, primary ok still true
    }

    #[test]
    fn tee_primary_peer_gone_returns_false_but_secondaries_still_called() {
        let p: Arc<dyn Transport> = Arc::new(PeerGoneTransport);
        let (s1, sw1) = MockTransport::new("sec1");
        let tee = TeeTransport::new(p, vec![s1.clone() as Arc<dyn Transport>]);
        let ok = tee.write(r#"{"a":1}"#);
        assert!(!ok);
        assert_eq!(sw1.lock().unwrap().len(), 1);
    }

    #[test]
    fn tee_secondary_panic_swallowed_primary_true() {
        let (p, pw) = MockTransport::new("primary");
        let s: Arc<dyn Transport> = Arc::new(PanicOnWrite);
        let tee = TeeTransport::with_one_secondary(p.clone() as Arc<dyn Transport>, s);
        let ok = tee.write(r#"{"a":1}"#);
        assert!(ok);
        assert_eq!(pw.lock().unwrap().len(), 1);
    }

    #[test]
    fn tee_close_all_even_if_primary_panics() {
        struct ClosePanic;
        impl Transport for ClosePanic {
            fn write(&self, _s: &str) -> bool {
                true
            }
            fn close(&self) {
                panic!("close panic");
            }
        }
        struct CloseTrack {
            closed: Arc<Mutex<bool>>,
        }
        impl Transport for CloseTrack {
            fn write(&self, _s: &str) -> bool {
                true
            }
            fn close(&self) {
                *self.closed.lock().unwrap() = true;
            }
        }
        let primary: Arc<dyn Transport> = Arc::new(ClosePanic);
        let closed1 = Arc::new(Mutex::new(false));
        let closed2 = Arc::new(Mutex::new(false));
        let s1: Arc<dyn Transport> = Arc::new(CloseTrack { closed: Arc::clone(&closed1) });
        let s2: Arc<dyn Transport> = Arc::new(CloseTrack { closed: Arc::clone(&closed2) });
        let tee = TeeTransport::new(primary, vec![s1, s2]);
        tee.close(); // should not panic, should close both secondaries even though primary panicked
        assert!(*closed1.lock().unwrap());
        assert!(*closed2.lock().unwrap());
    }

    #[test]
    fn tee_secondary_close_panic_swallowed() {
        struct ClosePanic;
        impl Transport for ClosePanic {
            fn write(&self, _s: &str) -> bool {
                true
            }
            fn close(&self) {
                panic!("boom");
            }
        }
        let (p, _) = MockTransport::new("p");
        let tee = TeeTransport::new(p as Arc<dyn Transport>, vec![Arc::new(ClosePanic) as Arc<dyn Transport>]);
        // should not panic
        tee.close();
    }

    #[test]
    fn tee_empty_secondaries_is_primary_only() {
        let (p, pw) = MockTransport::new("primary");
        let tee = TeeTransport::with_primary_only(p as Arc<dyn Transport>);
        assert_eq!(tee.secondaries_len(), 0);
        let ok = tee.write(r#"{"a":1}"#);
        assert!(ok);
        assert_eq!(pw.lock().unwrap().len(), 1);
        tee.close();
    }
}
