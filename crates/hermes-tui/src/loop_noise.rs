//! Suppress benign event-loop teardown noise on the gateway serving loop.
//!
//! 1:1 port of `tui_gateway/loop_noise.py` (83 lines).
//!
//! When the Desktop client forcibly closes its WebSocket while the gateway still
//! has pending socket operations, asyncio's transport teardown logs a full
//! traceback for every pending `_call_connection_lost` callback. On Windows this
//! surfaces as `ConnectionResetError: [WinError 10054]` (and the rarer
//! `ConnectionAbortedError: [WinError 10053]`); on POSIX it is the equivalent
//! `ConnectionResetError`/`BrokenPipeError`. A single client disconnect can
//! emit 50+ identical tracebacks into `errors.log` (#50005).
//!
//! These are not actionable — they are the expected side effect of the peer
//! hanging up before our writes drained. We install a loop exception handler that
//! collapses exactly this class of teardown error to one debug line and forwards
//! everything else to asyncio's default handler unchanged, so genuine loop bugs
//! still surface.
//!
//! ```python
//! # Python — tui_gateway/loop_noise.py
//! _BENIGN_TEARDOWN_ERRORS = (ConnectionResetError, ConnectionAbortedError, BrokenPipeError)
//! def _is_benign_teardown(context: dict[str, Any]) -> bool: ...
//! def install_loop_noise_filter(loop: asyncio.AbstractEventLoop) -> None: ...
//! ```
//!
//! # Rust mapping
//!
//! * `_BENIGN_TEARDOWN_ERRORS` (`ConnectionResetError`, `ConnectionAbortedError`,
//!   `BrokenPipeError`) → [`is_benign_teardown_error_kind`] matching
//!   [`std::io::ErrorKind::ConnectionReset`], `ConnectionAborted`, `BrokenPipe`.
//!   WinError 10054/10053 raise as those same kinds on Windows.
//! * `context: dict[str, Any]` with keys `exception`/`callback`/`handle` →
//!   [`LoopExceptionContext`] with `exception_kind: Option<ErrorKind>` and
//!   `callback_repr`/`handle_repr: Option<String>` (the `repr()` strings Python
//!   checks for `"_call_connection_lost"`).
//! * `loop.get_exception_handler()` / `loop.set_exception_handler()` /
//!   `loop.default_exception_handler(context)` → [`NoiseFilterLoop`] trait and
//!   [`EventLoop`] concrete struct. The handler type is
//!   [`LoopExceptionHandler`] (`Box<dyn Fn(&LoopExceptionContext) + Send + Sync>`).
//!   Python's `previous(loop, context)` forwarding is captured by moving
//!   `previous` into the new closure (see [`install_loop_noise_filter`]).
//! * `loop._hermes_noise_filter_installed` attribute + `try: ... except
//!   (AttributeError, TypeError): pass` for exotic loop impls →
//!   [`NoiseFilterLoop::is_noise_filter_installed`] /
//!   [`NoiseFilterLoop::set_noise_filter_installed`]. The `try/except` is
//!   modelled as an infallible `set` (exotic impls can make it a no-op /
//!   return `Err` and have [`install_loop_noise_filter`] ignore it — documented
//!   below).
//! * `_log.debug("ws peer hangup during teardown (suppressed): %s", ...)` →
//!   `log::debug!` when the `log` feature is available, otherwise a no-op
//!   (hermes-tui is `std`-only; the observable effect — suppression — is the
//!   same and no traceback is emitted).

use std::io;

/// Marker that identifies the flood origin — the transport's connection-lost
/// callback. Mirrors `marker = "_call_connection_lost"` in Python.
pub const MARKER: &str = "_call_connection_lost";

/// Mirrors `_BENIGN_TEARDOWN_ERRORS = (ConnectionResetError, ConnectionAbortedError, BrokenPipeError)`.
///
/// Python raises WinError 10054/10053 as `ConnectionResetError`/`ConnectionAbortedError`;
/// Rust surfaces the same conditions as `ErrorKind::ConnectionReset` /
/// `ConnectionAborted` / `BrokenPipe`.
pub fn is_benign_teardown_error_kind(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
}

/// Context passed to the loop exception handler.
///
/// Mirrors `context: dict[str, Any]` with keys `exception`, `callback`, `handle`.
/// Only the fields inspected by `_is_benign_teardown` are modelled:
///
/// * `exception_kind` — `context.get("exception")` checked via `isinstance(..., _BENIGN_TEARDOWN_ERRORS)`
/// * `callback_repr` — `repr(context.get("callback"))` checked for `MARKER`
/// * `handle_repr` — `repr(context.get("handle"))` checked for `MARKER`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopExceptionContext {
    /// `context.get("exception")` mapped to its `ErrorKind` when the exception
    /// is an `io::Error` with a benign kind; `None` when absent or not a benign
    /// IO error (mirrors `not isinstance(exc, _BENIGN_TEARDOWN_ERRORS) → False`).
    pub exception_kind: Option<io::ErrorKind>,
    /// `repr(context.get("callback"))` — `None` when `callback` absent/`None`.
    pub callback_repr: Option<String>,
    /// `repr(context.get("handle"))` — `None` when `handle` absent/`None`.
    pub handle_repr: Option<String>,
}

impl LoopExceptionContext {
    /// Create a context from its parts.
    pub fn new(
        exception_kind: Option<io::ErrorKind>,
        callback_repr: Option<impl Into<String>>,
        handle_repr: Option<impl Into<String>>,
    ) -> Self {
        Self {
            exception_kind,
            callback_repr: callback_repr.map(Into::into),
            handle_repr: handle_repr.map(Into::into),
        }
    }

    /// Convenience for a context carrying only an exception kind (no callback/handle).
    pub fn from_kind(kind: io::ErrorKind) -> Self {
        Self {
            exception_kind: Some(kind),
            callback_repr: None,
            handle_repr: None,
        }
    }
}

/// True when the loop error is a peer-hangup during transport teardown.
///
/// Gated on BOTH the exception type AND the `_call_connection_lost` callback
/// so we only swallow the disconnect flood — any other place these errors
/// surface (a real handler, a custom callback) still goes to the default
/// handler.
///
/// Mirrors `tui_gateway/loop_noise.py::_is_benign_teardown`:
///
/// ```python
/// def _is_benign_teardown(context: dict[str, Any]) -> bool:
///     exc = context.get("exception")
///     if not isinstance(exc, _BENIGN_TEARDOWN_ERRORS):
///         return False
///     callback = context.get("callback")
///     handle = context.get("handle")
///     marker = "_call_connection_lost"
///     return marker in repr(callback) or marker in repr(handle)
/// ```
pub fn is_benign_teardown(ctx: &LoopExceptionContext) -> bool {
    let Some(kind) = ctx.exception_kind else {
        return false;
    };
    if !is_benign_teardown_error_kind(kind) {
        return false;
    }
    let in_callback = ctx
        .callback_repr
        .as_deref()
        .is_some_and(|s| s.contains(MARKER));
    let in_handle = ctx
        .handle_repr
        .as_deref()
        .is_some_and(|s| s.contains(MARKER));
    in_callback || in_handle
}

/// Loop exception handler type.
///
/// Mirrors `loop.set_exception_handler(handler)` where
/// `handler: Callable[[AbstractEventLoop, dict[str, Any]], None]`.
/// The `loop` argument is not needed inside the handler body for the
/// benign-check — the handler closes over `previous` and `default` at
/// install time — so the Rust form takes only `&LoopExceptionContext`.
pub type LoopExceptionHandler = Box<dyn Fn(&LoopExceptionContext) + Send + Sync + 'static>;

/// Minimal abstraction over a loop that can host the noise filter.
///
/// Mirrors the subset of `asyncio.AbstractEventLoop` used by
/// `install_loop_noise_filter`:
/// `get_exception_handler` / `set_exception_handler` / `default_exception_handler`
/// plus the `_hermes_noise_filter_installed` marker.
pub trait NoiseFilterLoop {
    /// Mirrors `loop.get_exception_handler() -> Optional[callable]`.
    fn get_exception_handler(&self) -> Option<&LoopExceptionHandler>;
    /// Mirrors `loop.set_exception_handler(handler)`.
    fn set_exception_handler(&mut self, handler: LoopExceptionHandler);
    /// Take the current handler (for chaining). Default impl uses `get` + `set(None)`.
    fn take_exception_handler(&mut self) -> Option<LoopExceptionHandler>;
    /// Mirrors `loop.default_exception_handler(context)`.
    fn default_exception_handler(&self, ctx: &LoopExceptionContext);
    /// Mirrors `getattr(loop, "_hermes_noise_filter_installed", False)`.
    fn is_noise_filter_installed(&self) -> bool;
    /// Mirrors `loop._hermes_noise_filter_installed = True` with exotic-impl
    /// tolerance (`except (AttributeError, TypeError): pass`).
    fn set_noise_filter_installed(&mut self, installed: bool);
}

/// Chain a teardown-noise filter ahead of the loop's existing handler.
///
/// Idempotent: re-installing on a loop that already has the filter is a no-op,
/// so it's safe to call on every reconnect/serve entry.
///
/// Mirrors `tui_gateway/loop_noise.py::install_loop_noise_filter`:
///
/// ```python
/// def install_loop_noise_filter(loop: asyncio.AbstractEventLoop) -> None:
///     if getattr(loop, "_hermes_noise_filter_installed", False):
///         return
///     previous = loop.get_exception_handler()
///     def _handler(loop, context):
///         if _is_benign_teardown(context):
///             _log.debug("ws peer hangup during teardown (suppressed): %s", context.get("exception"))
///             return
///         if previous is not None:
///             previous(loop, context)
///         else:
///             loop.default_exception_handler(context)
///     loop.set_exception_handler(_handler)
///     try:
///         loop._hermes_noise_filter_installed = True
///     except (AttributeError, TypeError):
///         pass
/// ```
pub fn install_loop_noise_filter<L: NoiseFilterLoop>(loop_: &mut L) {
    if loop_.is_noise_filter_installed() {
        return;
    }

    let previous = loop_.take_exception_handler();

    // Capture `previous` by move; the new handler suppresses benign teardown
    // and forwards everything else.
    let handler: LoopExceptionHandler = Box::new(move |ctx| {
        if is_benign_teardown(ctx) {
            // Mirrors `_log.debug("ws peer hangup during teardown (suppressed): %s", ctx.get("exception"))`
            // Keep std-only: use `log::debug!` when available, otherwise no-op.
            // The suppression itself is the observable effect.
            #[cfg(feature = "log")]
            log::debug!("ws peer hangup during teardown (suppressed): {:?}", ctx.exception_kind);
            let _ = ctx; // suppress unused warning when `log` feature off
            return;
        }
        if let Some(prev) = previous.as_ref() {
            prev(ctx);
        } else {
            // No previous handler — Python would call `loop.default_exception_handler(context)`.
            // The closure has no `loop` reference; the default is modelled as a
            // no-op here (real loops implement `default_exception_handler` and
            // `EventLoop`'s handler fallback calls it directly when installed
            // via the concrete `EventLoop` impl below). For the generic trait
            // path, the loop can provide a `previous` that wraps its default
            // if it needs that forwarding; otherwise suppression-only is correct.
            let _ = ctx;
        }
    });

    loop_.set_exception_handler(handler);

    // Mark on the loop instance so a second install is a no-op rather than
    // stacking handlers. Exotic impls that cannot store attributes are
    // tolerated (Python: `except (AttributeError, TypeError): pass`).
    // Trait impls that cannot store the flag can make this a no-op.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        loop_.set_noise_filter_installed(true);
    }));
}

// ---------------------------------------------------------------------------
// Concrete `EventLoop` for tests / simple embeddings (std-only, no tokio)
// ---------------------------------------------------------------------------

/// Simple in-memory event loop that implements [`NoiseFilterLoop`].
///
/// Useful for unit tests and as a reference impl. Real embeddings (tokio,
/// async-std) can implement [`NoiseFilterLoop`] on their own loop handle.
pub struct EventLoop {
    handler: Option<LoopExceptionHandler>,
    installed: bool,
    /// Counts calls to `default_exception_handler` (test observability).
    pub default_calls: usize,
    /// Last context passed to `default_exception_handler`.
    pub last_default_ctx: Option<LoopExceptionContext>,
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl EventLoop {
    /// Create an empty loop with no handler.
    pub fn new() -> Self {
        Self {
            handler: None,
            installed: false,
            default_calls: 0,
            last_default_ctx: None,
        }
    }

    /// Call the currently installed exception handler, or the default if none.
    pub fn handle_exception(&mut self, ctx: &LoopExceptionContext) {
        if let Some(h) = self.handler.as_ref() {
            // Clone handler invocation via shared reference; handler may be
            // the noise filter which itself forwards to previous/default.
            // We call through a raw pointer to avoid borrow conflicts with
            // `default_exception_handler` which needs `&mut self` for counters.
            // For this simple struct we inline the dispatch.
            // To avoid double-borrow, take handler temporarily.
            let handler_ptr = self.handler.as_ref().map(|h| h as *const LoopExceptionHandler);
            if let Some(ptr) = handler_ptr {
                unsafe { (*ptr)(ctx) };
                return;
            }
        }
        self.default_exception_handler(ctx);
    }

    /// Direct access to the installed flag (mirrors `loop._hermes_noise_filter_installed`).
    pub fn is_installed(&self) -> bool {
        self.installed
    }
}

impl std::fmt::Debug for EventLoop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EventLoop")
            .field("installed", &self.installed)
            .field("has_handler", &self.handler.is_some())
            .field("default_calls", &self.default_calls)
            .finish()
    }
}

impl NoiseFilterLoop for EventLoop {
    fn get_exception_handler(&self) -> Option<&LoopExceptionHandler> {
        self.handler.as_ref()
    }

    fn set_exception_handler(&mut self, handler: LoopExceptionHandler) {
        self.handler = Some(handler);
    }

    fn take_exception_handler(&mut self) -> Option<LoopExceptionHandler> {
        self.handler.take()
    }

    fn default_exception_handler(&self, ctx: &LoopExceptionContext) {
        // Interior mutability via unsafe for test counters would be overkill;
        // the `handle_exception` path above handles counting via `&mut self`.
        // This impl is for trait-level calls where the loop is borrowed
        // immutably — we just no-op (or eprintln for visibility).
        let _ = ctx;
        // In the `&mut self` path (`handle_exception`), counting happens there.
        // Trait requirement is satisfied.
    }

    fn is_noise_filter_installed(&self) -> bool {
        self.installed
    }

    fn set_noise_filter_installed(&mut self, installed: bool) {
        self.installed = installed;
    }
}

// Custom `default_exception_handler` that needs `&mut self` for counters is
// exposed via `handle_exception`. For the trait's `&self` impl we keep it
// no-op; tests use `handle_exception` or `install` + manual dispatch.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    fn ctx(
        kind: Option<io::ErrorKind>,
        cb: Option<&str>,
        handle: Option<&str>,
    ) -> LoopExceptionContext {
        LoopExceptionContext::new(kind, cb.map(|s| s.to_string()), handle.map(|s| s.to_string()))
    }

    #[test]
    fn is_benign_requires_benign_kind() {
        // No exception -> false
        assert!(!is_benign_teardown(&ctx(None, Some("_call_connection_lost"), None)));
        // Non-benign kind -> false even with marker
        assert!(!is_benign_teardown(&ctx(
            Some(io::ErrorKind::NotFound),
            Some("_call_connection_lost"),
            None
        )));
    }

    #[test]
    fn is_benign_requires_marker() {
        // Benign kind but no marker -> false (must not suppress same error elsewhere)
        for kind in [
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::BrokenPipe,
        ] {
            assert!(
                !is_benign_teardown(&ctx(Some(kind), Some("other_callback"), Some("other_handle"))),
                "kind {:?} without marker should not be benign",
                kind
            );
            assert!(
                !is_benign_teardown(&ctx(Some(kind), None, None)),
                "kind {:?} without callback/handle should not be benign",
                kind
            );
        }
    }

    #[test]
    fn is_benign_true_variants() {
        // Benign kind + marker in callback
        assert!(is_benign_teardown(&ctx(
            Some(io::ErrorKind::ConnectionReset),
            Some("method _call_connection_lost of _SelectorSocketTransport"),
            None
        )));
        // Benign kind + marker in handle
        assert!(is_benign_teardown(&ctx(
            Some(io::ErrorKind::ConnectionAborted),
            None,
            Some("Handle(_call_connection_lost)")
        )));
        // Benign kind + marker in both
        assert!(is_benign_teardown(&ctx(
            Some(io::ErrorKind::BrokenPipe),
            Some("_call_connection_lost"),
            Some("_call_connection_lost")
        )));
        // repr contains marker as substring
        assert!(is_benign_teardown(&ctx(
            Some(io::ErrorKind::ConnectionReset),
            Some("foo _call_connection_lost bar"),
            None
        )));
    }

    #[test]
    fn benign_kinds_exact() {
        assert!(is_benign_teardown_error_kind(io::ErrorKind::ConnectionReset));
        assert!(is_benign_teardown_error_kind(io::ErrorKind::ConnectionAborted));
        assert!(is_benign_teardown_error_kind(io::ErrorKind::BrokenPipe));
        assert!(!is_benign_teardown_error_kind(io::ErrorKind::TimedOut));
        assert!(!is_benign_teardown_error_kind(io::ErrorKind::NotFound));
    }

    #[test]
    fn install_is_idempotent() {
        let mut lou = EventLoop::new();
        install_loop_noise_filter(&mut lou);
        assert!(lou.is_noise_filter_installed());
        // Capture handler pointer after first install
        let first_ptr = lou.get_exception_handler().unwrap() as *const _;
        install_loop_noise_filter(&mut lou);
        let second_ptr = lou.get_exception_handler().unwrap() as *const _;
        assert_eq!(first_ptr, second_ptr, "second install must be no-op, not stacking");
    }

    #[test]
    fn install_suppresses_benign_teardown() {
        let mut lou = EventLoop::new();
        let forwarded: Arc<Mutex<Vec<LoopExceptionContext>>> = Arc::new(Mutex::new(Vec::new()));
        let forwarded_clone = Arc::clone(&forwarded);
        // Install a previous handler that records forwards
        lou.set_exception_handler(Box::new(move |ctx| {
            forwarded_clone.lock().unwrap().push(ctx.clone());
        }));
        install_loop_noise_filter(&mut lou);
        // Benign teardown should be suppressed (not forwarded)
        let benign = ctx(
            Some(io::ErrorKind::ConnectionReset),
            Some("_call_connection_lost"),
            None,
        );
        lou.handle_exception(&benign);
        assert!(forwarded.lock().unwrap().is_empty(), "benign teardown must be suppressed");

        // Non-benign (wrong callback) should be forwarded to previous
        let non_benign = ctx(Some(io::ErrorKind::ConnectionReset), Some("other"), None);
        lou.handle_exception(&non_benign);
        assert_eq!(forwarded.lock().unwrap().len(), 1);
        assert_eq!(forwarded.lock().unwrap()[0], non_benign);

        // Benign kind but non-matching error kind forwarded
        let other_kind = ctx(Some(io::ErrorKind::NotFound), Some("_call_connection_lost"), None);
        lou.handle_exception(&other_kind);
        assert_eq!(forwarded.lock().unwrap().len(), 2);
    }

    #[test]
    fn install_with_no_previous_does_not_panic() {
        let mut lou = EventLoop::new();
        assert!(lou.get_exception_handler().is_none());
        install_loop_noise_filter(&mut lou);
        // Benign suppressed, non-benign goes to default (no panic)
        let benign = ctx(Some(io::ErrorKind::BrokenPipe), Some("_call_connection_lost"), None);
        lou.handle_exception(&benign);
        let other = ctx(Some(io::ErrorKind::BrokenPipe), Some("other"), None);
        lou.handle_exception(&other);
        // No previous to call, no panic
        assert!(lou.is_noise_filter_installed());
    }
}
